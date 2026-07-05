//! Insert finalized history lines into the terminal's **normal scrollback**,
//! above the inline viewport. Ported and trimmed from codex-rs
//! `tui/src/insert_history.rs` (Standard / DECSTBM path only).
//!
//! # The mechanism (codex's "Standard" mode)
//!
//! The inline viewport occupies the bottom `viewport_area.height` rows. To add
//! a finalized line *above* it without repainting the viewport, we:
//!
//! 1. Set a DECSTBM scroll region covering the rows above the viewport
//!    (`CSI 1 ; top r`).
//! 2. Move to the bottom of that region and emit Reverse Index (`ESC M`) to
//!    scroll the region's content down, opening blank rows — but when there is
//!    room below the viewport we instead let the region above grow.
//! 3. Print the new line(s) into the freed space.
//! 4. Reset the scroll region (`CSI r`) and restore the cursor.
//!
//! Because the printed rows land in the terminal's own grid (not ratatui's
//! double buffer), they become **real scrollback**: the user can mouse-select
//! them, scroll to them with the wheel / scrollbar, and copy them via tmux
//! copy-mode — all with no app mode key. That is the whole point.
//!
//! We pre-wrap each [`Line`] to the viewport width so a long line occupies the
//! right number of physical rows. The wrapping here is a straightforward
//! word-wrap (codex additionally keeps URLs unsplit; that refinement is
//! deferred — see the crate-level notes).

use std::io;
use std::io::Write;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::Color as CColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetColors;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::backend::Backend;
use ratatui::layout::Size;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::tui_terminal::Terminal;

/// `CSI top ; bottom r` — set the DECSTBM scroll region (1-based, inclusive).
fn set_scroll_region<W: Write>(w: &mut W, top: u16, bottom: u16) -> io::Result<()> {
    write!(w, "\x1b[{top};{bottom}r")
}

/// `CSI r` — reset the scroll region to the full screen.
fn reset_scroll_region<W: Write>(w: &mut W) -> io::Result<()> {
    write!(w, "\x1b[r")
}

/// Insert `lines` into scrollback above the inline viewport, sliding the
/// viewport down by as much as fits below it (and scrolling older history up
/// otherwise). Updates `terminal.viewport_area` so the next draw paints in the
/// viewport's new position. Cursor-position-neutral: restores the cursor to
/// where it was on entry.
pub fn insert_history_lines<B>(terminal: &mut Terminal<B>, mut lines: Vec<Line>) -> io::Result<()>
where
    B: Backend + Write,
{
    // SANITIZE FIRST, wrap after: span content is later Printed verbatim into
    // the terminal mid-DECSTBM-dance (`write_spans` does no escaping), so raw
    // control bytes in untrusted transcript content would execute there — a
    // stray `CSI r` / `CSI 2J` corrupts the screen, an OSC retitles the
    // window. Tabs additionally count width 0 in `wrap_line` but advance up to
    // 8 columns on a real terminal, breaking the one-Line-one-row invariant
    // the row math below depends on. This is the single chokepoint: every
    // scrollback write goes through this function.
    for line in &mut lines {
        sanitize_line_in_place(line);
    }

    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));
    let mut area = terminal.viewport_area;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let visible_history_rows = terminal.visible_history_rows();
    let visible_history_bottom = terminal.visible_history_bottom();
    let wrap_width = area.width.max(1) as usize;

    // Pre-wrap to the viewport width so each physical row is one grid row.
    let mut wrapped: Vec<Line> = Vec::new();
    for line in &lines {
        wrapped.extend(wrap_line(line, wrap_width));
    }
    let wrapped_rows = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);
    if wrapped_rows == 0 {
        return Ok(());
    }

    let writer = terminal.backend_mut();
    let mut should_update_area = false;

    // If there is room below the viewport, grow the region above by scrolling
    // the area between the screen top and the viewport bottom; the viewport then
    // shifts down. Otherwise the region above is already full-height and the
    // reverse-index scroll pushes the oldest rows into scrollback.
    if area.bottom() < screen_size.height {
        let scroll_amount = wrapped_rows.min(screen_size.height - area.bottom());
        let top_1based = area.top() + 1;
        set_scroll_region(writer, top_1based, screen_size.height)?;
        queue!(writer, MoveTo(0, area.top()))?;
        for _ in 0..scroll_amount {
            queue!(writer, Print("\x1bM"))?; // Reverse Index (ESC M): scroll region down.
        }
        reset_scroll_region(writer)?;
        area.y += scroll_amount;
        should_update_area = true;
    }

    // A full-screen viewport leaves NO rows above it: `CSI 1;top r` would be
    // the invalid `CSI 1;0r` (xterm reads the 0 param as "default", i.e. the
    // whole screen) and the lines would print inside the viewport, only to be
    // repainted over by the next draw — silently lost on tiny panes (#232
    // #3). Stream them through the bottom row of an explicit full-screen
    // region instead: each `\r\n` at the bottom margin scrolls one row out
    // through the top into scrollback. The trailing feeds push every printed
    // line the rest of the way out — the next draw repaints the (full-screen)
    // viewport regardless, so rows left on screen would be lost, not saved.
    if area.top() == 0 {
        let bottom = screen_size.height.max(1);
        set_scroll_region(writer, 1, bottom)?;
        queue!(writer, MoveTo(0, bottom.saturating_sub(1)))?;
        for line in &wrapped {
            write_history_line(writer, line)?;
            queue!(writer, Print("\r\n"))?;
        }
        for _ in 0..bottom.saturating_sub(1) {
            queue!(writer, Print("\r\n"))?;
        }
        reset_scroll_region(writer)?;
        queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;
        terminal.set_visible_history_extent(0, 0);
        // The scroll shifted the on-screen viewport content; force the next
        // draw to repaint it fully instead of diffing against a stale buffer.
        terminal.invalidate_viewport();
        Backend::flush(terminal.backend_mut())?;
        return Ok(());
    }

    // Limit scrolling to the rows above the viewport. The terminal may have
    // spare blank capacity between the already-inserted history and the live
    // viewport (for example after the live tail shrinks). Append into that gap
    // first; scroll only the DEFICIT — the rows the new content overflows past
    // the viewport top when appended directly below the existing history:
    //
    // * On the FIRST flush (`visible_history_rows == 0`) the extent is empty
    //   at `area.top()`, so the deficit equals the full insert and the region
    //   scrolls before printing — whatever the shell had above the viewport
    //   shifts up (into scrollback as needed) instead of being overwritten
    //   (#232 #1).
    // * On later flushes the deficit consumes blank filler above the history
    //   first, one flush at a time, so the transcript stays flush against the
    //   viewport instead of being relocated to the region top with the blank
    //   band bulk-dumped into scrollback (#232 #2, the old
    //   `history_top + to_scroll` relocation).
    set_scroll_region(writer, 1, area.top())?;
    let history_bottom = if visible_history_rows == 0 {
        area.top()
    } else {
        visible_history_bottom.min(area.top())
    };
    let history_rows = visible_history_rows.min(history_bottom);
    let history_top = history_bottom.saturating_sub(history_rows);
    let overflowing_rows = history_bottom
        .saturating_add(wrapped_rows)
        .saturating_sub(area.top());
    // Scrollable budget = the rows at or above `history_bottom` (the blank
    // band plus the history itself). Never scroll the gap BELOW the history
    // (rows between it and the viewport): those are consumed by the print
    // itself, and scrolling them would interleave stale blank rows between
    // the old and new content in scrollback.
    let index_count = overflowing_rows.min(history_bottom);
    if index_count > 0 {
        queue!(writer, MoveTo(0, area.top().saturating_sub(1)))?;
        for _ in 0..index_count {
            queue!(writer, Print("\x1bD"))?; // Index (ESC D): scroll region up.
        }
    }
    // Everything in the region shifted up by `index_count`; history rows that
    // passed the region top (beyond the blank band above them) left the screen.
    let shifted_history_bottom = history_bottom.saturating_sub(index_count);
    let shifted_history_rows = history_rows.saturating_sub(index_count.saturating_sub(history_top));
    let new_bottom = shifted_history_bottom
        .saturating_add(wrapped_rows)
        .min(area.top());
    let new_visible_history_rows = shifted_history_rows
        .saturating_add(wrapped_rows)
        .min(area.top());
    let cursor_top = new_bottom.saturating_sub(wrapped_rows.min(new_bottom));
    queue!(writer, MoveTo(0, cursor_top))?;
    for (idx, line) in wrapped.iter().enumerate() {
        if idx > 0 {
            queue!(writer, Print("\r\n"))?;
        }
        write_history_line(writer, line)?;
    }
    reset_scroll_region(writer)?;

    // Restore the cursor to where it was (history insertion is position-neutral).
    queue!(writer, MoveTo(last_cursor_pos.x, last_cursor_pos.y))?;

    if should_update_area {
        terminal.set_viewport_area(area);
    }
    terminal.set_visible_history_extent(new_visible_history_rows, new_bottom);
    // Flush these out-of-band scrollback writes now. The draw() that follows
    // only flushes the backend when the live viewport diff or the cursor
    // changed, so without this the inserted history could sit buffered and not
    // appear until some later write (codex P2). This only runs when there is new
    // history to insert, so an idle TUI still emits nothing.
    Backend::flush(terminal.backend_mut())?;
    Ok(())
}

/// Replace each span's content with a terminal-safe version: tabs expand to 4
/// spaces (fixed expansion — scrollback lines are wrapped by column, so
/// tabstop-relative expansion has no anchor here) and every other control
/// char — C0 (including `\r`, `\n`, ESC), DEL, and C1 (U+0080–U+009F, the
/// 8-bit CSI/OSC forms) — is removed. Removing the introducer defuses the
/// whole escape sequence (its params become inert printable text). Styling
/// and span boundaries are preserved. Clean spans are left unallocated.
fn sanitize_line_in_place(line: &mut Line<'_>) {
    for span in &mut line.spans {
        let content = span.content.as_ref();
        if content.chars().any(char::is_control) {
            span.content = std::borrow::Cow::Owned(sanitize_span_content(content));
        }
    }
}

/// `char::is_control` matches Unicode `Cc` — exactly C0 (U+0000–U+001F), DEL
/// (U+007F) and C1 (U+0080–U+009F).
fn sanitize_span_content(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    for ch in content.chars() {
        if ch == '\t' {
            out.push_str("    ");
        } else if !ch.is_control() {
            out.push(ch);
        }
    }
    out
}

/// Word-wrap a [`Line`] to `width` display columns, preserving per-span style.
/// Empty lines round up to one physical row.
fn wrap_line(line: &Line, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    if line.width() <= width {
        return vec![owned_line(line)];
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_width = 0usize;

    for span in &line.spans {
        // Split the span into words while keeping whitespace runs attached so
        // wrapping doesn't drop the spacing inside a styled run.
        for word in split_keep_ws(span.content.as_ref()) {
            let w = word.width();
            if cur_width + w > width && cur_width > 0 {
                out.push(finish_line(std::mem::take(&mut cur), line.style));
                cur_width = 0;
                if word.trim().is_empty() {
                    continue; // don't start a wrapped row with leading whitespace
                }
            }
            // A single word longer than the width: hard-split it.
            if w > width {
                for chunk in hard_split(word, width) {
                    let cw = chunk.width();
                    if cur_width + cw > width && cur_width > 0 {
                        out.push(finish_line(std::mem::take(&mut cur), line.style));
                        cur_width = 0;
                    }
                    cur.push(Span::styled(chunk.to_string(), span.style));
                    cur_width += cw;
                }
            } else {
                cur.push(Span::styled(word.to_string(), span.style));
                cur_width += w;
            }
        }
    }
    if !cur.is_empty() {
        out.push(finish_line(cur, line.style));
    }
    if out.is_empty() {
        out.push(Line::default().style(line.style));
    }
    out
}

fn finish_line(spans: Vec<Span<'static>>, style: ratatui::style::Style) -> Line<'static> {
    Line::from(spans).style(style)
}

fn owned_line(line: &Line) -> Line<'static> {
    let spans = line
        .spans
        .iter()
        .map(|s| Span::styled(s.content.to_string(), s.style))
        .collect::<Vec<_>>();
    Line::from(spans).style(line.style)
}

/// Split a string into words and whitespace runs (both preserved).
fn split_keep_ws(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev_ws: Option<bool> = None;
    for (i, ch) in s.char_indices() {
        let is_ws = ch.is_whitespace();
        if let Some(p) = prev_ws
            && p != is_ws
        {
            out.push(&s[start..i]);
            start = i;
        }
        prev_ws = Some(is_ws);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// Hard-split an over-long word into `width`-column chunks (char boundaries).
fn hard_split(word: &str, width: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut col = 0usize;
    for (i, ch) in word.char_indices() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + cw > width && i > start {
            out.push(&word[start..i]);
            start = i;
            col = 0;
        }
        col += cw;
    }
    if start < word.len() {
        out.push(&word[start..]);
    }
    if out.is_empty() {
        out.push(word);
    }
    out
}

/// Write a single (already-wrapped) history line: set colors, clear to EOL,
/// then write each styled span. Caller positions the cursor / emits `\r\n`.
fn write_history_line<W: Write>(writer: &mut W, line: &Line) -> io::Result<()> {
    queue!(
        writer,
        SetColors(Colors::new(
            line.style.fg.map(Into::into).unwrap_or(CColor::Reset),
            line.style.bg.map(Into::into).unwrap_or(CColor::Reset),
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;
    // Merge the line-level style into each span so ANSI colors reflect it.
    let merged: Vec<Span> = line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged.iter())
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            queue_modifier_diff(&mut writer, last_modifier, modifier)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(next_fg.into(), next_bg.into()))
            )?;
            fg = next_fg;
            bg = next_bg;
        }
        queue!(writer, Print(span.content.clone()))?;
    }
    // Emit explicit fg/bg/attribute resets instead of crossterm's compact
    // `CSI m` form so history insertion always leaves a stable reset fence.
    write!(writer, "\x1b[39m\x1b[49m\x1b[0m")
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui_terminal::FrameLike;
    use ratatui::backend::{Backend, ClearType as RtClearType, WindowSize};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::Style;

    /// A `Backend + Write` that records every byte emitted, so tests can assert on
    /// the exact escape-sequence stream `insert_history_lines` writes into the
    /// terminal's real scrollback (codex's tests use a VT100 backend for this; the
    /// octos-tui crate has no vt100 dep, so we inspect the raw bytes instead).
    struct RecordingBackend {
        buf: Vec<u8>,
        size: Size,
    }

    impl RecordingBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                buf: Vec::new(),
                size: Size::new(width, height),
            }
        }

        fn output(&self) -> String {
            String::from_utf8_lossy(&self.buf).into_owned()
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
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(Position { x: 0, y: 0 })
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, _position: P) -> io::Result<()> {
            Ok(())
        }
        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clear_region(&mut self, _clear_type: RtClearType) -> io::Result<()> {
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

    fn term(width: u16, height: u16) -> Terminal<RecordingBackend> {
        let mut t = Terminal::new(RecordingBackend::new(width, height)).expect("terminal");
        // Anchor a 1-row viewport at the bottom so history inserts scroll upward.
        t.set_viewport_area(Rect::new(0, height - 1, width, 1));
        t
    }

    #[derive(Debug)]
    struct ScreenBackend {
        buf: Vec<u8>,
        size: Size,
        cursor: Position,
        margin_top: u16,
        margin_bottom: u16,
        rows: Vec<Vec<char>>,
        scrollback: Vec<String>,
        pending: Vec<u8>,
    }

    impl ScreenBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                buf: Vec::new(),
                size: Size::new(width, height),
                cursor: Position { x: 0, y: 0 },
                margin_top: 0,
                margin_bottom: height.saturating_sub(1),
                rows: vec![vec![' '; usize::from(width)]; usize::from(height)],
                scrollback: Vec::new(),
                pending: Vec::new(),
            }
        }

        fn screen_rows_above(&self, viewport_top: u16) -> Vec<String> {
            let mut rows = self.scrollback.clone();
            rows.extend(
                self.rows
                    .iter()
                    .take(usize::from(viewport_top))
                    .map(|row| row.iter().collect::<String>().trim_end().to_string()),
            );
            trim_transcript_rows(rows)
        }

        fn write_byte(&mut self, byte: u8) {
            match byte {
                b'\r' => self.cursor.x = 0,
                b'\n' => self.linefeed(),
                0x20..=0x7e => self.write_printable(char::from(byte)),
                _ => {}
            }
        }

        fn write_printable(&mut self, ch: char) {
            if self.cursor.y >= self.size.height {
                return;
            }
            if self.cursor.x >= self.size.width {
                self.cursor.x = 0;
                self.linefeed();
            }
            if let Some(row) = self.rows.get_mut(usize::from(self.cursor.y))
                && let Some(cell) = row.get_mut(usize::from(self.cursor.x))
            {
                *cell = ch;
            }
            self.cursor.x = self.cursor.x.saturating_add(1);
        }

        fn linefeed(&mut self) {
            if self.cursor.y == self.margin_bottom {
                self.scroll_region_up();
            } else {
                self.cursor.y = (self.cursor.y + 1).min(self.size.height.saturating_sub(1));
            }
        }

        fn reverse_index(&mut self) {
            if self.cursor.y == self.margin_top {
                self.scroll_region_down();
            } else {
                self.cursor.y = self.cursor.y.saturating_sub(1);
            }
        }

        fn scroll_region_up(&mut self) {
            let top = usize::from(self.margin_top);
            let bottom = usize::from(self.margin_bottom);
            if top >= self.rows.len() || bottom >= self.rows.len() || top > bottom {
                return;
            }
            if self.margin_top == 0 {
                self.scrollback.push(
                    self.rows[top]
                        .iter()
                        .collect::<String>()
                        .trim_end()
                        .to_string(),
                );
            }
            for row in top..bottom {
                self.rows[row] = self.rows[row + 1].clone();
            }
            self.rows[bottom] = vec![' '; usize::from(self.size.width)];
        }

        fn scroll_region_down(&mut self) {
            let top = usize::from(self.margin_top);
            let bottom = usize::from(self.margin_bottom);
            if top >= self.rows.len() || bottom >= self.rows.len() || top > bottom {
                return;
            }
            for row in (top + 1..=bottom).rev() {
                self.rows[row] = self.rows[row - 1].clone();
            }
            self.rows[top] = vec![' '; usize::from(self.size.width)];
        }

        fn clear_to_eol(&mut self) {
            if let Some(row) = self.rows.get_mut(usize::from(self.cursor.y)) {
                for cell in row.iter_mut().skip(usize::from(self.cursor.x)) {
                    *cell = ' ';
                }
            }
        }

        fn handle_csi(&mut self, params: &str, command: u8) {
            match command {
                b'H' | b'f' => {
                    let parsed = parse_csi_numbers(params);
                    let row = parsed.first().copied().unwrap_or(1).saturating_sub(1);
                    let col = parsed.get(1).copied().unwrap_or(1).saturating_sub(1);
                    self.cursor = Position {
                        x: col.min(self.size.width.saturating_sub(1)),
                        y: row.min(self.size.height.saturating_sub(1)),
                    };
                }
                b'K' => self.clear_to_eol(),
                b'r' => {
                    let parsed = parse_csi_numbers(params);
                    if parsed.len() >= 2 {
                        let top = parsed[0].max(1).saturating_sub(1);
                        let bottom = parsed[1].max(1).min(self.size.height).saturating_sub(1);
                        if top <= bottom {
                            self.margin_top = top;
                            self.margin_bottom = bottom;
                        }
                    } else {
                        self.margin_top = 0;
                        self.margin_bottom = self.size.height.saturating_sub(1);
                    }
                }
                b'm' => {}
                _ => {}
            }
        }

        fn process_pending(&mut self) {
            let mut consumed = 0;
            while consumed < self.pending.len() {
                if self.pending[consumed] == 0x1b {
                    match self.pending.get(consumed + 1).copied() {
                        None => break,
                        Some(b'[') => {
                            let start = consumed + 2;
                            let mut end = start;
                            while end < self.pending.len()
                                && !(0x40..=0x7e).contains(&self.pending[end])
                            {
                                end += 1;
                            }
                            if end >= self.pending.len() {
                                break;
                            }
                            let params =
                                String::from_utf8_lossy(&self.pending[start..end]).into_owned();
                            let command = self.pending[end];
                            self.handle_csi(&params, command);
                            consumed = end + 1;
                            continue;
                        }
                        Some(b'M') => {
                            self.reverse_index();
                            consumed += 2;
                            continue;
                        }
                        Some(b'D') => {
                            self.linefeed();
                            consumed += 2;
                            continue;
                        }
                        Some(_) => {}
                    }
                }
                self.write_byte(self.pending[consumed]);
                consumed += 1;
            }
            if consumed > 0 {
                self.pending.drain(..consumed);
            }
        }
    }

    fn parse_csi_numbers(params: &str) -> Vec<u16> {
        params
            .trim_start_matches('?')
            .split(';')
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse::<u16>().ok())
            .collect()
    }

    fn trim_transcript_rows(mut rows: Vec<String>) -> Vec<String> {
        let first_content = rows.iter().position(|row| !row.is_empty()).unwrap_or(0);
        rows.drain(..first_content);
        if let Some(last_content) = rows.iter().rposition(|row| !row.is_empty()) {
            rows.truncate((last_content + 2).min(rows.len()));
        }
        rows
    }

    impl Write for ScreenBackend {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(data);
            self.pending.extend_from_slice(data);
            self.process_pending();
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for ScreenBackend {
        fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            for (x, y, cell) in content {
                if y >= self.size.height || x >= self.size.width {
                    continue;
                }
                let symbol = cell.symbol();
                let row = &mut self.rows[usize::from(y)];
                let mut col = usize::from(x);
                if symbol.is_empty() {
                    if let Some(target) = row.get_mut(col) {
                        *target = ' ';
                    }
                    continue;
                }
                for ch in symbol.chars() {
                    if col >= row.len() {
                        break;
                    }
                    row[col] = ch;
                    col += 1;
                }
            }
            Ok(())
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor)
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor = position.into();
            Ok(())
        }
        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clear_region(&mut self, _clear_type: RtClearType) -> io::Result<()> {
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

    fn screen_term(width: u16, height: u16, viewport: Rect) -> Terminal<ScreenBackend> {
        let mut t = Terminal::new(ScreenBackend::new(width, height)).expect("terminal");
        t.set_viewport_area(viewport);
        t
    }

    fn text_line(text: &str) -> Line<'static> {
        Line::from(text.to_string())
    }

    fn blank_line() -> Line<'static> {
        Line::from("")
    }

    fn seed_history_then_move_viewport(
        width: u16,
        height: u16,
        final_viewport: Rect,
    ) -> Terminal<ScreenBackend> {
        let seed_top = final_viewport.y.saturating_sub(1).max(1);
        let seed_viewport = Rect::new(0, seed_top, width, height - seed_top);
        let mut term = screen_term(width, height, seed_viewport);
        insert_history_lines(&mut term, vec![text_line("old")]).expect("seed old history");
        term.set_viewport_area(final_viewport);
        term
    }

    fn draw_live_tail(term: &mut Terminal<ScreenBackend>, label: &str) {
        term.invalidate_viewport();
        term.draw(|frame| {
            use ratatui::widgets::Paragraph;

            frame.render_widget(Paragraph::new(format!("\n{label}")), frame.area());
        })
        .expect("draw live tail");
    }

    /// Explicit default-background reset (`CSI 49 m`). Any background SGR that
    /// is NOT this is a non-default (theme) background.
    fn default_bg_seq() -> &'static str {
        "\x1b[49m"
    }

    #[test]
    fn history_line_with_theme_bg_is_emitted_on_default_background() {
        // Bug 3 (b): a finalized line that *carried* a theme surface background
        // would emit `SetBackgroundColor(Rgb(..))` and a `Clear(UntilNewLine)`
        // under it, painting a "brown block" that bleeds to the row's right edge.
        // After the fix, finalized lines have no bg (Bug 3a strips it), so the
        // only background SGR in the scrollback stream is the default reset.
        let mut t = term(40, 6);
        // Mirror a stripped finalized line: fg set, NO bg (default background).
        let line = Line::from(vec![Span::styled(
            "committed reply",
            Style::default().fg(Color::Rgb(236, 239, 244)),
        )]);
        insert_history_lines(&mut t, vec![line]).expect("insert history");

        let out = t.backend().output();
        let default_bg = default_bg_seq();
        // Any non-default background SGR in the stream means a theme surface
        // leaked into scrollback; only the default reset (`CSI 49 m`) is allowed.
        assert!(
            !out.contains("\x1b[48;2;"),
            "scrollback stream emitted a truecolor background (brown block): {out:?}"
        );
        assert!(
            !out.contains("\x1b[48;5;"),
            "scrollback stream emitted an indexed background (brown block): {out:?}"
        );
        assert!(
            out.contains(default_bg),
            "scrollback stream should reset the background to default: {out:?}"
        );
    }

    #[test]
    fn history_write_resets_sgr_after_each_line() {
        // Bug 3 (b): un-reset SGR would bleed the last line's colors into the
        // scroll-region ops / subsequent prints. Every history write must end on
        // a full reset (fg + bg + attributes) so nothing bleeds "all over".
        let mut t = term(40, 6);
        let line = Line::from(vec![Span::styled(
            "styled",
            Style::default()
                .fg(Color::Rgb(110, 188, 255))
                .add_modifier(Modifier::BOLD),
        )]);
        insert_history_lines(&mut t, vec![line]).expect("insert history");

        let out = t.backend().output();
        // crossterm emits `SetAttribute(Reset)` as `CSI 0 m`.
        assert!(
            out.contains("\x1b[0m"),
            "expected an SGR reset (CSI 0 m) in the scrollback stream: {out:?}"
        );
        // The reset trio must appear; the foreground reset is `CSI 39 m`.
        assert!(
            out.contains("\x1b[39m"),
            "expected a foreground reset (CSI 39 m) in the scrollback stream: {out:?}"
        );
    }

    #[test]
    fn consecutive_blank_terminated_inserts_match_one_combined_insert() {
        for height in 3..=8 {
            for viewport_top in 1..height {
                let width = 24;
                let viewport = Rect::new(0, viewport_top, width, height - viewport_top);

                let mut split = seed_history_then_move_viewport(width, height, viewport);
                insert_history_lines(&mut split, vec![text_line("first"), blank_line()])
                    .expect("first insert history");
                insert_history_lines(&mut split, vec![text_line("second"), blank_line()])
                    .expect("second insert history");
                let split_rows = split.backend().screen_rows_above(split.viewport_area.top());

                let mut combined = seed_history_then_move_viewport(width, height, viewport);
                insert_history_lines(
                    &mut combined,
                    vec![
                        text_line("first"),
                        blank_line(),
                        text_line("second"),
                        blank_line(),
                    ],
                )
                .expect("combined insert history");
                let combined_rows = combined
                    .backend()
                    .screen_rows_above(combined.viewport_area.top());

                assert_eq!(
                    split_rows,
                    vec!["old", "first", "", "second", ""],
                    "split inserts should keep exactly one blank separator: height={height} viewport_top={viewport_top}"
                );
                assert_eq!(
                    split_rows, combined_rows,
                    "chunked history insertion must be row-equivalent to one combined insertion: height={height} viewport_top={viewport_top}"
                );
            }
        }
    }

    #[test]
    fn live_tail_redraw_between_blank_terminated_inserts_does_not_leave_blank_row() {
        let width = 24;
        let height = 8;
        let viewport = Rect::new(0, 3, width, 2);
        let mut split = screen_term(width, height, viewport);

        draw_live_tail(&mut split, "live one");
        insert_history_lines(&mut split, vec![text_line("first"), blank_line()])
            .expect("first insert history");
        draw_live_tail(&mut split, "live two");
        insert_history_lines(&mut split, vec![text_line("second"), blank_line()])
            .expect("second insert history");

        let split_rows = split.backend().screen_rows_above(split.viewport_area.top());
        assert_eq!(
            split_rows,
            vec!["first", "", "second", ""],
            "redrawing a leading-blank live tail between inserts must not add a second blank"
        );
    }

    #[test]
    fn insert_history_strips_raw_escapes_and_controls_from_span_content() {
        // Fix #5: span content is Printed verbatim into the terminal mid-
        // DECSTBM dance; raw ESC/CSI/OSC or C1 bytes in untrusted transcript
        // content (tool output, `cat` of a binary, model-echoed escapes) would
        // corrupt the screen / retitle the window. Every control char must be
        // removed at the chokepoint (the introducer gone, the sequence is
        // inert text).
        let mut t = term(40, 8);
        let line = Line::from(vec![Span::raw(
            "a\u{1b}[31mred\u{1b}]0;evil\u{7}b\rc\u{9b}2Jd\u{7f}e",
        )]);
        insert_history_lines(&mut t, vec![line]).expect("insert history");

        let out = t.backend().output();
        assert!(!out.contains("\x1b[31m"), "raw CSI color leaked: {out:?}");
        assert!(!out.contains("\x1b[2J"), "raw CSI clear leaked: {out:?}");
        assert!(!out.contains("\x1b]"), "raw OSC leaked: {out:?}");
        assert!(!out.contains('\u{9b}'), "raw C1 CSI leaked: {out:?}");
        assert!(!out.contains('\u{7}'), "raw BEL leaked: {out:?}");
        assert!(!out.contains('\u{7f}'), "raw DEL leaked: {out:?}");
        // The printable payload survives with the introducers/controls gone
        // (defused sequence params render as inert text).
        assert!(
            out.contains("a[31mred]0;evilbc2Jde"),
            "sanitized payload missing: {out:?}"
        );
    }

    #[test]
    fn insert_history_expands_tabs_so_wrapping_matches_terminal_columns() {
        // Fix #5 (tab drift): `wrap_line` counted '\t' as width 0 while a real
        // terminal advances up to 8 columns, breaking the one-Line-one-row
        // invariant the DECSTBM row math depends on (#220 zone). Tabs expand to
        // 4 spaces BEFORE wrapping, so "aaaa\tbbbb" (12 expanded columns) must
        // occupy two physical rows at width 8 — not one.
        let width = 8;
        let height = 8;
        let viewport = Rect::new(0, 4, width, 2);
        let mut t = screen_term(width, height, viewport);

        insert_history_lines(&mut t, vec![text_line("aaaa\tbbbb")]).expect("insert history");

        let rows = t.backend().screen_rows_above(t.viewport_area.top());
        assert_eq!(
            rows,
            vec!["aaaa", "bbbb"],
            "tab-expanded content must wrap into two physical rows"
        );

        // No wrap needed: the tab still renders as spaces (single row).
        let mut t = screen_term(12, height, Rect::new(0, 4, 12, 2));
        insert_history_lines(&mut t, vec![text_line("a\tb")]).expect("insert history");
        let rows = t.backend().screen_rows_above(t.viewport_area.top());
        assert_eq!(rows, vec!["a    b"]);
    }

    #[test]
    fn insert_history_row_count_saturates_instead_of_wrapping_at_u16_max() {
        // Fix #6: `wrapped.len() as u16` wrapped modulo 65536, so 65_537
        // one-row lines truncated to 1 and the viewport slid by a single row
        // even with 2 spare rows below it. Saturating keeps the full (clamped)
        // row count in play.
        let mut t = Terminal::new(RecordingBackend::new(10, 8)).expect("terminal");
        t.set_viewport_area(Rect::new(0, 5, 10, 1)); // 2 spare rows below

        let lines: Vec<Line> = (0..=usize::from(u16::MAX))
            .map(|_| Line::from("x"))
            .collect();
        insert_history_lines(&mut t, lines).expect("insert history");

        assert_eq!(
            t.viewport_area.y, 7,
            "viewport must slide by the full spare gap (2 rows), not by len % 65536"
        );
    }

    /// Write `text` directly into the emulated screen row (bypassing the
    /// escape-sequence path) — simulates content that was on the terminal
    /// BEFORE the TUI started tracking it (prior shell output at launch).
    fn seed_screen_row(term: &mut Terminal<ScreenBackend>, row: u16, text: &str) {
        let backend = term.backend_mut();
        let cells = &mut backend.rows[usize::from(row)];
        for (idx, ch) in text.chars().enumerate() {
            if idx >= cells.len() {
                break;
            }
            cells[idx] = ch;
        }
    }

    /// P2 (tri-repo #246 ⊃ #232 #1): the FIRST flush of a session
    /// (`visible_history_rows == 0`) treated the whole region above the
    /// viewport as a printable gap and wrote upward from `area.top()` with no
    /// scroll — clobbering whatever the user's shell had there. The first
    /// flush must scroll the pre-existing rows up (into scrollback if needed)
    /// and print into the freed rows.
    #[test]
    fn first_flush_preserves_preexisting_shell_rows() {
        let width = 24;
        let height = 8;
        // Viewport occupies the bottom 3 rows; rows 0..5 belong to the shell.
        let mut term = screen_term(width, height, Rect::new(0, 5, width, 3));
        seed_screen_row(&mut term, 3, "shell-one");
        seed_screen_row(&mut term, 4, "shell-two");

        insert_history_lines(&mut term, vec![text_line("tui-1"), text_line("tui-2")])
            .expect("first flush");

        let rows = term.backend().screen_rows_above(term.viewport_area.top());
        assert_eq!(
            rows,
            vec!["shell-one", "shell-two", "tui-1", "tui-2"],
            "the first flush must scroll prior shell output up, not overwrite it"
        );
    }

    /// P2 (tri-repo #246 ⊃ #232 #2): when a flush overflowed the space above
    /// the viewport, `index_count = history_top + to_scroll` relocated the
    /// retained history to the region TOP — bulk-dumping the blank band into
    /// scrollback and detaching the transcript from the viewport (content at
    /// the screen top, blank rows between it and the live area). The scroll
    /// must instead cover exactly the deficit: history stays flush against
    /// the viewport and nothing readable is pushed out while blank capacity
    /// remains above.
    #[test]
    fn overflow_flush_keeps_history_flush_against_the_viewport() {
        let width = 24;
        let height = 10;
        // Viewport on the bottom 4 rows; region above is rows 0..6.
        let mut term = screen_term(width, height, Rect::new(0, 6, width, 4));

        insert_history_lines(&mut term, vec![text_line("one"), text_line("two")])
            .expect("first flush");
        insert_history_lines(
            &mut term,
            vec![
                text_line("three"),
                text_line("four"),
                text_line("five"),
                text_line("six"),
                text_line("seven"),
            ],
        )
        .expect("overflow flush");

        // All seven rows are readable in order (scrollback + screen)...
        let rows = term.backend().screen_rows_above(term.viewport_area.top());
        assert_eq!(
            rows,
            vec!["one", "two", "three", "four", "five", "six", "seven"],
            "the transcript stays contiguous across an overflow flush"
        );
        // ...the newest row sits FLUSH against the viewport (no detach)...
        let screen_last = term.backend().rows[usize::from(term.viewport_area.top()) - 1]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string();
        assert_eq!(
            screen_last, "seven",
            "the newest history row must sit directly above the viewport"
        );
        // ...and nothing readable entered scrollback beyond the true deficit
        // (7 content rows - 6 region rows = 1): "one" scrolls out, "two"
        // stays on screen.
        let readable_in_scrollback: Vec<&String> = term
            .backend()
            .scrollback
            .iter()
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(
            readable_in_scrollback,
            vec!["one"],
            "only the true overflow leaves the screen; the old code relocated everything"
        );
    }

    /// P2 (tri-repo #246 ⊃ #232 #3): with the viewport covering the whole
    /// screen (`area.top() == 0`, tiny pane), the old code emitted the
    /// INVALID `CSI 1;0r` (xterm reads param 0 as "default" = full screen)
    /// and printed the lines inside the viewport, where the next draw
    /// repainted over them — content silently lost. The lines must instead be
    /// streamed through the bottom of a full-screen scroll region so they
    /// land in scrollback.
    #[test]
    fn full_screen_viewport_streams_lines_into_scrollback() {
        let width = 24;
        let height = 4;
        let mut term = screen_term(width, height, Rect::new(0, 0, width, height));

        insert_history_lines(&mut term, vec![text_line("alpha"), text_line("beta")])
            .expect("full-screen flush");

        assert!(
            !String::from_utf8_lossy(&term.backend().buf).contains("\x1b[1;0r"),
            "must not emit the invalid CSI 1;0r scroll region"
        );
        let readable: Vec<&String> = term
            .backend()
            .scrollback
            .iter()
            .filter(|row| !row.is_empty())
            .collect();
        assert_eq!(
            readable,
            vec!["alpha", "beta"],
            "flushed lines must reach scrollback, not be repainted over inside the viewport"
        );
    }

    #[test]
    fn wrap_short_line_is_unchanged() {
        let line = Line::from("hello");
        let out = wrap_line(&line, 20);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].width(), 5);
    }

    #[test]
    fn wrap_splits_on_word_boundary() {
        let line = Line::from("the quick brown fox jumps");
        let out = wrap_line(&line, 10);
        assert!(out.len() >= 3, "expected wrapping, got {out:?}");
        for l in &out {
            assert!(l.width() <= 10, "row too wide: {l:?}");
        }
    }

    #[test]
    fn wrap_hard_splits_overlong_word() {
        let line = Line::from("abcdefghijklmnopqrstuvwxyz");
        let out = wrap_line(&line, 8);
        assert!(out.len() >= 3);
        for l in &out {
            assert!(l.width() <= 8);
        }
    }

    #[test]
    fn wrap_preserves_span_style() {
        use ratatui::style::Stylize;
        let line = Line::from(vec!["bold ".bold(), "and plain text here".into()]);
        let out = wrap_line(&line, 8);
        assert!(out.len() >= 2);
        // The first span's bold style must survive on the first row.
        let first_has_bold = out[0]
            .spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(first_has_bold, "bold style lost: {out:?}");
    }

    #[test]
    fn split_keep_ws_round_trips() {
        let s = "a  bc   d";
        let parts = split_keep_ws(s);
        assert_eq!(parts.concat(), s);
    }
}
