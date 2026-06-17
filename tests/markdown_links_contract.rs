//! Contract tests for markdown link / strikethrough / horizontal-rule rendering
//! (`specs/task-markdown-links.spec`). Driven through the public scrollback
//! render path (`finalized_history_lines`), which feeds the same
//! `inline_markdown_spans` used by the live tail and the pager.

use octos_core::{Message, SessionKey};
use octos_tui::app::finalized_history_lines;
use octos_tui::cli::ThemeName;
use octos_tui::model::{AppState, SessionView};
use octos_tui::theme::Palette;
use ratatui::style::Modifier;
use ratatui::text::Line;

fn render(reply: &str) -> Vec<Line<'static>> {
    let app = AppState::new(
        vec![SessionView {
            id: SessionKey("local:md".into()),
            title: "md".into(),
            profile_id: Some("coding".into()),
            messages: vec![Message::user("q"), Message::assistant(reply)],
            tasks: vec![],
            live_reply: None,
        }],
        0,
        "ready".into(),
        None,
        false,
    );
    finalized_history_lines(&app, Palette::for_theme(ThemeName::Slate), 80)
}

fn spans(lines: &[Line<'static>]) -> Vec<(String, ratatui::style::Style)> {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|s| (s.content.to_string(), s.style))
        .collect()
}

#[test]
fn link_renders_text_and_muted_url() {
    let palette = Palette::for_theme(ThemeName::Slate);
    let lines = render("see [Octos](https://example.com) now");
    let spans = spans(&lines);

    let text = spans
        .iter()
        .find(|(c, _)| c == "Octos")
        .expect("link text span");
    assert_eq!(
        text.1.fg,
        Some(palette.highlight),
        "link text uses the highlight/selected color"
    );
    let url = spans
        .iter()
        .find(|(c, _)| c.contains("https://example.com"))
        .expect("url span");
    assert!(
        url.1.add_modifier.contains(Modifier::DIM),
        "url is rendered dimmed/muted"
    );
}

#[test]
fn link_emits_no_osc8_escape() {
    let lines = render("[Octos](https://example.com)");
    for (content, _) in spans(&lines) {
        assert!(
            !content.contains("\u{1b}]8"),
            "no raw OSC 8 hyperlink escape (would corrupt cell layout): {content:?}"
        );
    }
}

#[test]
fn strikethrough_adds_crossed_out() {
    let lines = render("this is ~~obsolete~~ text");
    let span = spans(&lines)
        .into_iter()
        .find(|(c, _)| c == "obsolete")
        .expect("struck span");
    assert!(span.1.add_modifier.contains(Modifier::CROSSED_OUT));
}

#[test]
fn horizontal_rule_renders_divider() {
    let lines = render("above\n\n---\n\nbelow");
    let joined: Vec<String> = lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    assert!(
        joined.iter().any(|row: &String| row.contains("───")),
        "thematic break renders as a divider line: {joined:#?}"
    );
    assert!(
        !joined.iter().any(|row: &String| row.trim() == "---"),
        "literal dashes must not survive"
    );
}

#[test]
fn long_url_is_not_truncated() {
    let long = format!("https://example.com/{}", "segment/".repeat(12));
    let lines = render(&format!("[doc]({long})"));
    let joined: String = lines
        .iter()
        .flat_map(|l| l.spans.iter())
        .map(|s| s.content.to_string())
        .collect();
    assert!(
        joined.contains(&long),
        "the full url must survive (no truncation) for terminal auto-linkification"
    );
    assert!(!joined.contains(" ..."), "no truncation ellipsis");
}

#[test]
fn non_link_brackets_render_plain() {
    let palette = Palette::for_theme(ThemeName::Slate);
    let lines = render("just [some brackets] here");
    let spans = spans(&lines);
    // No DIM url span should appear, and the bracket text isn't highlight-colored.
    assert!(
        !spans
            .iter()
            .any(|(_, st)| st.add_modifier.contains(Modifier::DIM)),
        "plain brackets must not produce a url span"
    );
    assert!(
        !spans
            .iter()
            .any(|(c, st)| c.contains("brackets") && st.fg == Some(palette.highlight)),
        "plain brackets must not be styled as a link"
    );
}
