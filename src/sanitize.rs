//! Terminal control-sequence sanitisation for server-supplied text.
//!
//! Tool output (dev servers, npm, vite, …) frequently carries raw ANSI
//! escape sequences and cursor-control bytes. The TUI renders message and
//! preview text verbatim into styled ratatui lines — a stray `ESC [2J` or
//! OSC title write from a quoted tool result corrupts the whole viewport
//! ("garbled text"). Strip every escape sequence and non-printing control
//! byte at the ingestion boundary; keep `\n` and `\t`, and normalise `\r\n`
//! to `\n` (a bare `\r` is dropped rather than letting it overwrite the
//! rendered line).

use std::borrow::Cow;

/// Strip ANSI/VT escape sequences (CSI, OSC, DCS/APC/PM/SOS, 2-byte ESC
/// forms) and C0/C1 control bytes (except `\n`/`\t`) from `input`.
/// Returns `Cow::Borrowed` when the text is already clean, so the common
/// case allocates nothing.
pub fn strip_terminal_controls(input: &str) -> Cow<'_, str> {
    if !input.bytes().any(|byte| {
        byte == 0x1b
            || byte == b'\r'
            || (byte < 0x20 && byte != b'\n' && byte != b'\t')
            || byte == 0x7f
    }) && !input.chars().any(|ch| ('\u{80}'..='\u{9f}').contains(&ch))
    {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek().copied() {
                // CSI: ESC [ <params/intermediates> <final byte @-~>
                Some('[') => {
                    chars.next();
                    for follow in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&follow) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... terminated by BEL or ST (ESC \)
                Some(']') => {
                    chars.next();
                    let mut prev_esc = false;
                    for follow in chars.by_ref() {
                        if follow == '\u{07}' || (prev_esc && follow == '\\') {
                            break;
                        }
                        prev_esc = follow == '\u{1b}';
                    }
                }
                // DCS / SOS / PM / APC: ESC P / X / ^ / _ ... terminated by ST
                Some('P') | Some('X') | Some('^') | Some('_') => {
                    chars.next();
                    let mut prev_esc = false;
                    for follow in chars.by_ref() {
                        if prev_esc && follow == '\\' {
                            break;
                        }
                        prev_esc = follow == '\u{1b}';
                    }
                }
                // Two-byte ESC sequences (ESC c, ESC 7, ESC =, charset
                // selection ESC ( X, …). Consume one following byte; for the
                // charset designators consume their parameter byte too.
                Some('(') | Some(')') | Some('*') | Some('+') => {
                    chars.next();
                    chars.next();
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {
                // Normalise \r\n to \n; drop bare \r instead of letting it
                // overwrite the rendered line.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    out.push('\n');
                }
            }
            // Unicode C1 controls: CSI/OSC/DCS single-char forms behave like
            // their ESC-prefixed equivalents; the rest are dropped.
            '\u{9b}' => {
                for follow in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&follow) {
                        break;
                    }
                }
            }
            '\u{9d}' | '\u{90}' => {
                let mut prev_esc = false;
                for follow in chars.by_ref() {
                    if follow == '\u{07}' || follow == '\u{9c}' || (prev_esc && follow == '\\') {
                        break;
                    }
                    prev_esc = follow == '\u{1b}';
                }
            }
            '\u{80}'..='\u{9f}' => {}
            '\n' | '\t' => out.push(ch),
            ch if (ch as u32) < 0x20 || ch == '\u{7f}' => {}
            ch => out.push(ch),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_borrows_without_allocation() {
        let text = "plain text\nwith lines\tand tabs";
        assert!(matches!(
            strip_terminal_controls(text),
            Cow::Borrowed(borrowed) if borrowed == text
        ));
    }

    #[test]
    fn strips_csi_color_and_cursor_sequences() {
        let input = "\u{1b}[32mready\u{1b}[0m in \u{1b}[1m123 ms\u{1b}[0m\u{1b}[2J\u{1b}[H";
        assert_eq!(strip_terminal_controls(input), "ready in 123 ms");
    }

    #[test]
    fn strips_osc_title_writes_with_bel_and_st_terminators() {
        let input = "\u{1b}]0;astro dev\u{07}serving\u{1b}]8;;http://x\u{1b}\\link";
        assert_eq!(strip_terminal_controls(input), "servinglink");
    }

    #[test]
    fn normalises_carriage_returns_and_drops_progress_overwrites() {
        assert_eq!(
            strip_terminal_controls("line one\r\nline two"),
            "line one\nline two"
        );
        assert_eq!(strip_terminal_controls("50%\r100%"), "50%100%");
    }

    #[test]
    fn strips_c0_controls_but_keeps_newline_and_tab() {
        let input = "a\u{08}b\u{07}c\nd\te\u{7f}f";
        assert_eq!(strip_terminal_controls(input), "abc\nd\tef");
    }

    #[test]
    fn vite_style_boxed_banner_survives_readably() {
        let input = "\u{1b}[36m\u{1b}[1m  ┃ Local    http://localhost:4321/\u{1b}[22m\u{1b}[39m";
        assert_eq!(
            strip_terminal_controls(input),
            "  ┃ Local    http://localhost:4321/"
        );
    }
}
