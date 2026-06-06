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
pub fn osc52_copy_sequence(text: &str) -> String {
    let encoded = base64_encode(text.as_bytes());
    format!("\x1b]52;c;{encoded}\x07")
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
