//! Clipboard copy support for the TUI.
//!
//! The TUI runs in the alternate screen with mouse capture enabled, so the
//! terminal's native click-drag selection is intercepted by the app and the
//! user cannot select-to-copy. This module provides an in-app copy path that
//! is **terminal-agnostic and SSH-safe**: it writes the system clipboard via
//! the [OSC 52] terminal escape sequence. OSC 52 travels in-band over the same
//! PTY/SSH channel the TUI already uses, so a copy on a remote host (the fleet
//! minis) lands in the *operator's local* clipboard — something a clipboard
//! crate that talks to the remote host's clipboard could never do.
//!
//! [OSC 52]: https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands
//!
//! Two pure, unit-tested pieces live here so the behaviour can be verified
//! without a real terminal:
//!  - [`osc52_copy_sequence`]: builds the escape sequence (base64 + framing).
//!  - [`copyable_assistant_text`]: decides *what* gets copied (the last
//!    assistant reply — the answer / research report / code block the user
//!    most often wants out of the TUI).

use crate::model::AppState;

/// Maximum size, in bytes, of the base64 payload inside the OSC 52 sequence.
///
/// Common terminals silently drop OSC 52 sequences past an internal limit —
/// xterm historically caps the whole sequence near 100 KB, and tmux's
/// passthrough adds framing on top — so a multi-hundred-KB copy would no-op
/// with no feedback. 72 KB of encoded payload (~54 KB of text) stays well
/// under every known cap while still fitting any realistic answer.
pub const OSC52_MAX_ENCODED_BYTES: usize = 72 * 1024;

/// Maximum input bytes so `base64(input)` never exceeds
/// [`OSC52_MAX_ENCODED_BYTES`]: base64 emits 4 output bytes per 3 input bytes.
const OSC52_MAX_INPUT_BYTES: usize = OSC52_MAX_ENCODED_BYTES / 4 * 3;

/// Build the OSC 52 escape sequence that sets the system clipboard to `text`.
///
/// Shape: `ESC ] 52 ; c ; <base64(text)> BEL`. The `c` selection targets the
/// clipboard (as opposed to the primary/selection buffer). Terminals that
/// honour OSC 52 (iTerm2, kitty, WezTerm, foot, recent xterm, tmux with
/// `set-clipboard on`, and SSH sessions through any of them) decode the
/// base64 payload and place it on the local clipboard.
///
/// The payload is standard base64 (RFC 4648, `+`/`/`, `=` padding) with **no**
/// line wrapping — line breaks in an OSC string would terminate the sequence.
/// Oversized input is truncated (head kept) to [`OSC52_MAX_ENCODED_BYTES`];
/// use [`osc52_copy_sequence_capped`] to learn whether truncation happened.
pub fn osc52_copy_sequence(text: &str) -> String {
    osc52_copy_sequence_for(text, std::env::var_os("TMUX").is_some())
}

/// Build the OSC 52 sequence, optionally wrapped for tmux passthrough.
///
/// Inside tmux, a bare OSC 52 sequence is consumed by tmux itself and never
/// reaches the outer terminal (so the operator's *local* clipboard is never
/// set). tmux's DCS passthrough — `ESC P tmux; <escaped-payload> ESC \` with the
/// inner `ESC` bytes doubled — forwards the sequence to the outer terminal.
/// codex uses the same wrapper (`clipboard_copy.rs`). The `set-clipboard on`
/// tmux option must also be enabled for this to work end to end.
///
/// Detection is by the `TMUX` env var (set by tmux for its child processes);
/// `tmux` is the parameterized seam so the behaviour is unit-testable.
pub fn osc52_copy_sequence_for(text: &str, tmux: bool) -> String {
    osc52_copy_sequence_capped(text, tmux).0
}

/// [`osc52_copy_sequence_for`] plus a truncation signal.
///
/// Returns `(sequence, truncated)`: when `text` encodes past
/// [`OSC52_MAX_ENCODED_BYTES`] the input is cut at the largest char boundary
/// that fits (keeping the head — the start of an answer is the part the user
/// asked for) and `truncated` is `true`, so callers can surface a "copied
/// first N KB" hint instead of a silent whole-copy no-op in the terminal.
pub fn osc52_copy_sequence_capped(text: &str, tmux: bool) -> (String, bool) {
    let (text, truncated) = truncate_at_char_boundary(text, OSC52_MAX_INPUT_BYTES);
    let encoded = base64_encode(text.as_bytes());
    let bare = format!("\x1b]52;c;{encoded}\x07");
    let sequence = if tmux {
        // Double every ESC inside the payload, then wrap in the tmux DCS frame.
        let escaped = bare.replace('\x1b', "\x1b\x1b");
        format!("\x1bPtmux;{escaped}\x1b\\")
    } else {
        bare
    };
    (sequence, truncated)
}

/// Truncate `text` to at most `max_bytes`, backing off to a UTF-8 char
/// boundary so the kept head is always valid UTF-8. Returns the (possibly
/// shortened) slice and whether anything was dropped.
fn truncate_at_char_boundary(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    (&text[..cut], true)
}

/// The text to copy when the user invokes the copy command: the most recent
/// assistant output for the active session.
///
/// Prefers the in-flight `live_reply` (the answer currently streaming in) and
/// otherwise falls back to the last committed assistant message. Returns
/// `None` when there is no assistant text yet (e.g. a fresh session), so the
/// caller can surface a "nothing to copy" hint instead of clobbering the
/// clipboard with an empty string.
pub fn copyable_assistant_text(state: &AppState) -> Option<String> {
    let session = state.active_session()?;

    if let Some(live) = session.live_reply.as_ref() {
        let trimmed = live.text.trim();
        if !trimmed.is_empty() {
            return Some(live.text.clone());
        }
    }

    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role.as_str() == "assistant" && !message.content.trim().is_empty())
        .map(|message| message.content.clone())
}

/// Minimal RFC 4648 standard base64 encoder (no line wrapping).
///
/// OSC 52 needs a self-contained, single-line base64 payload; rolling the few
/// lines here keeps the escape-sequence builder free of any line-wrapping or
/// alphabet surprises a general-purpose call site might introduce.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use octos_core::Message;
    use octos_core::SessionKey;
    use octos_core::app_ui::{AppUiLiveReply, AppUiSession};
    use octos_core::ui_protocol::TurnId;

    fn base_session() -> AppUiSession {
        AppUiSession {
            id: SessionKey("local:test".into()),
            title: "t".into(),
            profile_id: None,
            messages: Vec::new(),
            tasks: Vec::new(),
            live_reply: None,
        }
    }

    fn state_with(session: AppUiSession) -> AppState {
        AppState::new(vec![session], 0, "ready".into(), None, false)
    }

    fn empty_state() -> AppState {
        AppState::new(Vec::new(), 0, "ready".into(), None, false)
    }

    // --- base64 (RFC 4648 test vectors) ---

    #[test]
    fn should_match_rfc4648_base64_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn should_encode_non_ascii_utf8_bytes() {
        // "✓ café" — exercises multi-byte UTF-8 through the byte encoder.
        assert_eq!(base64_encode("✓ café".as_bytes()), "4pyTIGNhZsOp");
    }

    // --- OSC 52 framing ---

    #[test]
    fn should_wrap_payload_in_osc52_clipboard_frame() {
        let seq = osc52_copy_sequence("foobar");
        assert_eq!(seq, "\x1b]52;c;Zm9vYmFy\x07");
    }

    #[test]
    fn should_wrap_in_tmux_passthrough_when_inside_tmux() {
        // Matches codex's tmux DCS frame: ESC P tmux ; <esc-doubled OSC52> ESC \
        let seq = osc52_copy_sequence_for("foobar", /*tmux*/ true);
        assert_eq!(seq, "\x1bPtmux;\x1b\x1b]52;c;Zm9vYmFy\x07\x1b\\");
    }

    #[test]
    fn should_emit_bare_osc52_when_not_in_tmux() {
        let seq = osc52_copy_sequence_for("foobar", /*tmux*/ false);
        assert_eq!(seq, "\x1b]52;c;Zm9vYmFy\x07");
    }

    // --- OSC 52 payload cap ---

    fn payload_of(seq: &str) -> &str {
        seq.strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .expect("bare OSC 52 frame present")
    }

    #[test]
    fn should_not_truncate_input_at_or_below_the_cap() {
        let text = "a".repeat(OSC52_MAX_INPUT_BYTES);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(!truncated);
        assert_eq!(payload_of(&seq), base64_encode(text.as_bytes()));
        assert!(payload_of(&seq).len() <= OSC52_MAX_ENCODED_BYTES);
    }

    #[test]
    fn should_cap_oversized_payload_keeping_the_head() {
        // Multi-hundred-KB copy: terminals cap OSC 52 (~100 KB) and would
        // silently no-op. The sequence must stay under the documented cap.
        let text = "x".repeat(300 * 1024);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(truncated);
        let payload = payload_of(&seq);
        assert!(payload.len() <= OSC52_MAX_ENCODED_BYTES);
        // Head is kept: payload is exactly base64 of the input's prefix.
        assert_eq!(payload, base64_encode(&text.as_bytes()[..OSC52_MAX_INPUT_BYTES]));
        // Valid base64: length is a multiple of 4 (whole input, no padding split).
        assert_eq!(payload.len() % 4, 0);
    }

    #[test]
    fn should_truncate_on_a_char_boundary() {
        // 2-byte codepoints: with an odd byte cap the cut would land
        // mid-character; the cap must back off to a boundary, never panic.
        let ch = "é"; // 2 bytes
        let text = ch.repeat(OSC52_MAX_INPUT_BYTES); // ~110 KB, boundary at every even byte
        let (seq, truncated) = osc52_copy_sequence_capped(&text, false);
        assert!(truncated);
        let mut cut = OSC52_MAX_INPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        assert_eq!(payload_of(&seq), base64_encode(&text.as_bytes()[..cut]));
    }

    #[test]
    fn should_cap_the_tmux_wrapped_variant_too() {
        let text = "y".repeat(300 * 1024);
        let (seq, truncated) = osc52_copy_sequence_capped(&text, true);
        assert!(truncated);
        assert!(seq.starts_with("\x1bPtmux;"));
        // The whole wrapped sequence stays within cap + framing overhead.
        assert!(seq.len() <= OSC52_MAX_ENCODED_BYTES + 32);
    }

    #[test]
    fn public_uncapped_helpers_apply_the_same_cap() {
        let text = "z".repeat(300 * 1024);
        let seq = osc52_copy_sequence_for(&text, false);
        assert!(payload_of(&seq).len() <= OSC52_MAX_ENCODED_BYTES);
    }

    #[test]
    fn should_not_emit_newlines_in_the_escape_sequence() {
        // A literal newline mid-sequence would terminate the OSC string and
        // corrupt the terminal; the base64 payload must be single-line even
        // when the source text spans many lines.
        let seq = osc52_copy_sequence("line one\nline two\nline three\n");
        let payload = seq
            .strip_prefix("\x1b]52;c;")
            .and_then(|s| s.strip_suffix('\x07'))
            .expect("frame present");
        assert!(!payload.contains('\n'));
        assert!(!payload.contains('\r'));
    }

    // --- selection: what text gets copied ---

    #[test]
    fn should_copy_last_assistant_message_when_no_live_reply() {
        let mut session = base_session();
        session.messages = vec![
            Message::user("please summarize"),
            Message::assistant("first answer"),
            Message::user("and again"),
            Message::assistant("the final report"),
        ];
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("the final report")
        );
    }

    #[test]
    fn should_prefer_streaming_live_reply_over_committed_message() {
        let mut session = base_session();
        session.messages = vec![Message::assistant("old answer")];
        session.live_reply = Some(AppUiLiveReply {
            turn_id: TurnId::new(),
            text: "streaming answer".into(),
        });
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("streaming answer")
        );
    }

    #[test]
    fn should_fall_back_to_message_when_live_reply_is_blank() {
        let mut session = base_session();
        session.messages = vec![Message::assistant("committed answer")];
        session.live_reply = Some(AppUiLiveReply {
            turn_id: TurnId::new(),
            text: "   ".into(),
        });
        let state = state_with(session);
        assert_eq!(
            copyable_assistant_text(&state).as_deref(),
            Some("committed answer")
        );
    }

    #[test]
    fn should_return_none_when_session_has_no_assistant_text() {
        let mut session = base_session();
        session.messages = vec![Message::user("just a prompt")];
        let state = state_with(session);
        assert_eq!(copyable_assistant_text(&state), None);
    }

    #[test]
    fn should_return_none_when_no_active_session() {
        let state = empty_state();
        assert_eq!(copyable_assistant_text(&state), None);
    }
}
