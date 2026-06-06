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
    }

    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// Pin the inline viewport to `height` rows at the bottom of the screen,
    /// scrolling existing content up if the viewport would run off the bottom.
    /// This is the per-frame analogue of codex's `update_inline_viewport`.
    pub fn resize_viewport_to(&mut self, height: u16) -> io::Result<()> {
        let size = self.backend.size()?;
        let mut area = self.viewport_area;
        area.width = size.width;
        area.height = height.min(size.height).max(1);
        if area.bottom() > size.height {
            let scroll_by = area.bottom() - size.height;
            // Push the rows above the viewport up into scrollback so the
            // viewport fits at the bottom, using a DECSTBM scroll region over
            // the rows above the (old) viewport bottom + Index (`ESC D`). We emit
            // the escapes directly so we don't depend on ratatui's optional
            // `scrolling-regions` Backend feature.
            scroll_region_up(&mut self.backend, area.top(), scroll_by)?;
        }
        // Always re-pin the viewport to the bottom `height` rows. When the live
        // UI SHRINKS (a turn completes, a menu closes) the old bottom no longer
        // overflows the screen, so without this the smaller viewport would
        // repaint at the old (higher) top and leave blank rows below the
        // composer until later history shifted it down (codex P2). `clear()`
        // below clears from the OLD viewport top to end-of-screen, so the rows
        // the viewport vacates are blanked rather than left stale.
        area.y = size.height - area.height;
        if area != self.viewport_area {
            self.clear()?;
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

        self.flush()?;

        match cursor_position {
            None => self.hide_cursor()?,
            Some(position) => {
                self.show_cursor()?;
                self.set_cursor_position(position)?;
            }
        }

        self.swap_buffers();
        Backend::flush(&mut self.backend)?;
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

    /// Diff the current vs previous buffer and emit only the changes.
    fn flush(&mut self) -> io::Result<()> {
        let updates = diff_buffers(self.previous_buffer(), &self.buffers[self.current]);
        if let Some(&(x, y, _)) = updates.last() {
            self.last_known_cursor_pos = Position { x, y };
        }
        draw(&mut self.backend, updates.into_iter())
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
        self.backend
            .set_cursor_position(self.viewport_area.as_position())?;
        self.backend.clear_region(ClearType::AfterCursor)?;
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
