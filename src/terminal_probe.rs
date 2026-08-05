//! Terminal detection and color adaptation for octos-tui.
//!
//! Probes the terminal's actual background color via OSC 11 so the `Terminal`
//! theme can adapt to the user's light/dark terminal preference instead of
//! assuming a dark background. The probe runs ONCE at startup — warmed in
//! `event_loop::run` right after raw mode is enabled and before the input loop
//! begins — and is cached in a `OnceLock`, so the per-frame theme lookup never
//! re-probes and never reads keystrokes off `/dev/tty` during rendering.
//!
//! Modeled on codex-rs/tui/src/terminal_palette.rs. Contains NO `unsafe`: the
//! non-blocking `/dev/tty` setup goes through `rustix` (a safe, per-platform
//! wrapper), so the module honors the crate-wide `deny(unsafe_code)` lint.

/// Query terminal default colors via OSC 10 (fg) and OSC 11 (bg).
/// Returns `None` if the terminal doesn't respond within `timeout` or we can't
/// open `/dev/tty`.
///
/// crossterm 0.28 has no built-in background query, so this writes the OSC
/// query straight to `/dev/tty` and reads the reply. The fd is put into
/// NON-BLOCKING mode via `rustix` before the first read — critical, because
/// the `timeout` deadline is only checked BETWEEN reads: a blocking `read()`
/// on a terminal that never answers OSC 11 (tmux without passthrough, or a
/// terminal lacking OSC 11 support) would hang forever and wedge the TUI.
#[cfg(unix)]
pub fn query_default_colors(timeout: std::time::Duration) -> Option<DefaultColors> {
    use std::fs::OpenOptions;
    use std::io::{Read, Write};
    use std::time::Instant;

    // Open /dev/tty read+write. Fails silently if no controlling terminal.
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;

    // Send OSC 10 (fg) + OSC 11 (bg) queries.
    let _ = tty.write_all(b"\x1B]10;?\x1B\\\x1B]11;?\x1B\\");
    let _ = tty.flush();

    // Set the fd to non-blocking so `read()` returns `EAGAIN` immediately
    // instead of blocking when the terminal has no reply queued. `rustix`'s
    // `ioctl_fionbio` is the safe, per-platform-correct equivalent of
    // `fcntl(fd, F_SETFL, O_NONBLOCK)` — the previous hand-rolled version
    // hardcoded `O_NONBLOCK = 0x800`, which is the LINUX value (on macOS/BSD
    // that bit is `O_EXCL`), so non-blocking was never actually set there and
    // the read below could hang past the deadline.
    rustix::io::ioctl_fionbio(&tty, true).ok()?;

    let deadline = Instant::now() + timeout;
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];

    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match tty.read(&mut chunk) {
            Ok(0) => {
                // No data yet — brief sleep, retry.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(colors) = parse_osc_color_response(&buf) {
                    return Some(colors);
                }
            }
            Err(_) => {
                // EAGAIN/EWOULDBLOCK — no data yet, brief sleep, retry.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }
}

/// Non-Unix stub: there's no `/dev/tty` OSC probe path, so the public API is
/// kept platform-symmetric by always returning `None`. Callers fall back to
/// the dark-background default.
#[cfg(not(unix))]
pub fn query_default_colors(_timeout: std::time::Duration) -> Option<DefaultColors> {
    None
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultColors {
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
}

/// Parse OSC 10/11 response: `ESC ] 1 0 ; rgb:RR/GG/BB ESC \` (or similar).
/// Returns `None` if incomplete or malformed.
///
/// Pure/OS-independent (only the tty I/O in `query_default_colors` is
/// Unix-gated), so it stays ungated to compile + unit-test on every platform.
/// On non-Unix it's reachable only from tests (no probe caller), so allow the
/// dead-code lint there.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_osc_color_response(buf: &[u8]) -> Option<DefaultColors> {
    let text = String::from_utf8_lossy(buf);
    let mut fg = None;
    let mut bg = None;

    // Look for OSC 10 (fg) and OSC 11 (bg) responses.
    for part in text.split("\x1B\\") {
        let part = part.trim_start_matches('\x1B');
        if let Some(rest) = part.strip_prefix("]10;") {
            fg = parse_rgb_triplet(rest);
        } else if let Some(rest) = part.strip_prefix("]11;") {
            bg = parse_rgb_triplet(rest);
        }
    }
    match (fg, bg) {
        (Some(f), Some(b)) => Some(DefaultColors { fg: f, bg: b }),
        _ => None,
    }
}

/// Parse `"rgb:RR/GG/BB"` or `"rgba:RR/GG/BB/AA"` → `(r, g, b)`.
///
/// Terminals commonly reply with 16-bit-per-channel values
/// (`rgb:RRRR/GGGG/BBBB`, e.g. `rgb:ffff/8000/0000`); we take the TOP byte of
/// each channel by reading only the first two hex digits.
///
/// Pure/OS-independent — ungated so it compiles + unit-tests on every platform;
/// on non-Unix it's reachable only from tests, so allow dead-code there.
#[cfg_attr(not(unix), allow(dead_code))]
fn parse_rgb_triplet(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_end_matches(['\x1B', '\x07', '\\']);
    let s = s.strip_prefix("rgb:").or_else(|| s.strip_prefix("rgba:"))?;
    let mut parts = s.split('/');
    let r = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    let g = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    let b = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    Some((r, g, b))
}

/// Cached terminal state. Probed once at startup, then reused every frame.
pub struct TerminalInfo {
    pub default_colors: Option<DefaultColors>,
}

impl TerminalInfo {
    pub fn probe() -> Self {
        Self {
            default_colors: probe_default_colors(),
        }
    }

    pub fn is_light_bg(&self) -> bool {
        self.default_colors.map(|c| is_light(c.bg)).unwrap_or(false)
    }
}

/// Perform the one-shot OSC background probe if — and only if — we have a
/// usable interactive controlling terminal to answer it. Skipped in tests
/// (`/dev/tty` may not exist or may block a CI runner).
#[cfg(unix)]
fn probe_default_colors() -> Option<DefaultColors> {
    if cfg!(test) || !probe_tty_is_interactive() {
        return None;
    }
    query_default_colors(std::time::Duration::from_millis(100))
}

#[cfg(not(unix))]
fn probe_default_colors() -> Option<DefaultColors> {
    None
}

/// Returns true when a usable, interactive controlling terminal is available
/// to probe. The OSC probe talks to `/dev/tty` (NOT stdin/stdout), so the
/// guard checks THAT fd — gating on `stdin`, as the original did, is
/// inconsistent with what's actually probed: stdin can be piped while the tty
/// stays interactive (`octos-tui < script`), or a terminal while the process
/// has no controlling tty. Checking the probe target itself is what keeps the
/// non-blocking read path from being entered on a fd it can't use.
#[cfg(unix)]
fn probe_tty_is_interactive() -> bool {
    use std::io::IsTerminal;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(|tty| tty.is_terminal())
        .unwrap_or(false)
}

/// Global cached terminal info. Warmed once at startup via `terminal_info()`
/// (see `event_loop::run`), then reused for the lifetime of the process.
static TERMINAL_INFO: std::sync::OnceLock<TerminalInfo> = std::sync::OnceLock::new();

pub fn terminal_info() -> &'static TerminalInfo {
    TERMINAL_INFO.get_or_init(TerminalInfo::probe)
}

pub fn is_light(bg: (u8, u8, u8)) -> bool {
    let (r, g, b) = bg;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y > 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_light_dark() {
        assert!(is_light((255, 255, 255)));
        assert!(is_light((200, 200, 200)));
        assert!(!is_light((0, 0, 0)));
        assert!(!is_light((30, 30, 30)));
        assert!(!is_light((128, 128, 128)));
    }

    #[test]
    fn parse_rgb() {
        assert_eq!(parse_rgb_triplet("rgb:ff/00/80"), Some((255, 0, 128)));
        assert_eq!(parse_rgb_triplet("rgb:0a/1b/2c"), Some((10, 27, 44)));
    }

    #[test]
    fn parse_rgb_16bit_reduces_top_byte() {
        // The common terminal reply is 16-bit-per-channel `rgb:RRRR/GGGG/BBBB`.
        // We keep the top byte of each channel (first two hex digits).
        assert_eq!(parse_rgb_triplet("rgb:ffff/8000/0000"), Some((255, 128, 0)));
        // With a trailing ST (ESC \) still attached, as a real reply carries.
        assert_eq!(
            parse_rgb_triplet("rgb:ffff/ffff/ffff\x1B\\"),
            Some((255, 255, 255))
        );
    }

    #[test]
    fn parse_osc_bg_response() {
        // A full OSC 10 + OSC 11 reply parses into fg/bg with top-byte values.
        let reply = b"\x1B]10;rgb:ffff/ffff/ffff\x1B\\\x1B]11;rgb:0000/0000/0000\x1B\\";
        let colors = parse_osc_color_response(reply).expect("both fg+bg parse");
        assert_eq!(colors.fg, (255, 255, 255));
        assert_eq!(colors.bg, (0, 0, 0));
        assert!(!is_light(colors.bg));
    }
}
