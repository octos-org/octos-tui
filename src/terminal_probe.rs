//! Terminal detection and color adaptation for octos-tui.
//!
//! Modeled on codex-rs/tui/src/terminal_palette.rs and terminal_probe.rs.
//! Detects terminal color capability (TrueColor/ANSI256/ANSI16) and probes
//! the terminal's actual background color via OSC 11 so themes can adapt
//! to the user's light/dark terminal preference instead of assuming dark.
//!
//! Contains one `unsafe` call (fcntl for O_NONBLOCK on /dev/tty) required
//! for the one-shot OSC 11 startup probe — no safe alternative exists in
//! std for non-blocking file I/O. This is isolated, documented, and used
//! exactly once at startup.

#![allow(unsafe_code)]

use ratatui::style::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StdoutColorLevel {
    TrueColor,
    Ansi256,
    Ansi16,
    Unknown,
}

pub fn stdout_color_level() -> StdoutColorLevel {
    // Check COLORTERM first — many terminals advertise truecolor via env
    if std::env::var_os("COLORTERM").is_some_and(|v| {
        let v = v.to_string_lossy().to_lowercase();
        v.contains("truecolor") || v.contains("24bit")
    }) {
        return StdoutColorLevel::TrueColor;
    }
    match std::env::var_os("TERM").map(|v| v.to_string_lossy().to_lowercase()) {
        Some(term) if term.contains("truecolor") || term.contains("24bit") => {
            StdoutColorLevel::TrueColor
        }
        Some(term) if term.contains("256") => StdoutColorLevel::Ansi256,
        Some(_) => StdoutColorLevel::Ansi16,
        None => StdoutColorLevel::Unknown,
    }
}

/// Query terminal default colors via OSC 10 (fg) and OSC 11 (bg).
/// Returns None if the terminal doesn't respond or we can't probe.
/// Uses raw tty write/read with a short timeout — crossterm 0.28
/// doesn't have built-in background query (codex's terminal_probe.rs
/// handles this too).
///
/// The read path opens /dev/tty in non-blocking mode and busy-polls
/// with short sleeps — no libc, no unsafe.
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

    // Send OSC 10 (fg) + OSC 11 (bg) queries
    let _ = tty.write_all(b"\x1B]10;?\x1B\\\x1B]11;?\x1B\\");
    let _ = tty.flush();

    // Set the fd to non-blocking so read() returns immediately.
    // We use the portable std::os::unix::io::AsRawFd to get the fd,
    // then a single ioctl call to set O_NONBLOCK. This is the only
    // unsafe code in the module, isolated here.
    set_nonblocking(&tty);

    let deadline = Instant::now() + timeout;
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];

    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match tty.read(&mut chunk) {
            Ok(0) => {
                // No data yet — brief sleep, retry
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(colors) = parse_osc_color_response(&buf) {
                    return Some(colors);
                }
            }
            Err(_) => {
                // EAGAIN/EWOULDBLOCK — no data yet, brief sleep, retry
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
    }
}

/// Set a file to non-blocking mode using the `fcntl` syscall.
/// This is the ONLY unsafe function in the crate — isolated, documented,
/// and used only for the one-shot terminal color probe at startup.
/// The project's `#![deny(unsafe_code)]` is relaxed for this single call.
#[cfg(unix)]
fn set_nonblocking(f: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    let fd = f.as_raw_fd();
    // F_GETFL = 3, F_SETFL = 4, O_NONBLOCK = 0x800 on macOS
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0x800;
    unsafe extern "C" {
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    }
    unsafe {
        let flags = fcntl(fd, F_GETFL);
        if flags != -1 {
            fcntl(fd, F_SETFL, flags | O_NONBLOCK);
        }
    }
}

#[cfg(not(unix))]
pub fn query_default_colors(_timeout: std::time::Duration) -> Option<DefaultColors> {
    None
}

#[derive(Debug, Clone, Copy)]
pub struct DefaultColors {
    pub fg: (u8, u8, u8),
    pub bg: (u8, u8, u8),
}

/// Parse OSC 10/11 response: ESC ] 1 0 ; r g b : R R / G G / B B ESC \
/// or similar. Returns None if incomplete or malformed.
fn parse_osc_color_response(buf: &[u8]) -> Option<DefaultColors> {
    let text = String::from_utf8_lossy(buf);
    let mut fg = None;
    let mut bg = None;

    // Look for OSC 10 (fg) and OSC 11 (bg) responses
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

/// Parse "rgb:RR/GG/BB" or "rgba:RR/GG/BB/AA" → (r, g, b)
fn parse_rgb_triplet(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_end_matches(['\x1B', '\x07', '\\']);
    let s = s.strip_prefix("rgb:").or_else(|| s.strip_prefix("rgba:"))?;
    let mut parts = s.split('/');
    let r = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    let g = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    let b = u8::from_str_radix(parts.next()?.get(..2)?, 16).ok()?;
    Some((r, g, b))
}

/// Cached terminal state. Probed once at startup, then reused.
/// Avoids re-probing every frame.
pub struct TerminalInfo {
    pub color_level: StdoutColorLevel,
    pub default_colors: Option<DefaultColors>,
}

impl TerminalInfo {
    pub fn probe() -> Self {
        let color_level = stdout_color_level();
        // Skip OSC probe in tests — /dev/tty may not exist or may block.
        // Also skip when not connected to a terminal (CI, piped stdout).
        let default_colors = if cfg!(test) || !is_terminal() {
            None
        } else {
            query_default_colors(std::time::Duration::from_millis(100))
        };
        Self {
            color_level,
            default_colors,
        }
    }

    pub fn is_light_bg(&self) -> bool {
        self.default_colors
            .map(|c| is_light(c.bg))
            .unwrap_or(false)
    }
}

/// Global cached terminal info. Probed lazily on first access.
static TERMINAL_INFO: std::sync::OnceLock<TerminalInfo> = std::sync::OnceLock::new();

pub fn terminal_info() -> &'static TerminalInfo {
    TERMINAL_INFO.get_or_init(TerminalInfo::probe)
}

/// Returns true if stdin is connected to a terminal (not piped/redirected).
fn is_terminal() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

pub fn is_light(bg: (u8, u8, u8)) -> bool {
    let (r, g, b) = bg;
    let y = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    y > 128.0
}

/// Blend two RGB colors: result = fg*alpha + bg*(1-alpha)
pub fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8), alpha: f32) -> (u8, u8, u8) {
    let r = (fg.0 as f32 * alpha + bg.0 as f32 * (1.0 - alpha)) as u8;
    let g = (fg.1 as f32 * alpha + bg.1 as f32 * (1.0 - alpha)) as u8;
    let b = (fg.2 as f32 * alpha + bg.2 as f32 * (1.0 - alpha)) as u8;
    (r, g, b)
}

/// Pick the closest xterm-256 color to a target RGB.
/// Uses simple Euclidean distance in RGB space (CIE76 is overkill for
/// terminal color picking — the 240 fixed xterm colors are close enough
/// that RGB distance works fine and is much cheaper).
pub fn nearest_xterm_256(target: (u8, u8, u8)) -> u8 {
    // xterm colors 16-255 are fixed and terminal-independent.
    // 0-15 vary by terminal theme, so skip them.
    let mut best_idx = 16u8;
    let mut best_dist = f32::MAX;
    for i in 16..=255u8 {
        let c = xterm_color(i);
        let dist = {
            let dr = target.0 as f32 - c.0 as f32;
            let dg = target.1 as f32 - c.1 as f32;
            let db = target.2 as f32 - c.2 as f32;
            dr * dr + dg * dg + db * db
        };
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    best_idx
}

/// Standard xterm-256 color table (colors 16-255 are fixed).
/// Color 16-231: 6×6×6 cube. Color 232-255: grayscale ramp.
pub fn xterm_color(idx: u8) -> (u8, u8, u8) {
    match idx {
        0..=15 => match idx {
            0 => (0, 0, 0),
            1 => (128, 0, 0),
            2 => (0, 128, 0),
            3 => (128, 128, 0),
            4 => (0, 0, 128),
            5 => (128, 0, 128),
            6 => (0, 128, 128),
            7 => (192, 192, 192),
            8 => (128, 128, 128),
            9 => (255, 0, 0),
            10 => (0, 255, 0),
            11 => (255, 255, 0),
            12 => (0, 0, 255),
            13 => (255, 0, 255),
            14 => (0, 255, 255),
            _ => (255, 255, 255),
        },
        16..=231 => {
            let n = idx - 16;
            let r = n / 36;
            let g = (n % 36) / 6;
            let b = n % 6;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (scale(r), scale(g), scale(b))
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            (v, v, v)
        }
    }
}

/// Convert an RGB color to the best displayable color for the terminal's
/// actual capability. TrueColor → RGB passthrough. ANSI256 → nearest xterm.
/// ANSI16/Unknown → Color::default() (let terminal decide).
pub fn best_color(rgb: (u8, u8, u8)) -> Color {
    match terminal_info().color_level {
        StdoutColorLevel::TrueColor => Color::Rgb(rgb.0, rgb.1, rgb.2),
        StdoutColorLevel::Ansi256 => Color::Indexed(nearest_xterm_256(rgb)),
        StdoutColorLevel::Ansi16 | StdoutColorLevel::Unknown => Color::default(),
    }
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
    fn blend_basic() {
        assert_eq!(blend((255, 255, 255), (0, 0, 0), 0.5), (127, 127, 127));
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 0.0), (255, 255, 255));
        assert_eq!(blend((0, 0, 0), (255, 255, 255), 1.0), (0, 0, 0));
    }

    #[test]
    fn xterm_color_cube() {
        assert_eq!(xterm_color(16), (0, 0, 0));
        assert_eq!(xterm_color(21), (0, 0, 255));
        assert_eq!(xterm_color(231), (255, 255, 255));
        assert_eq!(xterm_color(232), (8, 8, 8));
        assert_eq!(xterm_color(255), (238, 238, 238));
    }

    #[test]
    fn nearest_xterm_finds_close() {
        let red = nearest_xterm_256((255, 0, 0));
        assert_eq!(xterm_color(red), (255, 0, 0));
    }

    #[test]
    fn parse_rgb() {
        assert_eq!(parse_rgb_triplet("rgb:ff/00/80"), Some((255, 0, 128)));
        assert_eq!(parse_rgb_triplet("rgb:0a/1b/2c"), Some((10, 27, 44)));
    }
}
