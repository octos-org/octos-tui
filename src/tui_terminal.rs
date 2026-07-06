//! Inline-viewport terminal — ported and trimmed from codex-rs `tui/src/custom_terminal.rs`.
//!
//! # Why this exists
//!
//! The default `ratatui::Terminal` (and octos-tui's previous use of it inside an
//! `EnterAlternateScreen` fullscreen buffer) repaints the *entire* screen every
//! frame. That has two fatal consequences for a chat/agent TUI:
//!
//! 1. The alternate screen has **no scrollback**, so the user cannot scroll up
//!    to prior output with the terminal's own scrollbar / wheel / tmux copy-mode.
//! 2. Every repaint rewrites the screen cells, so any **native text selection**
//!    the user starts gets wiped on the next frame.
//!
//! codex solves both by *not* using the alternate screen for its main chat: it
//! keeps an **inline viewport** pinned to the bottom of the screen (just the
//! live composer/status), and writes finalized history into the terminal's
//! **normal scrollback** via escape sequences ([`crate::insert_history`]). The
//! scrollback then belongs to the terminal — so native mouse-select, wheel
//! scroll, and tmux copy-mode all work with no app mode key.
//!
//! This is a faithful but trimmed port: we keep the inline-viewport bookkeeping
//! ([`Terminal::set_viewport_area`], the buffer diffing in [`Terminal::flush`],
//! the cursor/clear helpers) and drop the bits octos-tui does not need for the
//! first cut (Zellij raw-newline scrolling, `^Z` suspend resume, OSC-width
//! special-casing). `unsafe_code` is denied workspace-wide, and nothing here
//! needs it.
//!
//! Derived from `ratatui::Terminal`, MIT licensed (c) 2016-2025 The Ratatui
//! Developers, and from codex-rs which is also MIT/Apache licensed.

use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use ratatui::backend::Backend;
use ratatui::backend::ClearType;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// The slice of `ratatui::Frame` that octos-tui's render code uses. Implemented
/// by both `ratatui::Frame` (the full-screen overlay path) and the inline
/// [`Frame`] below (the scrollback/inline-viewport path), so the `render_*`
/// functions in `app.rs` can be written once against `&mut impl FrameLike` and
/// drive either renderer.
pub trait FrameLike {
    fn area(&self) -> Rect;
    fn render_widget<W: Widget>(&mut self, widget: W, area: Rect);
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P);
    fn buffer_mut(&mut self) -> &mut Buffer;
}

impl FrameLike for ratatui::Frame<'_> {
    fn area(&self) -> Rect {
        ratatui::Frame::area(self)
    }
    fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        ratatui::Frame::render_widget(self, widget, area);
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        ratatui::Frame::set_cursor_position(self, position);
    }
    fn buffer_mut(&mut self) -> &mut Buffer {
        ratatui::Frame::buffer_mut(self)
    }
}

/// A render frame handed to the inline-viewport draw closure. Mirrors the slice
/// of `ratatui::Frame` that octos-tui's render code actually uses.
pub struct Frame<'a> {
    pub(crate) cursor_position: Option<Position>,
    pub(crate) viewport_area: Rect,
    pub(crate) buffer: &'a mut Buffer,
}

impl<'a> Frame<'a> {
    /// Construct a `Frame` over an arbitrary buffer for tests/render-into-buffer.
    #[cfg(test)]
    pub(crate) fn for_test(area: Rect, buffer: &'a mut Buffer) -> Self {
        Frame {
            cursor_position: None,
            viewport_area: area,
            buffer,
        }
    }
}

impl FrameLike for Frame<'_> {
    fn area(&self) -> Rect {
        self.viewport_area
    }

    #[allow(clippy::needless_pass_by_value)]
    fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }

    fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }
}

/// An inline-viewport terminal.
///
/// Unlike `ratatui::Terminal`, the drawable area is a `viewport_area` rectangle
/// that occupies only the bottom rows of the screen. Everything above it is the
/// terminal's normal scrollback, which we never repaint — history is pushed
/// there once via [`crate::insert_history::insert_history_lines`].
pub struct Terminal<B>
where
    B: Backend + Write,
{
    backend: B,
    /// Double buffer: `buffers[current]` is what we are about to draw,
    /// `buffers[1 - current]` is what is on screen. The diff between them is the
    /// minimal set of cell updates we emit, so an unchanged frame writes nothing.
    buffers: [Buffer; 2],
    current: usize,
    hidden_cursor: bool,
    /// The rectangle (bottom of the screen) we are allowed to draw into.
    pub viewport_area: Rect,
    /// Last screen size we saw, to detect resizes.
    pub last_known_screen_size: Size,
    /// Last cursor position we placed, so history insertion can restore it.
    pub last_known_cursor_pos: Position,
    /// Count of visible history rows currently occupying the area above the
    /// inline viewport. Rows above the viewport that have never held inserted
    /// history are spare capacity, not blank transcript separators.
    visible_history_rows: u16,
    /// One-past-the-last row occupied by visible history above the viewport.
    /// This lets history remain bottom-adjacent normally while still tracking
    /// a blank gap if the live viewport later moves down.
    visible_history_bottom: u16,
}

impl<B> Drop for Terminal<B>
where
    B: Backend + Write,
{
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.show_cursor();
        }
    }
}

impl<B> Terminal<B>
where
    B: Backend + Write,
{
    /// Create an inline terminal anchored at the current cursor row. The
    /// viewport starts with zero height; the first [`Terminal::draw`] sizes it.
    pub fn new(mut backend: B) -> io::Result<Self> {
        let screen_size = backend.size()?;
        let cursor_pos = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        Ok(Self {
            backend,
            buffers: [Buffer::empty(Rect::ZERO), Buffer::empty(Rect::ZERO)],
            current: 0,
            hidden_cursor: false,
            viewport_area: Rect::new(0, cursor_pos.y, 0, 0),
            last_known_screen_size: screen_size,
            last_known_cursor_pos: cursor_pos,
            visible_history_rows: 0,
            visible_history_bottom: 0,
        })
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.current]
    }

    fn previous_buffer(&self) -> &Buffer {
        &self.buffers[1 - self.current]
    }

    fn previous_buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[1 - self.current]
    }

    /// Move/resize the inline viewport. Resizes the double buffers to match so
    /// the next draw paints into the new rectangle.
    pub fn set_viewport_area(&mut self, area: Rect) {
        self.current_buffer_mut().resize(area);
        self.previous_buffer_mut().resize(area);
        self.viewport_area = area;
        self.visible_history_rows = self.visible_history_rows.min(area.top());
        self.visible_history_bottom = self.visible_history_bottom.min(area.top());
        self.visible_history_rows = self.visible_history_rows.min(self.visible_history_bottom);
    }

    pub(crate) fn visible_history_rows(&self) -> u16 {
        self.visible_history_rows
    }

    pub(crate) fn visible_history_bottom(&self) -> u16 {
        self.visible_history_bottom
    }

    pub(crate) fn set_visible_history_extent(&mut self, rows: u16, bottom: u16) {
        self.visible_history_bottom = bottom.min(self.viewport_area.top());
        self.visible_history_rows = rows.min(self.viewport_area.top());
        self.visible_history_rows = self.visible_history_rows.min(self.visible_history_bottom);
    }

    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// Pin the inline viewport to `height` rows at the bottom of the screen,
    /// scrolling existing content up if the viewport would run off the bottom.
    /// This is the per-frame analogue of codex's `update_inline_viewport`.
    pub fn resize_viewport_to(&mut self, height: u16) -> io::Result<()> {
        let size = self.backend.size()?;
        self.resize_viewport_to_size(height, size)
    }

    /// Whether [`Terminal::resize_viewport_to`] would emit any terminal writes
    /// for the supplied screen size. Used by the event loop to decide whether
    /// DEC synchronized update wrapping is needed before the draw.
    pub fn viewport_resize_needed(&self, height: u16, size: Size) -> bool {
        let mut area = self.viewport_area;
        area.height = height.min(size.height).max(1);
        area.width = size.width;
        // Overflow triggers a scroll + reanchor, which writes bytes.
        if area.bottom() > size.height {
            return true;
        }
        // No overflow: area.y is left unchanged (codex-rs: terminal Y growth is
        // handled by insert_history_lines, not here). Only height/width changes
        // require a clear + set_viewport_area.
        area != self.viewport_area
    }

    /// Reposition the viewport to match the requested `height` within `size`.
    ///
    /// Mirrors the resize logic in codex-rs `Tui::draw`:
    ///
    /// - If the viewport overflows below the screen (`area.bottom() > size.height`
    ///   — covers both terminal shrink and viewport-height increases that don't
    ///   fit), scroll the rows above the viewport up into scrollback and reanchor
    ///   the viewport at the new screen bottom.
    /// - Otherwise (`area.bottom() <= size.height`, i.e. terminal grew in Y or
    ///   nothing changed), leave `area.y` alone. `insert_history_lines` will slide
    ///   the viewport toward the screen bottom via ESC M (Reverse Index) in its
    ///   Standard mode as history arrives — this is the exact mechanism codex-rs
    ///   relies on and requires no explicit reposition here.
    fn resize_viewport_to_size(&mut self, height: u16, size: Size) -> io::Result<()> {
        let mut area = self.viewport_area;
        area.height = height.min(size.height).max(1);
        area.width = size.width;

        if area.bottom() > size.height {
            let scroll_by = area.bottom() - size.height;
            scroll_region_up(&mut self.backend, area.top(), scroll_by)?;
            self.visible_history_bottom = self.visible_history_bottom.saturating_sub(scroll_by);
            self.visible_history_rows = self.visible_history_rows.min(self.visible_history_bottom);
            area.y = size.height.saturating_sub(area.height);
        }

        if area != self.viewport_area {
            // Clear from the old viewport top (codex-rs `clear_for_viewport_change`).
            // On first draw the old area is empty; clear from the new top instead so
            // stale shell cells don't show through the initial render.
            let clear_position = if self.viewport_area.is_empty() {
                area.as_position()
            } else {
                self.viewport_area.as_position()
            };
            self.clear_after_position(clear_position)?;
            self.set_viewport_area(area);
        }

        self.last_known_screen_size = size;
        Ok(())
    }

    /// Draw a single frame into the inline viewport. Only the cells that changed
    /// since the previous frame are written to the backend, so a no-op redraw
    /// emits nothing (and therefore never wipes a native selection in scrollback).
    pub fn draw<F>(&mut self, render_callback: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        let mut frame = self.get_frame();
        render_callback(&mut frame);
        let cursor_position = frame.cursor_position;

        // A no-change frame must emit ZERO bytes. The inline-viewport model leaves
        // finalized output in the terminal's real scrollback, and any write (even
        // a redundant cursor move) can drop the user's in-progress native text
        // selection. So we only touch the backend when the cell diff produced
        // updates or the cursor state actually has to change. This mirrors codex,
        // which draws solely on a scheduled frame; here the event loop may tick
        // (e.g. the spinner cadence while a turn runs) without anything visually
        // changing, and those ticks must not repaint.
        let wrote_cells = self.flush()?;

        let cursor_changed = match cursor_position {
            None => {
                if self.hidden_cursor {
                    false
                } else {
                    self.hide_cursor()?;
                    true
                }
            }
            Some(position) => {
                let mut changed = false;
                if self.hidden_cursor {
                    self.show_cursor()?;
                    changed = true;
                }
                // After `flush()` emits `Print` for changed cells, the PHYSICAL
                // terminal cursor is left wherever the last `Print` advanced it —
                // not necessarily `last_known_cursor_pos` (which tracks the last
                // written cell's start). So whenever we wrote cells we must
                // re-place the cursor even if the requested position equals our
                // tracked one (codex P2: e.g. Backspace at the composer end left
                // the cursor one column too far right). When nothing was written
                // (idle) this stays a no-op, preserving the zero-byte invariant.
                if wrote_cells || self.last_known_cursor_pos != position {
                    self.set_cursor_position(position)?;
                    changed = true;
                }
                changed
            }
        };

        self.swap_buffers();
        if wrote_cells || cursor_changed {
            Backend::flush(&mut self.backend)?;
        }
        Ok(())
    }

    fn get_frame(&mut self) -> Frame<'_> {
        let viewport_area = self.viewport_area;
        Frame {
            cursor_position: None,
            viewport_area,
            buffer: self.current_buffer_mut(),
        }
    }

    /// Diff the current vs previous buffer and emit only the changes. Returns
    /// `true` when at least one cell update was written. When the diff is empty
    /// we emit NOTHING — not even the trailing SGR reset — so an unchanged frame
    /// is a true no-op and cannot disturb a native scrollback selection.
    fn flush(&mut self) -> io::Result<bool> {
        let updates = diff_buffers(self.previous_buffer(), &self.buffers[self.current]);
        if updates.is_empty() {
            return Ok(false);
        }
        if let Some(&(x, y, _)) = updates.last() {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw(&mut self.backend, updates.into_iter())?;
        Ok(true)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.backend.hide_cursor()?;
        self.hidden_cursor = true;
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.backend.show_cursor()?;
        self.hidden_cursor = false;
        Ok(())
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.backend.set_cursor_position(position)?;
        self.last_known_cursor_pos = position;
        Ok(())
    }

    /// Clear the viewport region and force a full repaint on the next draw.
    pub fn clear(&mut self) -> io::Result<()> {
        if self.viewport_area.is_empty() {
            return Ok(());
        }
        self.clear_after_position(self.viewport_area.as_position())
    }

    /// Clear from `position` through the end of the visible screen and force a
    /// full repaint on the next draw.
    pub(crate) fn clear_after_position(&mut self, position: Position) -> io::Result<()> {
        self.backend.set_cursor_position(position)?;
        self.backend.clear_region(ClearType::AfterCursor)?;
        if position.y <= self.viewport_area.top() {
            self.visible_history_rows = self.visible_history_rows.min(position.y);
            self.visible_history_bottom = self.visible_history_bottom.min(position.y);
            self.visible_history_rows = self.visible_history_rows.min(self.visible_history_bottom);
        }
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Clear the whole visible screen and force a full repaint on the next draw.
    pub(crate) fn clear_visible_screen(&mut self) -> io::Result<()> {
        let home = Position { x: 0, y: 0 };
        self.backend.set_cursor_position(home)?;
        self.backend.clear_region(ClearType::All)?;
        self.backend.set_cursor_position(home)?;
        self.visible_history_rows = 0;
        self.visible_history_bottom = 0;
        self.previous_buffer_mut().reset();
        Ok(())
    }

    /// Drop the diff buffer so the next draw repaints every cell. Call after we
    /// move screen content outside ratatui's knowledge (e.g. history insertion).
    pub fn invalidate_viewport(&mut self) {
        self.previous_buffer_mut().reset();
    }

    fn swap_buffers(&mut self) {
        self.previous_buffer_mut().reset();
        self.current = 1 - self.current;
    }
}

/// Scroll the rows in `[0, region_bottom)` up by `scroll_by` rows, pushing the
/// rows that scroll off the top into the terminal's scrollback. Emitted as a
/// DECSTBM scroll region + Index (`ESC D`) so we don't need ratatui's optional
/// `scrolling-regions` Backend feature. Cursor-position-neutral.
fn scroll_region_up<W: Write>(w: &mut W, region_bottom: u16, scroll_by: u16) -> io::Result<()> {
    if scroll_by == 0 || region_bottom == 0 {
        return Ok(());
    }
    // Region is 1-based inclusive: rows 1..=region_bottom.
    write!(w, "\x1b[1;{region_bottom}r")?;
    // Move to the bottom row of the region, then Index `scroll_by` times to
    // scroll the region's content up.
    write!(w, "\x1b[{region_bottom};1H")?;
    for _ in 0..scroll_by {
        write!(w, "\x1bD")?; // Index (ESC D): move down / scroll region up at bottom.
    }
    // Reset the scroll region to the full screen.
    write!(w, "\x1b[r")?;
    Ok(())
}

/// `(x, y, cell)` updates that must be written this frame.
fn diff_buffers(previous: &Buffer, next: &Buffer) -> Vec<(u16, u16, Cell)> {
    let prev = &previous.content;
    let cur = &next.content;
    let mut updates = Vec::new();
    let mut invalidated: usize = 0;
    let mut to_skip: usize = 0;
    for (i, (current, old)) in cur.iter().zip(prev.iter()).enumerate() {
        if !current.skip && (current != old || invalidated > 0) && to_skip == 0 {
            let (x, y) = next.pos_of(i);
            updates.push((x, y, current.clone()));
        }
        to_skip = current.symbol().width().saturating_sub(1);
        let affected = current.symbol().width().max(old.symbol().width());
        invalidated = affected.max(invalidated).saturating_sub(1);
    }
    updates
}

/// Emit cell updates to the backend, tracking color/modifier state so we only
/// emit escape sequences when they change.
fn draw<B, I>(backend: &mut B, updates: I) -> io::Result<()>
where
    B: Write,
    I: Iterator<Item = (u16, u16, Cell)>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut modifier = Modifier::empty();
    let mut last_pos: Option<(u16, u16)> = None;
    for (x, y, cell) in updates {
        if !matches!(last_pos, Some((px, py)) if x == px + 1 && y == py) {
            queue!(backend, MoveTo(x, y))?;
        }
        last_pos = Some((x, y));

        if cell.modifier != modifier {
            queue_modifier_diff(backend, modifier, cell.modifier)?;
            modifier = cell.modifier;
        }
        if cell.fg != fg || cell.bg != bg {
            queue!(
                backend,
                SetColors(Colors::new(cell.fg.into(), cell.bg.into()))
            )?;
            fg = cell.fg;
            bg = cell.bg;
        }
        queue!(backend, Print(cell.symbol()))?;
    }
    queue!(
        backend,
        SetForegroundColor(crossterm::style::Color::Reset),
        SetBackgroundColor(crossterm::style::Color::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )?;
    Ok(())
}

fn queue_modifier_diff<W: Write>(w: &mut W, from: Modifier, to: Modifier) -> io::Result<()> {
    use crossterm::style::Attribute as A;
    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        queue!(w, SetAttribute(A::NoReverse))?;
    }
    if removed.contains(Modifier::BOLD) {
        queue!(w, SetAttribute(A::NormalIntensity))?;
        if to.contains(Modifier::DIM) {
            queue!(w, SetAttribute(A::Dim))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(w, SetAttribute(A::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(w, SetAttribute(A::NoUnderline))?;
    }
    if removed.contains(Modifier::DIM) {
        queue!(w, SetAttribute(A::NormalIntensity))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(w, SetAttribute(A::NotCrossedOut))?;
    }
    if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
        queue!(w, SetAttribute(A::NoBlink))?;
    }

    let added = to - from;
    if added.contains(Modifier::REVERSED) {
        queue!(w, SetAttribute(A::Reverse))?;
    }
    if added.contains(Modifier::BOLD) {
        queue!(w, SetAttribute(A::Bold))?;
    }
    if added.contains(Modifier::ITALIC) {
        queue!(w, SetAttribute(A::Italic))?;
    }
    if added.contains(Modifier::UNDERLINED) {
        queue!(w, SetAttribute(A::Underlined))?;
    }
    if added.contains(Modifier::DIM) {
        queue!(w, SetAttribute(A::Dim))?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        queue!(w, SetAttribute(A::CrossedOut))?;
    }
    if added.contains(Modifier::SLOW_BLINK) {
        queue!(w, SetAttribute(A::SlowBlink))?;
    }
    if added.contains(Modifier::RAPID_BLINK) {
        queue!(w, SetAttribute(A::RapidBlink))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::cursor::{Hide, MoveTo, Show};
    use ratatui::backend::WindowSize;
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    /// A `Backend + Write` that records every emitted byte, including the cursor
    /// escapes crossterm would emit (so a "no-op draw" can be asserted to write
    /// exactly zero bytes — the property that protects a native selection).
    struct RecordingBackend {
        buf: Vec<u8>,
        size: Size,
        cursor: Position,
        clears: Vec<ClearType>,
    }

    impl RecordingBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                buf: Vec::new(),
                size: Size::new(width, height),
                cursor: Position { x: 0, y: 0 },
                clears: Vec::new(),
            }
        }
    }

    impl Write for RecordingBackend {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for RecordingBackend {
        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Ok(())
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            queue!(self.buf, Hide)
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            queue!(self.buf, Show)
        }
        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            let position = position.into();
            self.cursor = position;
            queue!(self.buf, MoveTo(position.x, position.y))
        }
        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
            self.clears.push(clear_type);
            Ok(())
        }
        fn size(&self) -> io::Result<Size> {
            Ok(self.size)
        }
        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.size,
                pixels: Size::new(0, 0),
            })
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn render_hi(frame: &mut Frame) {
        let area = frame.area();
        frame.render_widget(Paragraph::new(Line::from("hi")), area);
    }

    #[test]
    fn unchanged_frame_emits_zero_bytes_so_selection_survives() {
        // Bug 2: an idle / no-change draw must write NOTHING to the terminal, or
        // it would disturb the user's native scrollback selection. After the
        // first (real) draw, a second identical draw with the SAME cursor target
        // must be a complete no-op (zero bytes).
        let mut terminal = Terminal::new(RecordingBackend::new(20, 5)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 4, 20, 1));

        // First draw paints "hi" and positions the cursor: this emits bytes.
        terminal
            .draw(|frame| {
                render_hi(frame);
                frame.set_cursor_position((2, 4));
            })
            .expect("first draw");
        assert!(
            !terminal.backend().buf.is_empty(),
            "the first draw should emit the initial paint"
        );

        // Second draw is byte-identical content AND the same cursor position.
        let before = terminal.backend().buf.len();
        terminal
            .draw(|frame| {
                render_hi(frame);
                frame.set_cursor_position((2, 4));
            })
            .expect("second draw");
        let after = terminal.backend().buf.len();
        assert_eq!(
            before, after,
            "an unchanged frame must emit zero bytes (selection-safe no-op)"
        );
    }

    #[test]
    fn cursor_is_replaced_after_writing_the_target_cell() {
        // codex P2: after flush() Prints changed cells, the PHYSICAL cursor sits
        // past the last cell. If the requested cursor equals our tracked logical
        // position (the written cell's start), the guard must NOT skip the
        // MoveTo, else the cursor renders one column too far right (e.g.
        // Backspace at the composer end). When nothing was written this stays a
        // no-op (covered by `unchanged_frame_emits_zero_bytes...`).
        let mut terminal = Terminal::new(RecordingBackend::new(20, 5)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 4, 20, 1));

        // First draw: "hi"; cursor parked at (2,4) -> tracked cursor = (2,4).
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(Line::from("hi")), area);
                frame.set_cursor_position((2, 4));
            })
            .expect("first draw");
        let mark = terminal.backend().buf.len();

        // Second draw: cell (2,4) changes to 'X' AND the cursor is requested on
        // it — the exact collision case from the bug.
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(Line::from("hiX")), area);
                frame.set_cursor_position((2, 4));
            })
            .expect("second draw");

        let delta = &terminal.backend().buf[mark..];
        // crossterm MoveTo(col=2,row=4) == "ESC[5;3H" (1-based). It appears once
        // for the cell write; the fix adds a second to re-place the cursor.
        let needle = b"\x1b[5;3H";
        let moves = delta.windows(needle.len()).filter(|w| *w == needle).count();
        assert!(
            moves >= 2,
            "cursor must be re-placed after writing its target cell; got {moves} MoveTo(2,4) in delta={:?}",
            String::from_utf8_lossy(delta)
        );
    }

    #[test]
    fn changed_cell_repaints_only_the_delta() {
        // A genuine content change still paints (so streaming output is visible);
        // only the no-change case is suppressed.
        let mut terminal = Terminal::new(RecordingBackend::new(20, 5)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 4, 20, 1));

        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(Line::from("aaa")), area);
                frame.set_cursor_position((3, 4));
            })
            .expect("first draw");

        let before = terminal.backend().buf.len();
        terminal
            .draw(|frame| {
                let area = frame.area();
                frame.render_widget(Paragraph::new(Line::from("bbb")), area);
                frame.set_cursor_position((3, 4));
            })
            .expect("second draw");
        assert!(
            terminal.backend().buf.len() > before,
            "a real content change must repaint"
        );
    }

    #[test]
    fn terminal_shrink_reanchors_and_clears() {
        // Codex-rs Tui::draw logic: on terminal shrink the overflow branch fires,
        // scroll_region_up (ESC D) is emitted, and the viewport is reanchored at
        // the new bottom. The clear position is the OLD viewport top (codex-rs
        // clear_for_viewport_change), so the cursor lands there, not the new top.
        // Real terminals clamp out-of-bounds DECSTBM regions to the visible screen.
        let mut terminal = Terminal::new(RecordingBackend::new(200, 50)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 46, 200, 4));
        terminal.last_known_screen_size = Size::new(200, 50);
        terminal.backend_mut().size = Size::new(130, 38);

        terminal.resize_viewport_to(4).expect("resize viewport");

        assert_eq!(terminal.viewport_area, Rect::new(0, 34, 130, 4));
        // Cursor is at old viewport top (codex-rs clear_for_viewport_change uses
        // the old viewport position; real terminals clamp row 46 to the last row).
        assert_eq!(terminal.backend().cursor, Position { x: 0, y: 46 });
        assert_eq!(terminal.backend().clears, vec![ClearType::AfterCursor]);
        let written = String::from_utf8_lossy(&terminal.backend().buf);
        assert!(
            written.contains("\u{1b}D"),
            "overflow path must scroll rows above the viewport up; wrote {written:?}"
        );
    }

    #[test]
    fn width_resize_clears_from_existing_viewport_top() {
        let mut terminal = Terminal::new(RecordingBackend::new(200, 50)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 45, 200, 5));
        terminal.last_known_screen_size = Size::new(200, 50);
        terminal.backend_mut().size = Size::new(130, 50);

        terminal.resize_viewport_to(5).expect("resize viewport");

        assert_eq!(terminal.viewport_area, Rect::new(0, 45, 130, 5));
        assert_eq!(terminal.backend().cursor, Position { x: 0, y: 45 });
        assert_eq!(terminal.backend().clears, vec![ClearType::AfterCursor]);
    }

    #[test]
    fn terminal_y_grow_keeps_viewport_in_place_no_clear() {
        // When the terminal grows in Y only (same width, same TUI height), the
        // viewport must NOT jump to the new bottom row or clear anything. Codex-rs
        // never has explicit reposition logic for this; insert_history_lines
        // slides the viewport down via ESC M as history arrives.
        let mut terminal = Terminal::new(RecordingBackend::new(200, 50)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 25, 200, 25));
        terminal.last_known_screen_size = Size::new(200, 50);
        terminal.backend_mut().size = Size::new(200, 55); // Y grew by 5

        terminal.resize_viewport_to(25).expect("resize viewport");

        // Viewport must stay at old position (Y=25), not jump to new ideal (Y=30).
        assert_eq!(
            terminal.viewport_area,
            Rect::new(0, 25, 200, 25),
            "viewport should not move when only terminal Y grew"
        );
        // No clears should have been emitted.
        assert!(
            terminal.backend().clears.is_empty(),
            "no clear should be emitted on terminal Y grow: {:?}",
            terminal.backend().clears
        );
    }

    #[test]
    fn viewport_growth_scrolls_visible_history_extent_up() {
        let mut terminal = Terminal::new(RecordingBackend::new(10, 10)).expect("terminal");
        terminal.set_viewport_area(Rect::new(0, 8, 10, 2));
        terminal.set_visible_history_extent(5, 6);
        terminal.last_known_screen_size = Size::new(10, 10);

        terminal.resize_viewport_to(4).expect("resize viewport");

        assert_eq!(terminal.viewport_area, Rect::new(0, 6, 10, 4));
        assert_eq!(terminal.visible_history_bottom(), 4);
        assert_eq!(terminal.visible_history_rows(), 4);
    }
}
