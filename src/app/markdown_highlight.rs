//! Style-only markdown highlighting for the composer draft.
//!
//! The composer is an editable input, so the highlighter NEVER changes the
//! character stream: it only assigns `Style`s to slices of the exact same
//! text. Content identity is the load-bearing invariant — the concatenation
//! of the returned span contents is always byte-identical to the input line —
//! so the wrap math (`wrap_composer_line`) and the cursor math
//! (`composer_input_view` / `composer_cursor_position`) keep operating on the
//! same characters they always did. This is syntax highlighting, not markdown
//! layout: markers (`#`, `` ` ``, `**`, …) stay visible and keep their
//! columns, nothing is hidden, inserted, or reflowed.
//!
//! Rules are line-based plus per-line inline scans (no lookahead past the
//! line), cheap enough to run every frame on drafts a few KB long:
//! - `#`–`######` headings → whole line bold in the title color.
//! - `` `code` `` → highlight color, backticks included.
//! - `**bold**` / `__bold__` → BOLD, `*italic*` / `_italic_` → ITALIC
//!   (markers included; `_` never opens/closes intraword, so `snake_case`
//!   stays plain).
//! - ``` fences toggle a caller-owned state; fence and interior lines render
//!   muted (no inline parsing inside a fence).
//! - `-`/`*`/`+`/`1.` list markers → accent on the marker only.
//! - `>` blockquote lines → muted italic.
//!
//! Unmatched or whitespace-adjacent markers stay plain.

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Palette;

/// True when `line` is a fence delimiter (```` ``` ````, optionally indented,
/// info string allowed). The caller flips its fence state on such lines.
pub(super) fn is_fence_line(line: &str) -> bool {
    line.trim_start().starts_with("```")
}

/// Highlight one logical composer line. `in_fence` carries fenced-code-block
/// state across lines (toggled by fence delimiter lines). The concatenated
/// contents of the returned spans are exactly `line`; only styles vary. An
/// empty line yields no spans.
pub(super) fn markdown_highlight_line(
    line: &str,
    in_fence: &mut bool,
    palette: Palette,
) -> Vec<Span<'static>> {
    if line.is_empty() {
        return Vec::new();
    }
    let base = palette.text().bg(palette.surface);
    let muted = palette.muted().bg(palette.surface);
    if is_fence_line(line) {
        *in_fence = !*in_fence;
        return vec![Span::styled(line.to_string(), muted)];
    }
    if *in_fence {
        return vec![Span::styled(line.to_string(), muted)];
    }
    if is_heading_line(line) {
        return vec![Span::styled(
            line.to_string(),
            palette
                .title()
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        )];
    }
    if line.trim_start().starts_with('>') {
        return vec![Span::styled(
            line.to_string(),
            muted.add_modifier(Modifier::ITALIC),
        )];
    }

    let mut spans = Vec::new();
    let mut rest = line;
    if let Some((indent_end, marker_end)) = bullet_marker_bounds(line) {
        if indent_end > 0 {
            spans.push(Span::styled(line[..indent_end].to_string(), base));
        }
        spans.push(Span::styled(
            line[indent_end..marker_end].to_string(),
            Style::default().fg(palette.accent).bg(palette.surface),
        ));
        rest = &line[marker_end..];
    }
    inline_spans(rest, palette, base, &mut spans);
    spans
}

/// Re-slice one logical line's highlighted spans into the visual row chunks
/// produced by `wrap_composer_line`. The emitted text comes from `chunks`
/// verbatim — the spans contribute STYLE only — so each row's content is
/// byte-identical to the unstyled render no matter what the highlighter did.
/// Should the span stream ever disagree with the chunk text (defensive; it
/// cannot when the spans concatenate to the full line), the remainder falls
/// back to `base` rather than altering a single character.
pub(super) fn split_highlighted_spans(
    spans: &[Span<'static>],
    chunks: &[String],
    base: Style,
) -> Vec<Vec<Span<'static>>> {
    let mut rows = Vec::with_capacity(chunks.len());
    let mut span_index = 0usize;
    let mut span_offset = 0usize;
    for chunk in chunks {
        let mut row = Vec::new();
        let mut taken = 0usize;
        while taken < chunk.len() && span_index < spans.len() {
            let span = &spans[span_index];
            let available = span.content.len().saturating_sub(span_offset);
            if available == 0 {
                span_index += 1;
                span_offset = 0;
                continue;
            }
            let take = available.min(chunk.len() - taken);
            let Some(slice) = chunk.get(taken..taken + take) else {
                // Split point is not a char boundary — spans disagree with the
                // chunk text. Bail to the plain fallback below.
                break;
            };
            row.push(Span::styled(slice.to_string(), span.style));
            taken += take;
            span_offset += take;
            if span_offset >= span.content.len() {
                span_index += 1;
                span_offset = 0;
            }
        }
        if taken < chunk.len() {
            row.push(Span::styled(chunk[taken..].to_string(), base));
        }
        rows.push(row);
    }
    rows
}

/// `#`–`######` then a space (or end of line). Leading whitespace disqualifies
/// (headings start the line), as does a 7th `#` or `#hashtag`-style text.
fn is_heading_line(line: &str) -> bool {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    (1..=6).contains(&hashes) && line[hashes..].chars().next().is_none_or(|c| c == ' ')
}

/// `(indent_end, marker_end)` byte bounds of a list marker: optional
/// indentation, then `-`/`*`/`+` or `1.`-style digits, followed by a space.
/// The space stays outside the marker (it renders plain).
fn bullet_marker_bounds(line: &str) -> Option<(usize, usize)> {
    let indent_end = line.len() - line.trim_start().len();
    let rest = &line[indent_end..];
    let first = rest.chars().next()?;
    if matches!(first, '-' | '*' | '+') {
        return rest[1..]
            .starts_with(' ')
            .then_some((indent_end, indent_end + 1));
    }
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if (1..=9).contains(&digits) && rest[digits..].starts_with(". ") {
        return Some((indent_end, indent_end + digits + 1));
    }
    None
}

/// Scan `text` (one line, or the remainder after a list marker) left to right
/// for inline code and emphasis, pushing styled spans onto `out`. All indexes
/// come from `char_indices`/`len_utf8`, so every slice boundary is a char
/// boundary — CJK/emoji safe. Whatever matches nothing is emitted with `base`.
fn inline_spans(text: &str, palette: Palette, base: Style, out: &mut Vec<Span<'static>>) {
    let code = Style::default().fg(palette.highlight).bg(palette.surface);
    let bold = base.add_modifier(Modifier::BOLD);
    let italic = base.add_modifier(Modifier::ITALIC);

    let mut plain_start = 0usize;
    let mut index = 0usize;
    while let Some(ch) = text[index..].chars().next() {
        let hit = match ch {
            '`' => find_code_close(text, index).map(|end| (end, code)),
            '*' | '_' => find_emphasis_close(text, index, ch)
                .map(|(end, is_bold)| (end, if is_bold { bold } else { italic })),
            _ => None,
        };
        match hit {
            Some((end, style)) => {
                if plain_start < index {
                    out.push(Span::styled(text[plain_start..index].to_string(), base));
                }
                out.push(Span::styled(text[index..end].to_string(), style));
                index = end;
                plain_start = end;
            }
            None => index += ch.len_utf8(),
        }
    }
    if plain_start < text.len() {
        out.push(Span::styled(text[plain_start..].to_string(), base));
    }
}

/// End (exclusive) of an inline code span opened by the backtick at `open`,
/// i.e. one past the closing backtick. `None` leaves the backtick plain.
fn find_code_close(text: &str, open: usize) -> Option<usize> {
    text[open + 1..].find('`').map(|rel| open + 1 + rel + 1)
}

/// End (exclusive, including the closing marker) of an emphasis run opened at
/// `open` by `marker` (`*` or `_`), plus whether it is the doubled (bold)
/// form. Guards keep prose plain: the opener must touch non-whitespace, the
/// closer must follow non-whitespace, and `_` never opens after or closes
/// before an alphanumeric char (`snake_case` stays unstyled). `None` when
/// unmatched — the marker renders as plain text.
fn find_emphasis_close(text: &str, open: usize, marker: char) -> Option<(usize, bool)> {
    let double = text[open + 1..].starts_with(marker);
    let marker_len = if double { 2 } else { 1 };
    let content_start = open + marker_len;
    let first = text[content_start..].chars().next()?;
    if first.is_whitespace() || (!double && first == marker) {
        return None;
    }
    if marker == '_'
        && text[..open]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
    {
        return None;
    }

    let content = &text[content_start..];
    let mut previous: Option<char> = None;
    for (rel, ch) in content.char_indices() {
        let candidate = ch == marker
            && (!double || content[rel + 1..].starts_with(marker))
            && previous.is_some_and(|p| !p.is_whitespace() && p != marker);
        if candidate {
            let close_end = rel + marker_len;
            let intraword_close = marker == '_'
                && content[close_end..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphanumeric);
            if !intraword_close {
                return Some((content_start + close_end, double));
            }
        }
        previous = Some(ch);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::wrap_composer_line;
    use super::*;
    use crate::cli::ThemeName;

    fn palette() -> Palette {
        Palette::for_theme(ThemeName::Codex)
    }

    fn concat(spans: &[Span<'static>]) -> String {
        spans.iter().map(|span| span.content.as_ref()).collect()
    }

    fn highlight(line: &str) -> Vec<Span<'static>> {
        let mut fence = false;
        markdown_highlight_line(line, &mut fence, palette())
    }

    #[test]
    fn heading_lines_render_bold_in_title_color_and_keep_their_text() {
        let palette = palette();
        for line in ["# hi", "###### deep"] {
            let spans = highlight(line);
            assert_eq!(concat(&spans), line, "content identity for {line:?}");
            for span in &spans {
                assert_eq!(span.style.fg, Some(palette.accent), "title color {line:?}");
                assert!(
                    span.style.add_modifier.contains(Modifier::BOLD),
                    "bold {line:?}"
                );
            }
        }
        // Seven hashes and `#hashtag` are not headings.
        for line in ["####### seven", "#hashtag"] {
            let spans = highlight(line);
            assert_eq!(concat(&spans), line);
            assert!(
                spans
                    .iter()
                    .all(|span| !span.style.add_modifier.contains(Modifier::BOLD)),
                "{line:?} must stay plain"
            );
        }
    }

    #[test]
    fn inline_code_is_highlight_colored_with_char_safe_boundaries() {
        let palette = palette();
        let line = "前缀 `代码` 后缀";
        let spans = highlight(line);
        assert_eq!(concat(&spans), line);
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>(),
            vec!["前缀 ", "`代码`", " 后缀"],
            "code span boundaries land exactly on the backticks"
        );
        assert_eq!(spans[0].style.fg, Some(palette.text));
        assert_eq!(spans[1].style.fg, Some(palette.highlight));
        assert_eq!(spans[2].style.fg, Some(palette.text));
    }

    #[test]
    fn bold_and_italic_ranges_add_modifiers_with_markers_visible() {
        let line = "mix **b** and *i* plus _u_ end";
        let spans = highlight(line);
        assert_eq!(concat(&spans), line);
        let find = |needle: &str| {
            spans
                .iter()
                .find(|span| span.content.as_ref() == needle)
                .unwrap_or_else(|| panic!("span {needle:?}"))
        };
        assert!(find("**b**").style.add_modifier.contains(Modifier::BOLD));
        assert!(find("*i*").style.add_modifier.contains(Modifier::ITALIC));
        assert!(find("_u_").style.add_modifier.contains(Modifier::ITALIC));
        assert!(
            find("mix ").style.add_modifier.is_empty(),
            "plain text keeps no modifier"
        );
        let doubled = highlight("__strong__");
        assert_eq!(concat(&doubled), "__strong__");
        assert!(doubled[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn fence_state_toggles_across_lines_and_mutes_the_interior() {
        let palette = palette();
        let mut fence = false;
        let opener = markdown_highlight_line("```rust", &mut fence, palette);
        assert!(fence, "opening fence toggles the state on");
        assert_eq!(concat(&opener), "```rust");
        assert_eq!(opener[0].style.fg, Some(palette.muted));

        let interior = markdown_highlight_line("let x = `1` ** 2;", &mut fence, palette);
        assert!(fence, "interior lines leave the state on");
        assert_eq!(concat(&interior), "let x = `1` ** 2;");
        assert_eq!(interior.len(), 1, "no inline parsing inside a fence");
        assert_eq!(interior[0].style.fg, Some(palette.muted));

        let closer = markdown_highlight_line("```", &mut fence, palette);
        assert!(!fence, "closing fence toggles the state off");
        assert_eq!(closer[0].style.fg, Some(palette.muted));

        let after = markdown_highlight_line("plain again", &mut fence, palette);
        assert_eq!(after[0].style.fg, Some(palette.text));
    }

    #[test]
    fn unmatched_or_prose_markers_stay_plain() {
        let palette = palette();
        for line in [
            "broken `code",
            "lone **bold",
            "dangling *star",
            "2 * 3 * 4",
            "snake_case_name stays",
            "*",
            "**",
        ] {
            let spans = highlight(line);
            assert_eq!(concat(&spans), line, "content identity for {line:?}");
            for span in &spans {
                assert_eq!(
                    span.style.fg,
                    Some(palette.text),
                    "{line:?} keeps the plain text color"
                );
                assert!(
                    span.style.add_modifier.is_empty(),
                    "{line:?} gains no modifier"
                );
            }
        }
    }

    #[test]
    fn bullet_markers_get_accent_on_the_marker_only() {
        let palette = palette();
        for (line, marker) in [
            ("- item", "-"),
            ("* item", "*"),
            ("+ item", "+"),
            ("1. thing", "1."),
            ("12. thing", "12."),
        ] {
            let spans = highlight(line);
            assert_eq!(concat(&spans), line);
            let marker_span = spans
                .iter()
                .find(|span| span.content.as_ref() == marker)
                .unwrap_or_else(|| panic!("marker span for {line:?}"));
            assert_eq!(marker_span.style.fg, Some(palette.accent));
            let rest = spans
                .iter()
                .find(|span| {
                    span.content.as_ref().contains("item") || span.content.contains("thing")
                })
                .expect("rest of the line");
            assert_eq!(
                rest.style.fg,
                Some(palette.text),
                "only the marker is accented"
            );
        }

        let indented = highlight("  - nested");
        assert_eq!(concat(&indented), "  - nested");
        assert_eq!(indented[0].content.as_ref(), "  ");
        assert_eq!(indented[0].style.fg, Some(palette.text));
        assert_eq!(indented[1].content.as_ref(), "-");
        assert_eq!(indented[1].style.fg, Some(palette.accent));

        // `-nospace` is not a bullet; `* emph *x*` still parses the rest.
        let not_bullet = highlight("-nospace");
        assert_eq!(not_bullet[0].style.fg, Some(palette.text));
        let with_emphasis = highlight("* item *x*");
        assert_eq!(concat(&with_emphasis), "* item *x*");
        assert!(
            with_emphasis
                .iter()
                .any(|span| span.content.as_ref() == "*x*"
                    && span.style.add_modifier.contains(Modifier::ITALIC)),
            "emphasis still applies after the bullet marker"
        );
    }

    #[test]
    fn blockquote_lines_render_muted_italic_without_inline_parsing() {
        let palette = palette();
        let line = "> quoted **not bold**";
        let spans = highlight(line);
        assert_eq!(concat(&spans), line);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].style.fg, Some(palette.muted));
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
        assert!(!spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn highlighting_never_changes_line_content() {
        // The invariant every other guarantee (wrap, cursor) rests on: for any
        // input and either fence state, span contents concatenate back to the
        // exact line.
        let lines = [
            "# h",
            "``` info",
            "内部 `code` 行",
            "```",
            "**粗** *斜* _u_ mixed 文本",
            "- 列表 `x` **y**",
            "  12. ordered",
            "> 引用 quote",
            "plain 文本 with 🚀 emoji and ``` not at start… wait",
            "",
            "a*b*c _snake_case_ `未闭合",
            "   indented plain",
        ];
        for initial_fence in [false, true] {
            let mut fence = initial_fence;
            for line in lines {
                let spans = markdown_highlight_line(line, &mut fence, palette());
                assert_eq!(
                    concat(&spans),
                    line,
                    "content must be byte-identical (initial_fence={initial_fence})"
                );
            }
        }
    }

    #[test]
    fn split_spans_preserve_chunk_content_and_carry_styles_across_wraps() {
        let palette = palette();
        let base = palette.text().bg(palette.surface);
        let line = "内联 `代码块` **粗体** tail";
        let spans = highlight(line);
        // Narrow width forces the code span across a row boundary.
        let chunks = wrap_composer_line(line, 4);
        assert!(chunks.len() > 2, "narrow wrap yields several rows");
        let rows = split_highlighted_spans(&spans, &chunks, base);
        assert_eq!(rows.len(), chunks.len(), "one span row per wrap chunk");
        for (row, chunk) in rows.iter().zip(&chunks) {
            assert_eq!(&concat(row), chunk, "row content matches its chunk");
        }
        let joined: String = rows.iter().map(|row| concat(row)).collect();
        assert_eq!(joined, line, "all rows joined reproduce the line");
        // Every piece of the code span keeps the code color, wherever it wrapped.
        for row in &rows {
            for span in row {
                if span.content.contains('码') || span.content.contains('代') {
                    assert_eq!(span.style.fg, Some(palette.highlight));
                }
            }
        }
    }

    #[test]
    fn fence_delimiter_detection_ignores_indentation_only() {
        assert!(is_fence_line("```"));
        assert!(is_fence_line("```rust"));
        assert!(is_fence_line("   ```"));
        assert!(!is_fence_line("`` not a fence"));
        assert!(!is_fence_line("text ``` later"));
    }
}
