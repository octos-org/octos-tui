//! Contract tests for theme-following code highlighting
//! (`specs/task-highlight-theme-follow.spec`).

use octos_tui::cli::ThemeName;
use octos_tui::highlight::highlight_block;
use octos_tui::theme::Palette;
use ratatui::style::Color;

fn rust_body() -> Vec<String> {
    vec![
        "fn main() {".to_string(),
        "    let answer = \"forty-two\";".to_string(),
        "}".to_string(),
    ]
}

fn colors_for(theme: &str) -> Vec<Option<Color>> {
    highlight_block(
        "rust",
        &rust_body(),
        Palette::for_theme(ThemeName::default()).muted(),
        true,
        theme,
    )
    .iter()
    .flatten()
    .map(|span| span.style.fg)
    .collect()
}

#[test]
fn themes_yield_distinct_token_colors() {
    let eighties = colors_for("base16-eighties.dark");
    let solarized = colors_for("Solarized (dark)");

    assert_ne!(
        eighties, solarized,
        "different UI themes map to different syntect palettes"
    );
}

#[test]
fn cache_isolated_per_theme() {
    // Render theme A first so its result is cached, then theme B: B must get
    // its own colors, never A's cached output.
    let a = colors_for("base16-ocean.dark");
    let b = colors_for("Solarized (dark)");
    let b_again = colors_for("Solarized (dark)");

    assert_ne!(a, b, "theme B must not be served theme A's cache entry");
    assert_eq!(b, b_again, "repeat renders of the same theme are stable");
}

#[test]
fn unknown_theme_falls_back_safely() {
    let colors = colors_for("no-such-theme-name");

    let distinct: std::collections::HashSet<_> = colors.iter().flatten().collect();
    assert!(
        distinct.len() > 1,
        "the fallback theme still highlights (no panic, no monochrome); got {distinct:?}"
    );
}

#[test]
fn themed_highlight_keeps_no_background() {
    for theme in [
        "base16-eighties.dark",
        "Solarized (dark)",
        "base16-mocha.dark",
    ] {
        let rendered = highlight_block(
            "rust",
            &rust_body(),
            Palette::for_theme(ThemeName::default()).muted(),
            true,
            theme,
        );
        for span in rendered.iter().flatten() {
            assert_eq!(span.style.bg, None, "no background in theme {theme}");
        }
    }
}
