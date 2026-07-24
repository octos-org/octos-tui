//! Test module for [`crate::app`] (#365): moved out of `app.rs`, which was
//! 23k lines of which ~87% was this test code. `use super::*` reaches the
//! app module's items exactly as the inline test module did.

#![allow(clippy::module_inception)]

use super::*;

/// #324: the session strip costs a row only with 2+ open sessions.
#[test]
fn session_strip_row_appears_only_with_multiple_sessions() {
    let one = AppState::new(
        vec![SessionView {
            id: SessionKey("local:a".into()),
            title: "a".into(),
            profile_id: None,
            messages: vec![],
            tasks: vec![],
            live_reply: None,
        }],
        0,
        "ready".into(),
        None,
        false,
    );
    assert_eq!(session_strip_height(&one), 0, "single session pays no row");

    let two = AppState::new(
        vec![
            SessionView {
                id: SessionKey("local:a".into()),
                title: "a".into(),
                profile_id: None,
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            },
            SessionView {
                id: SessionKey("local:b".into()),
                title: "b".into(),
                profile_id: None,
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            },
        ],
        0,
        "ready".into(),
        None,
        false,
    );
    assert_eq!(session_strip_height(&two), 1);
}

fn composer_height(app: &AppState) -> u16 {
    composer_height_for_size(app, 120, 42)
}
fn inline_diff_style_for_test(kind: &str, palette: Palette) -> Style {
    diff_line_style(kind, palette)
}
fn inline_diff_marker_style_for_test(kind: &str, palette: Palette) -> Style {
    diff_line_marker_style(kind, palette)
}
mod tests {
    use super::super::*;
    use super::*;
    use crate::{
        cli::ThemeName,
        model::{
            ApprovalModalState, DiffPreview, DiffPreviewFile, DiffPreviewGetResult,
            DiffPreviewHunk, DiffPreviewLine, ModelStatus, SessionModelCatalog,
            SessionRuntimeStatus, SessionView, TurnActivitySummary, TurnPromptAnchor,
        },
        store::Store,
        viewport::ScrollbackTracker,
    };

    #[test]
    fn user_message_block_uses_bright_gutter_and_reverse_video_body() {
        // Reverse video is theme-independent and SSH-portable — assert it on
        // BOTH the raw Terminal theme and a rich Rgb theme, since the old
        // surface_alt shade was invisible in both (dropped over SSH / too
        // subtle). Each input line → a bright bold accent gutter + a
        // reverse-video bold body bar.
        for theme in [ThemeName::Terminal, ThemeName::Codex] {
            let palette = Palette::for_theme(theme);
            let mut lines: Vec<Line<'static>> = Vec::new();
            push_user_message_block(&mut lines, palette, "line one\nline two");

            let user_lines: Vec<&Line<'static>> = lines
                .iter()
                .filter(|l| l.spans.first().is_some_and(|s| s.content.starts_with('▌')))
                .collect();
            assert_eq!(
                user_lines.len(),
                2,
                "one styled line per input line ({theme:?})"
            );
            for line in user_lines {
                let gutter = &line.spans[0];
                assert_eq!(
                    gutter.style.fg,
                    Some(palette.accent),
                    "accent gutter ({theme:?})"
                );
                assert!(
                    gutter.style.add_modifier.contains(Modifier::BOLD),
                    "gutter is bold/bright ({theme:?})"
                );
                let body = &line.spans[1];
                assert!(
                    body.style.add_modifier.contains(Modifier::REVERSED),
                    "body is a reverse-video bar — renders on any terminal ({theme:?})"
                );
                assert!(
                    body.content.contains("line"),
                    "body carries the user text ({theme:?})"
                );
            }
        }
    }
    use octos_core::{
        Message, SessionKey,
        ui_protocol::{
            ApprovalId, PreviewId, QuestionId, TaskRuntimeState, TurnId, UiProtocolCapabilities,
        },
    };
    use ratatui::{
        Terminal,
        backend::{Backend, TestBackend},
        buffer::Buffer,
        layout::Position,
    };

    fn rendered_buffer(app: &AppState, palette: Palette) -> Buffer {
        rendered_buffer_with_size(app, palette, 120, 42)
    }

    fn rendered_buffer_with_size(
        app: &AppState,
        palette: Palette,
        width: u16,
        height: u16,
    ) -> Buffer {
        rendered_buffer_and_cursor_with_size(app, palette, width, height).0
    }

    fn rendered_buffer_and_cursor(app: &AppState, palette: Palette) -> (Buffer, Position) {
        rendered_buffer_and_cursor_with_size(app, palette, 120, 42)
    }

    fn rendered_buffer_and_cursor_with_size(
        app: &AppState,
        palette: Palette,
        width: u16,
        height: u16,
    ) -> (Buffer, Position) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal
            .draw(|frame| render(frame, app, palette))
            .expect("render succeeds");
        let cursor = terminal
            .backend_mut()
            .get_cursor_position()
            .expect("cursor position");
        (terminal.backend().buffer().clone(), cursor)
    }

    fn rendered_text(app: &AppState) -> String {
        rendered_buffer(app, Palette::for_theme(ThemeName::Slate))
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn rendered_rows(buffer: &Buffer) -> Vec<String> {
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        (0..height)
            .map(|y| {
                let row_start = y * width;
                buffer.content[row_start..row_start + width]
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn row_containing<'a>(rows: &'a [String], needle: &str) -> &'a str {
        rows.iter()
            .find(|row| row.contains(needle))
            .map(String::as_str)
            .unwrap_or_else(|| panic!("row containing {needle:?}"))
    }

    /// Test-only [`SessionRuntimeStatus`] carrying just the fields the composer
    /// footer reads (model + workspace root); everything else stays empty.
    fn runtime_status_with_model_cwd(
        session_id: SessionKey,
        model: &str,
        cwd: &str,
    ) -> SessionRuntimeStatus {
        SessionRuntimeStatus {
            session_id,
            runtime_mode: None,
            profile_id: None,
            cwd: Some(cwd.into()),
            workspace_root: Some(cwd.into()),
            active_turn_id: None,
            runtime_policy_stamp: None,
            model: Some(ModelStatus {
                model: model.into(),
                provider: "test".into(),
                title: None,
                family: None,
                route: None,
                selected: true,
                available: Some(true),
                queue_mode: None,
                qoe_policy: None,
            }),
            permission_profile: None,
            approval_policy: None,
            sandbox_mode: None,
            sandbox: None,
            filesystem_scope: None,
            network: None,
            tool_policy_id: None,
            mcp_servers: Vec::new(),
            memory_scope: None,
            health: None,
            mcp_summary: None,
            tool_summary: None,
            usage: None,
            cursor: None,
        }
    }

    #[test]
    fn render_composer_shows_current_model_and_cwd_on_bottom_border() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // A non-home absolute path renders verbatim regardless of the test
        // runner's HOME (home-dir collapsing is covered separately).
        app.workspace.root = "/srv/octos-workspace".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "claude-fable-5",
            "/srv/octos-workspace",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);

        // The current model is surfaced ONLY on the composer footer (the status
        // bar never shows it), so the row carrying it is the composer's bottom
        // border — and that same border row must also carry the cwd.
        let footer = row_containing(&rows, "claude-fable-5");
        assert!(
            footer.contains("/srv/octos-workspace"),
            "composer bottom border should show the cwd next to the model; got {footer:?}"
        );
    }

    fn app_with_reasoning_message(reasoning: &str) -> (AppState, SessionKey) {
        let session_id = SessionKey("local:rsn".into());
        let mut assistant = Message::assistant("the answer is 4");
        assistant.reasoning_content = Some(reasoning.to_string());
        let app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "t".into(),
                profile_id: None,
                messages: vec![Message::user("q"), assistant],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        (app, session_id)
    }

    fn history_text(app: &AppState) -> String {
        finalized_history_lines(app, Palette::for_theme(ThemeName::Codex), 200)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn reasoning_block_hidden_by_default_shown_when_toggled_on() {
        let (mut app, session_id) = app_with_reasoning_message("step one\nstep two");
        // OFF (default): the reasoning text must not appear in scrollback.
        assert!(
            !history_text(&app).contains("reasoning"),
            "reasoning block must be hidden by default"
        );
        // ON: the block renders.
        app.session_reasoning_display.insert(session_id);
        let text = history_text(&app);
        assert!(
            text.contains("· reasoning"),
            "toggle on renders the block: {text}"
        );
        assert!(text.contains("step one") && text.contains("step two"));
    }

    #[test]
    fn reasoning_block_caps_lines_until_expanded() {
        let long: String = (1..=12)
            .map(|n| format!("thought line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (mut app, session_id) = app_with_reasoning_message(&long);
        app.session_reasoning_display.insert(session_id);

        let capped = history_text(&app);
        assert!(
            capped.contains("thought line 6"),
            "cap shows the first 6 lines"
        );
        assert!(
            !capped.contains("thought line 7"),
            "beyond the cap is hidden until expanded"
        );
        assert!(capped.contains("more line(s) (Ctrl+O expand)"));

        app.expanded_tool_outputs = true;
        let expanded = history_text(&app);
        assert!(
            expanded.contains("thought line 12"),
            "Ctrl+O expand shows the full reasoning"
        );
    }

    #[test]
    fn toggling_reasoning_display_does_not_reflush_committed_scrollback() {
        // A terminal can't retroactively redraw scrolled-off history, so the
        // toggle must NOT flip the committed fingerprint — otherwise the
        // scrollback tracker's discontinuity branch would re-flush the whole
        // history and duplicate it below the stale copy. The toggle applies to
        // turns committed afterwards; past turns use the Tab inspector.
        let (mut app, session_id) = app_with_reasoning_message("some reasoning");
        let off = committed_messages_fingerprint(&app);
        app.session_reasoning_display.insert(session_id);
        let on = committed_messages_fingerprint(&app);
        assert_eq!(
            off.content_hash, on.content_hash,
            "the display toggle must not force a committed-history re-flush"
        );
    }

    #[test]
    fn in_progress_status_marker_is_the_galaxy_spinner() {
        // The pinned "still working" signal: the in-progress status marker is
        // one of the galaxy spinner frames (not a static bullet), so it stays
        // visible in the status bar even when the transcript chip scrolls off.
        let marker = run_state_marker(&SessionRunState::InProgress);
        assert!(
            SPINNER_FRAMES.contains(&marker),
            "in-progress marker must be a galaxy spinner frame, got {marker:?}"
        );
        // Settled states keep their static, non-animated markers.
        assert_eq!(run_state_marker(&SessionRunState::Success), "✓");
        assert_eq!(run_state_marker(&SessionRunState::Idle), "·");
    }

    #[test]
    fn galaxy_spinner_frames_swirl_and_stay_single_width() {
        // Every frame must occupy exactly ONE terminal cell: the marker slots
        // into fixed layout math (status bar, tool-card bullets, the
        // "◠ Working…" gradient label), so a width-2 frame would shove the
        // columns over once per cycle. Ambiguous-width-but-1 glyphs are fine —
        // ✻ / ⚠ are already shipped precedent in this UI.
        for frame in SPINNER_FRAMES {
            assert_eq!(
                UnicodeWidthStr::width(frame),
                1,
                "spinner frame {frame:?} must be exactly one cell wide"
            );
        }
        // The swirl: a 6-frame arc sweeping one full clockwise revolution,
        // then a 2-frame core glint (bright ✦ → fading ✧). At the 120ms tick
        // that is a 720ms rotation + a 240ms sparkle per 960ms cycle.
        assert_eq!(
            SPINNER_FRAMES,
            ["◜", "◠", "◝", "◞", "◡", "◟", "✦", "✧"],
            "galaxy swirl = arc revolution + core glint"
        );
        // The ticker (`spinner_frame`) indexes `elapsed/120 % len()`, so
        // whatever it returns must be a member of the cycle — no
        // out-of-bounds step at any elapsed time or frame-list length.
        assert!(SPINNER_FRAMES.contains(&spinner_frame()));
    }

    #[test]
    fn swimming_octopus_frames_have_boxed_eyes_four_arms_and_flip_direction() {
        // Each frame: a `[⇔]` head with one tilted-line arm glyph per side (彡/ミ).
        for frame in OCTOPUS_SWIM_FRAMES {
            assert!(frame.contains("[⇔]"), "[⇔] head: {frame}");
            let (left, right) = frame.split_once("[⇔]").expect("head splits arms");
            assert_eq!(left.chars().count(), 1, "one arm glyph left: {frame}");
            assert_eq!(right.chars().count(), 1, "one arm glyph right: {frame}");
        }
        // The two frames face opposite ways — now the travel *direction*:
        // 彡[⇔]ミ swims right, ミ[⇔]彡 swims left.
        assert_eq!(OCTOPUS_SWIM_FRAMES[0], "彡[⇔]ミ");
        assert_eq!(OCTOPUS_SWIM_FRAMES[1], "ミ[⇔]彡");
    }

    #[test]
    fn octopus_swim_starts_at_origin_with_the_first_stroke() {
        // elapsed=0 → sitting at the left margin, first paddle stroke.
        let (offset, frame) = octopus_swim(0, 80);
        assert_eq!(offset, 0, "starts flush-left");
        assert_eq!(frame, "彡[⇔]ミ");
        assert_eq!(frame, OCTOPUS_SWIM_FRAMES[0]);
    }

    #[test]
    fn octopus_swim_rests_at_the_far_edge_for_a_visible_window() {
        // The far edge must be PAINTABLE, not merely touched for a single
        // millisecond: the event loop repaints only every ~120ms, so the
        // octopus rests at MAX for the whole [SWEEP, SWEEP+DWELL] window —
        // any repaint cadence ≤ DWELL lands at least one frame on the edge
        // (codex P2 on the fixed-4s sweep). Same for the origin rest at the
        // cycle tail.
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        assert!(
            OCTOPUS_EDGE_DWELL_MS >= 200,
            "edge rest must cover at least one ~120ms repaint interval"
        );
        let leg = OCTOPUS_SWEEP_ONE_WAY_MS + OCTOPUS_EDGE_DWELL_MS;
        for wrap_width in [octopus_width + 2, 20usize, 40, 80, 146, 200, 1000] {
            let max = wrap_width.saturating_sub(octopus_width + 1);
            // Every sample within the far-edge rest window sits at MAX…
            for t in (OCTOPUS_SWEEP_ONE_WAY_MS..=leg).step_by(50) {
                let (offset, _) = octopus_swim(t, wrap_width);
                assert_eq!(
                    offset, max,
                    "must rest at the far edge at {t}ms, wrap_width={wrap_width}"
                );
            }
            // …and every sample within the origin rest window sits at 0.
            for t in ((leg + OCTOPUS_SWEEP_ONE_WAY_MS)..2 * leg).step_by(50) {
                let (offset, _) = octopus_swim(t, wrap_width);
                assert_eq!(
                    offset, 0,
                    "must rest at the origin at {t}ms, wrap_width={wrap_width}"
                );
            }
        }
    }

    #[test]
    fn octopus_swim_traces_a_symmetric_trapezoid_while_paddling() {
        // Sampled through one full cycle: offset rises monotonically to MAX,
        // rests, falls monotonically back, rests at the origin — mirror-
        // symmetric around the cycle — while the paddle stroke alternates
        // every OCTOPUS_STROKE_MS throughout.
        let wrap_width = 120usize;
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        let max = wrap_width.saturating_sub(octopus_width + 1);
        assert!(
            max > 28,
            "the sweep must exceed the old 28-column cap (got MAX={max})"
        );

        let leg = OCTOPUS_SWEEP_ONE_WAY_MS + OCTOPUS_EDGE_DWELL_MS;
        let cycle_ms = 2 * leg;
        let mut previous = None;
        for t in (0..=cycle_ms).step_by(50) {
            let (offset, frame) = octopus_swim(t, wrap_width);
            assert!(offset <= max, "offset {offset} exceeded MAX {max} at {t}ms");
            // Mirror symmetry around the far-edge rest: t and (cycle - DWELL
            // - t) sit at the same height on opposite legs.
            if t + OCTOPUS_EDGE_DWELL_MS <= cycle_ms {
                let (mirrored, _) = octopus_swim(cycle_ms - OCTOPUS_EDGE_DWELL_MS - t, wrap_width);
                assert_eq!(offset, mirrored, "trapezoid asymmetric at {t}ms");
            }
            // Monotone rise, then never rising again until the origin rest.
            if let Some((prev_t, prev_offset)) = previous {
                if t <= OCTOPUS_SWEEP_ONE_WAY_MS {
                    assert!(
                        offset >= prev_offset,
                        "rising leg regressed between {prev_t}ms and {t}ms"
                    );
                } else if prev_t >= OCTOPUS_SWEEP_ONE_WAY_MS {
                    assert!(
                        offset <= prev_offset,
                        "post-peak the offset must never climb ({prev_t}ms → {t}ms)"
                    );
                }
            }
            // Stroke follows the global clock, independent of position.
            assert_eq!(
                frame,
                OCTOPUS_SWIM_FRAMES[((t / OCTOPUS_STROKE_MS) % 2) as usize],
                "paddle stroke at {t}ms"
            );
            previous = Some((t, offset));
        }
        // The next cycle starts back at the origin.
        let (wrapped, _) = octopus_swim(cycle_ms, wrap_width);
        assert_eq!(wrapped, 0, "cycle wraps to the origin");
    }

    #[test]
    fn octopus_swim_never_overflows_the_wrap_width() {
        // The octopus (plus a one-column right margin) always stays inside
        // the wrap boundary across full cycles, for a range of widths — and
        // reaches the far edge on every one of them (full-width travel).
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        let cycle_ms = 2 * (OCTOPUS_SWEEP_ONE_WAY_MS + OCTOPUS_EDGE_DWELL_MS);
        for wrap_width in [octopus_width + 2, 20, 40, 80, 200, 1000] {
            let max = wrap_width.saturating_sub(octopus_width + 1);
            let mut peak = 0usize;
            for t in (0..cycle_ms).step_by(25) {
                let (offset, _frame) = octopus_swim(t, wrap_width);
                assert!(
                    offset + octopus_width <= wrap_width,
                    "octopus overflowed wrap_width={wrap_width}: offset={offset}",
                );
                peak = peak.max(offset);
            }
            assert_eq!(peak, max, "far edge at wrap_width={wrap_width}");
        }
    }

    #[test]
    fn octopus_swim_tiny_terminal_paddles_in_place_without_panicking() {
        // A terminal too narrow to travel: MAX collapses to 0, so the octopus
        // holds the left margin — still paddling — instead of panicking or
        // wrapping.
        let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
        for wrap_width in [0usize, 1, 2, octopus_width, octopus_width + 1] {
            // A large elapsed value also exercises the u128 math safely.
            let big = 9_999_999_999u128;
            let (offset, frame) = octopus_swim(big, wrap_width);
            assert_eq!(offset, 0, "no travel at wrap_width={wrap_width}");
            assert_eq!(
                frame,
                OCTOPUS_SWIM_FRAMES[((big / OCTOPUS_STROKE_MS) % 2) as usize],
                "keeps paddling in place"
            );
            // The next stroke interval still alternates while parked.
            let (_, next) = octopus_swim(big + OCTOPUS_STROKE_MS, wrap_width);
            assert_ne!(frame, next, "parked octopus must keep alternating strokes");
        }
    }

    #[test]
    fn segmented_reply_still_renders_a_trailing_summary_as_a_card() {
        // The native-scrollback segmented path (tool-backed replies) must also
        // give a trailing Session Summary the card treatment, not flat
        // markdown (codex P2 round 2 on #292).
        let summary = t!(
            "status.summary_partial_answer",
            count = 2,
            files = "none observed",
            validation = "not reported",
        )
        .into_owned();
        // A reply with an internal segment boundary (as a tool call inserts),
        // then the appended summary.
        let body = "First I ran a tool.\n\nThen I continued.";
        let content = format!("{body}\n\n{summary}");
        let boundaries = vec![body.find("\n\nThen").unwrap()];

        let palette = Palette::for_theme(ThemeName::Codex);
        let mut lines = Vec::new();
        push_committed_assistant_reply_segments(&mut lines, palette, &content, 120, &boundaries);
        let text = lines_text(&lines);
        assert!(
            text.contains("First I ran a tool"),
            "prose body renders: {text:?}"
        );
        assert!(
            text.contains("✦"),
            "the trailing summary gets the card glyph: {text:?}"
        );
    }

    #[test]
    fn session_summary_detected_as_a_suffix_after_partial_prose() {
        // The partial-completion path appends the summary AFTER the model's
        // partial reply (`{prose}\n\n{summary}`), so the title is NOT the
        // first line — detection must still find it (codex P2 on #292).
        let summary = t!(
            "status.summary_partial_answer",
            count = 3,
            files = "none observed",
            validation = "not reported",
        )
        .into_owned();
        let content = format!("Emulator installed. Booting the AVD now:\n\n{summary}");

        let start = session_summary_block_start(&content)
            .expect("summary suffix must be detected after prose");
        assert!(start > 0, "the block starts after the prose, not at 0");
        assert_eq!(
            &content[start..start + summary.len().min(20)],
            &summary[..summary.len().min(20)]
        );

        let palette = Palette::for_theme(ThemeName::Codex);
        let mut lines = Vec::new();
        push_message_block(&mut lines, palette, "assistant", &content, 120);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.contains("Emulator installed"), "prose still renders");
        assert!(
            text.contains("✦"),
            "the appended summary still gets the card treatment"
        );
    }

    #[test]
    fn session_summary_detection_is_locale_independent() {
        // A card stored in English must still be recognized after a `/lang`
        // switch to Chinese, and vice-versa (codex P2 on #292).
        let en = t!("status.summary_title", locale = "en").into_owned();
        let zh = t!("status.summary_title", locale = "zh").into_owned();
        let en_card = format!("{en}\n- Result: done.");
        let zh_card = format!("{zh}\n- 结果：完成。");
        assert_eq!(session_summary_block_start(&en_card), Some(0));
        assert_eq!(session_summary_block_start(&zh_card), Some(0));
    }

    #[test]
    fn session_summary_rows_never_exceed_a_narrow_pane() {
        // A 24-col pane: the `  - Risks / follow-up: ` prefix alone is wide,
        // so the value budget goes to zero — the clip backstop must keep every
        // emitted row within width instead of wrapping to column 0 (codex P2).
        let content = t!(
            "status.summary_failed",
            code = "runtime_error",
            message = "a fairly long error message that would overflow a narrow pane",
            count = 20,
            failed = "none recorded",
        )
        .into_owned();
        let palette = Palette::for_theme(ThemeName::Codex);
        let mut lines = Vec::new();
        push_message_block(&mut lines, palette, "assistant", &content, 24);
        for line in &lines {
            let cols: usize = line
                .spans
                .iter()
                .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                .sum();
            assert!(cols <= 24, "row exceeds 24 cols: {cols} — {line:?}");
        }
    }

    #[test]
    fn session_summary_card_gets_a_highlighted_title_and_labels() {
        // The synthesized failure "Session Summary" message renders as a
        // distinct card — a highlight-colored bold title and bold field
        // labels, with the error value in the danger color — instead of flat
        // muted markdown (user report: "need highlights and color the title").
        let content = t!(
            "status.summary_failed",
            code = "runtime_error",
            message = "failed to send streaming request to Anthropic",
            count = 20,
            failed = "none recorded",
        )
        .into_owned();

        assert_eq!(
            session_summary_block_start(&content),
            Some(0),
            "the failure template starts a summary card at offset 0"
        );

        let palette = Palette::for_theme(ThemeName::Codex);
        let bg = chat_message_bg(palette, "assistant");
        let mut lines = Vec::new();
        push_message_block(&mut lines, palette, "assistant", &content, 120);

        // Title row: bold + highlight color, prefixed with the ✦ notice glyph.
        let title_line = lines
            .iter()
            .find(|line| {
                line.spans
                    .iter()
                    .any(|s| s.content.contains("Session Summary"))
            })
            .expect("title line");
        let title_span = title_line
            .spans
            .iter()
            .find(|s| s.content.contains("Session Summary"))
            .unwrap();
        assert!(
            title_span.content.contains('✦'),
            "title carries the ✦ notice glyph"
        );
        assert_eq!(
            title_span.style.fg,
            Some(palette.highlight),
            "title is highlight-colored"
        );
        assert!(
            title_span.style.add_modifier.contains(Modifier::BOLD),
            "title is bold"
        );

        // The Error label is bold and its value is danger-colored.
        let error_line = lines
            .iter()
            .find(|line| line.spans.iter().any(|s| s.content.starts_with("Error")))
            .expect("error line");
        let label_span = error_line
            .spans
            .iter()
            .find(|s| s.content.starts_with("Error"))
            .unwrap();
        assert!(
            label_span.style.add_modifier.contains(Modifier::BOLD),
            "the Error label is bold"
        );
        let value_span = error_line
            .spans
            .iter()
            .find(|s| s.content.contains("failed to send"))
            .expect("error value span");
        assert_eq!(
            value_span.style.fg,
            Some(palette.danger),
            "the error value is danger-colored"
        );
        let _ = bg;
    }

    #[test]
    fn render_composer_collapses_home_dir_in_cwd_footer() {
        // Build the cwd from the runner's actual HOME so the assertion is
        // deterministic across machines and exercises the real render path.
        let Some(home) = std::env::var_os("HOME")
            .and_then(|home| home.into_string().ok())
            .map(|home| home.trim_end_matches('/').to_string())
            .filter(|home| !home.is_empty())
        else {
            return; // no HOME → collapsing is a documented no-op, nothing to assert
        };
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let cwd = format!("{home}/proj/octos");
        app.workspace.root = cwd.clone();
        app.set_runtime_status(runtime_status_with_model_cwd(session_id, "kimi-k2", &cwd));

        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);
        let footer = row_containing(&rows, "kimi-k2");

        assert!(
            footer.contains("~/proj/octos"),
            "composer cwd should collapse the home dir to ~; got {footer:?}"
        );
        assert!(
            !footer.contains(&home),
            "raw home dir must not leak once collapsed to ~; got {footer:?}"
        );
    }

    #[test]
    fn render_composer_shows_selected_catalog_model_without_runtime_status() {
        // When no session/status/read has landed yet (no runtime status), the
        // footer still shows the model the `/model` catalog marks selected.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/octos-workspace".into();
        app.set_model_catalog(SessionModelCatalog {
            session_id,
            models: vec![
                ModelStatus {
                    model: "deepseek-v4-pro".into(),
                    provider: "deepseek".into(),
                    title: None,
                    family: None,
                    route: None,
                    selected: true,
                    available: Some(true),
                    queue_mode: None,
                    qoe_policy: None,
                },
                ModelStatus {
                    model: "gpt-5".into(),
                    provider: "openai".into(),
                    title: None,
                    family: None,
                    route: None,
                    selected: false,
                    available: Some(true),
                    queue_mode: None,
                    qoe_policy: None,
                },
            ],
        });
        assert!(
            app.runtime_status_for(&SessionKey("local:test".into()))
                .is_none()
        );

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let footer = row_containing(&rows, "/srv/octos-workspace");
        assert!(
            footer.contains("deepseek-v4-pro"),
            "footer should fall back to the catalog's selected model; got {footer:?}"
        );
        assert!(
            !footer.contains("gpt-5"),
            "only the selected catalog model belongs on the footer; got {footer:?}"
        );
    }

    #[test]
    fn status_bar_shows_waiting_while_an_approval_or_question_is_pending() {
        // A turn parked on an approval (or AskUserQuestion) is not "Working" —
        // the agent is waiting on the OPERATOR. The state segment must say so,
        // and flip back to Working once the decision is resolved.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.run_state = SessionRunState::InProgress;

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            status_row.contains("Working"),
            "in-progress without a pending decision stays Working: {status_row:?}"
        );

        // The REAL arrival path parks run_state at Blocked (codex P1: the
        // InProgress-only gate never fired on the actual flow).
        app.run_state = SessionRunState::Blocked {
            message: "Run command".into(),
        };
        app.approval = Some(ApprovalModalState {
            session_id: session_id.clone(),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run command".into(),
            body: "approve?".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            status_row.contains("Waiting"),
            "pending approval must read Waiting: {status_row:?}"
        );
        assert!(
            !status_row.contains("Working"),
            "Waiting replaces Working: {status_row:?}"
        );

        // Even a hidden (collapsed) approval modal is still a parked turn.
        if let Some(approval) = app.approval.as_mut() {
            approval.visible = false;
        }
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            status_row.contains("Waiting"),
            "collapsed-but-pending approval still Waiting: {status_row:?}"
        );

        // Another session's decision must NOT mark this one waiting: with
        // the modal re-keyed to a different session the label falls back to
        // the plain run_state (Blocked here).
        if let Some(approval) = app.approval.as_mut() {
            approval.session_id = SessionKey("local:other".into());
        }
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            !status_row.contains("Waiting"),
            "another session's decision must not read Waiting here: {status_row:?}"
        );

        // Resolved -> back to the plain run_state display.
        app.approval = None;
        app.run_state = SessionRunState::InProgress;
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            status_row.contains("Working"),
            "resolved decision returns to Working: {status_row:?}"
        );
    }

    #[test]
    fn status_work_text_advertises_recovery_keys_when_a_decision_is_pending() {
        // Regression: a turn parked on an approval/question locks the composer and
        // its card can scroll off the clipped live tail — leaving a bare "Waiting"
        // with only the (two-step) Esc hint. The work text must instead advertise
        // Ctrl+R/Alt+A (bring the prompt back) and Ctrl+C (one-press interrupt).
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "t".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.run_state = SessionRunState::Blocked {
            message: "Run command".into(),
        };
        // No pending decision → no recovery hint.
        assert!(!status_bar_work_text(&app).contains("Ctrl+R/Alt+A"));

        // Park the active session on an approval (even collapsed/hidden).
        app.approval = Some(ApprovalModalState {
            session_id: session_id.clone(),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run".into(),
            body: "approve?".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: false,
        });
        let work = status_bar_work_text(&app);
        assert!(
            work.contains("Ctrl+R/Alt+A") && work.contains("Ctrl+C"),
            "a parked decision must advertise the recovery keys: {work:?}"
        );
        assert!(
            !work.contains("Esc interrupt"),
            "the dead-end Esc hint is replaced while a decision is pending: {work:?}"
        );

        // A decision on ANOTHER session must not hijack this session's hint.
        if let Some(approval) = app.approval.as_mut() {
            approval.session_id = SessionKey("local:other".into());
        }
        assert!(!status_bar_work_text(&app).contains("Ctrl+R/Alt+A"));
    }

    #[test]
    fn status_bar_does_not_duplicate_the_composer_footer_cwd() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/octos-workspace".into();

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));

        // The status bar (below the composer) must NOT repeat the cwd — it now
        // lives on the composer's bottom border, and repeating it one line below
        // read as clutter.
        let status_row = row_containing(&rows, "approval gated");
        assert!(
            !status_row.contains("/srv/octos-workspace"),
            "status bar should not duplicate the cwd now on the composer border; got {status_row:?}"
        );
        // ...but the cwd is still shown once (on the composer border).
        assert!(
            rows.iter().any(|row| row.contains("/srv/octos-workspace")),
            "cwd should still appear once, on the composer border"
        );
    }

    #[test]
    fn unflushed_activity_section_still_emits_turn_summary() {
        // Regression: an orchestrated turn's activity log is still covered by the
        // live tail at the settling flush, so it routes through the UNFLUSHED
        // section renderer with its items already flushed (empty here). The
        // committed status report must still land in scrollback.
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do it"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.attach_turn_summary(&session_id, &turn_id, 75, 1);
        let log = crate::model::TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request: Some("do it".into()),
            anchor_index: None,
            items: vec![],
        };

        let palette = Palette::for_theme(ThemeName::Codex);
        let mut lines = Vec::new();
        let coverage = LiveTurnFinalization::new(&session_id, &turn_id);
        push_turn_activity_log_section_unflushed(&mut lines, palette, &log, &app, &coverage, 80);

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert!(
            text.contains("✻ Ran for 1m 15s · 1 background task(s) still running"),
            "unflushed section must still emit the turn summary; got: {text:?}"
        );
    }

    #[test]
    fn turn_summary_text_formats_duration_and_running_tasks() {
        let with_tasks = TurnActivitySummary {
            session_id: SessionKey("local:test".into()),
            turn_id: TurnId::new(),
            elapsed_secs: 319,
            background_tasks: 2,
        };
        assert_eq!(
            turn_summary_text(&with_tasks),
            "✻ Ran for 5m 19s · 2 background task(s) still running"
        );

        let no_tasks = TurnActivitySummary {
            session_id: SessionKey("local:test".into()),
            turn_id: TurnId::new(),
            elapsed_secs: 8,
            background_tasks: 0,
        };
        assert_eq!(turn_summary_text(&no_tasks), "✻ Ran for 8s");
    }

    #[test]
    fn transcript_renders_turn_summary_line_after_completed_turn() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do the thing"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // A completed turn with one background task still running. No activity
        // log items — attach_turn_summary must synthesize a log so the report
        // still renders after the assistant reply.
        app.attach_turn_summary(&session_id, &turn_id, 75, 1);

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("✻ Ran for 1m 15s · 1 background task(s) still running"),
            "transcript should carry the committed turn status report; got:\n{text}"
        );
    }

    #[test]
    fn settled_session_keeps_rendering_btw_aside() {
        // Regression (live soak): the live tail gates on should_show_turn_flow,
        // which went false once the session settled — the aside card vanished
        // the moment the main turn completed, often before the answer landed.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "still with me?".into());
        // The aside now renders as a floating overlay, no longer gated on the
        // turn flow — so it stays visible even after the session settles.
        let palette = Palette::for_theme(ThemeName::Codex);
        let text = rendered_rows(&rendered_buffer(&app, palette)).join("\n");
        assert!(
            text.contains("/btw still with me?"),
            "settled session must still render the aside overlay; got:\n{text}"
        );
        // The pane chrome is load-bearing: without the titled border the
        // overlay reads as embedded transcript text, not its own window.
        assert!(
            text.contains("Aside — /btw"),
            "aside must render as a titled pane, not bare lines; got:\n{text}"
        );
        assert!(
            text.contains("┌") && text.contains("└"),
            "aside pane must draw its border; got:\n{text}"
        );
    }

    /// codex P1 (merge reconcile): the aside no longer contributes lines to
    /// the turn flow, so a SETTLED session's live tail collapses to 1-2 rows —
    /// under `render_btw_overlay`'s 3-row minimum — and the overlay became
    /// invisible in the real inline viewport (state kept it, nothing drew it).
    /// The tail height hint must reserve the overlay's rows.
    #[test]
    fn btw_aside_overlay_survives_settled_inline_viewport() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "still with me?".into());
        // Real inline-viewport path: the viewport is sized by live_ui_height
        // (a settled tail otherwise reserves ~1 row) and the overlay draws
        // over the tail's top rows.
        let text = viewport_rows(&app, 100, 40).join("\n");
        assert!(
            text.contains("Aside — /btw"),
            "settled inline viewport must still draw the aside pane; got:\n{text}"
        );
        assert!(
            text.contains("/btw still with me?"),
            "aside question echo missing from inline viewport; got:\n{text}"
        );
    }

    fn app_with_long_btw_answer() -> (AppState, SessionKey) {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(
            &session_id,
            "tell me more about what you are working on".into(),
        );
        let answer = "I'm working on integrating Astro into your World Cup 2026 frontend to provide better component-based architecture. The idea is to use Astro as a meta-framework wrapping your existing React islands.\n\nWhat's been done so far:\n- Researched Astro's React integration docs\n- Set up an Astro project alongside your existing React app\n- Got Astro to build successfully\n\nCurrent blocker: The Astro SSR pages try to fetch data from your GraphQL server at localhost:4000 during build time, but this sandbox environment blocks outbound network so the build data step fails.\n\nLikely next step: Switching the Astro pages to use client-side fetching instead of SSR fetch, so the browser does the GraphQL call at runtime instead of the build doing it.";
        app.resolve_btw_answer(&session_id, answer.into());
        (app, session_id)
    }

    #[test]
    fn btw_overlay_grows_to_fit_a_long_answer_on_a_tall_terminal() {
        // Regression: the aside was capped at half the viewport, so a long
        // answer stranded its last line behind a scroll even with screen space
        // to spare (reported: "…gives you a proper" cut off). It must now grow
        // to show the WHOLE answer and drop the scroll indicator.
        let (app, _session_id) = app_with_long_btw_answer();
        // Height 30 at width 100: the answer wraps to more than half of 30 (the
        // old cap), so the old code stranded its tail behind a scroll — but it
        // fits comfortably within the full viewport minus composer + scrollback.
        let text = viewport_rows(&app, 100, 30).join("\n");
        assert!(
            text.contains("Likely next step"),
            "the tail paragraph must render; got:\n{text}"
        );
        assert!(
            text.contains("instead of the build doing it."),
            "the FINAL sentence must be fully visible, not stranded; got:\n{text}"
        );
        assert!(
            !text.contains("PgUp/PgDn"),
            "a fitting answer must not show a scroll indicator when it fits; got:\n{text}"
        );
    }

    #[test]
    fn btw_overlay_wraps_long_prose_instead_of_clipping() {
        let (app, _session_id) = app_with_long_btw_answer();
        // Tall terminal: the whole answer fits, so nothing is clipped and no
        // scroll indicator appears.
        let text = viewport_rows(&app, 100, 44).join("\n");
        // The overflowing word ("component-based") wraps to a following row
        // rather than being hard-cut at the border mid-word.
        assert!(
            text.contains("component-based architecture"),
            "long prose must wrap intact, not clip mid-word; got:\n{text}"
        );
        // The tail paragraphs (previously dropped) are now visible in full.
        assert!(
            text.contains("Likely next step"),
            "content below the fold must render when it fits; got:\n{text}"
        );
        assert!(
            !text.contains("PgUp/PgDn"),
            "no scroll indicator when everything fits; got:\n{text}"
        );
    }

    #[test]
    fn btw_overlay_scrolls_when_taller_than_the_pane() {
        let (mut app, session_id) = app_with_long_btw_answer();
        // Short terminal: the pane is capped at half the viewport, so the answer
        // can't fit — a position indicator must appear instead of silent drops.
        let top = viewport_rows(&app, 100, 20).join("\n");
        assert!(
            top.contains("PgUp/PgDn"),
            "a too-tall answer must show a scroll indicator; got:\n{top}"
        );
        assert!(
            top.contains("I'm working on integrating Astro"),
            "unscrolled overlay starts at the top; got:\n{top}"
        );
        assert!(
            !top.contains("Likely next step"),
            "the tail is below the fold before scrolling; got:\n{top}"
        );

        // Scroll down: the window moves to reveal lower content.
        app.nudge_btw_scroll(&session_id, 12);
        let scrolled = viewport_rows(&app, 100, 20).join("\n");
        assert!(
            scrolled.contains("Likely next step"),
            "scrolling must reveal content below the fold; got:\n{scrolled}"
        );
    }

    #[test]
    fn btw_aside_card_renders_answering_then_answer() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do the thing"), Message::assistant("on it")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_btw_answering(&session_id, "what are you working on?".into());

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("/btw what are you working on?"),
            "aside question echo missing:\n{text}"
        );
        assert!(
            text.contains("✽ Answering…"),
            "answering indicator missing:\n{text}"
        );

        assert!(
            app.resolve_btw_answer(&session_id, "Refactoring the parser.".into()),
            "answer resolves the answering aside"
        );
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let text = rows.join("\n");
        assert!(
            text.contains("Refactoring the parser."),
            "answer block missing:\n{text}"
        );
        assert!(
            !text.contains("✽ Answering…"),
            "answering indicator must clear once answered:\n{text}"
        );
    }

    #[test]
    fn collapse_home_prefix_replaces_home_with_tilde() {
        assert_eq!(
            collapse_home_prefix("/Users/me/proj/octos", Some("/Users/me")),
            "~/proj/octos"
        );
        // Exact home collapses to a bare ~.
        assert_eq!(collapse_home_prefix("/Users/me", Some("/Users/me")), "~");
        // A trailing slash on HOME is tolerated.
        assert_eq!(
            collapse_home_prefix("/Users/me/x", Some("/Users/me/")),
            "~/x"
        );
    }

    #[test]
    fn collapse_home_prefix_only_matches_on_path_boundary() {
        // `/Users/mentor` shares a textual prefix with `/Users/me` but is NOT a
        // subdirectory — it must be left untouched.
        assert_eq!(
            collapse_home_prefix("/Users/mentor/x", Some("/Users/me")),
            "/Users/mentor/x"
        );
        // Absent/empty HOME is a no-op.
        assert_eq!(collapse_home_prefix("/Users/me/x", None), "/Users/me/x");
        assert_eq!(collapse_home_prefix("/Users/me/x", Some("")), "/Users/me/x");
    }

    #[test]
    fn collapse_home_prefix_handles_windows_separators() {
        // Native Windows paths use `\` as the boundary (USERPROFILE homes).
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me\proj", Some(r"C:\Users\me")),
            r"~\proj"
        );
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me", Some(r"C:\Users\me")),
            "~"
        );
        // Trailing backslash on the home is tolerated; boundary still enforced.
        assert_eq!(
            collapse_home_prefix(r"C:\Users\me\x", Some(r"C:\Users\me\")),
            r"~\x"
        );
        assert_eq!(
            collapse_home_prefix(r"C:\Users\mentor\x", Some(r"C:\Users\me")),
            r"C:\Users\mentor\x"
        );
    }

    #[test]
    fn composer_footer_prefers_session_workspace_root_over_global() {
        // A canonicalized/global `workspace.root` must not shadow the ACTIVE
        // session's server-confirmed workspace (from session/status/read) —
        // switching between sessions with different workspaces shows each
        // session's own root.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/global-root".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "kimi-k2",
            "/srv/session-root",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));
        let footer = row_containing(&rows, "kimi-k2");
        assert!(
            footer.contains("/srv/session-root"),
            "footer should show the session's server-confirmed workspace root; got {footer:?}"
        );
        assert!(
            !footer.contains("/srv/global-root"),
            "the global workspace root must not shadow the session's; got {footer:?}"
        );
    }

    #[test]
    fn composer_footer_keeps_model_and_drops_cwd_when_too_narrow_for_both_titles() {
        // Ratatui paints overlapping border titles over each other; when the
        // composer cannot fit cwd + model side by side, the cwd is dropped and
        // the MODEL is kept — never a collision. The model is the footer's sole
        // persistent home now that the status line no longer echoes it, so it
        // must never be the title that vanishes.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.workspace.root = "/srv/quite/long/workspace/path/here".into();
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id,
            "moonshotai-kimi-k2-instruct",
            "/srv/quite/long/workspace/path/here",
        ));

        let palette = Palette::for_theme(ThemeName::Codex);
        let narrow = rendered_rows(&rendered_buffer_with_size(&app, palette, 40, 30));
        assert!(
            narrow.iter().any(|row| row.contains("kimi")),
            "model must be kept when both footer titles cannot fit; got {narrow:?}"
        );
        assert!(
            !narrow.iter().any(|row| row.contains("workspace/path")),
            "cwd must be dropped when both footer titles cannot fit; got {narrow:?}"
        );
        let wide = rendered_rows(&rendered_buffer_with_size(&app, palette, 120, 30));
        assert!(
            wide.iter().any(|row| row.contains("kimi")),
            "model still renders when the composer is wide enough"
        );
        assert!(
            wide.iter().any(|row| row.contains("workspace/path")),
            "cwd renders alongside the model once the composer is wide enough"
        );
    }

    fn row_index_containing(rows: &[String], needle: &str) -> usize {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("row containing {needle:?}"))
    }

    fn style_for_text(buffer: &Buffer, needle: &str) -> Option<Style> {
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        for y in 0..height {
            let row_start = y * width;
            let row = buffer.content[row_start..row_start + width]
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            if let Some(x) = row.find(needle) {
                let cell = &buffer.content[row_start + x];
                return Some(
                    Style::default()
                        .fg(cell.fg)
                        .bg(cell.bg)
                        .add_modifier(cell.modifier),
                );
            }
        }
        None
    }

    fn app_with_diff(result: DiffPreviewGetResult) -> AppState {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "diff".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.diff_preview.apply_result(result);
        app
    }

    #[test]
    fn gradient_sample_lerps_endpoints_and_midpoint() {
        let stops = [(0u8, 0u8, 0u8), (100u8, 200u8, 40u8)];
        assert_eq!(gradient_sample(&stops, 0.0), (0, 0, 0));
        assert_eq!(gradient_sample(&stops, 1.0), (100, 200, 40));
        assert_eq!(gradient_sample(&stops, 0.5), (50, 100, 20));
        // Out-of-range clamps; degenerate stop lists don't panic.
        assert_eq!(gradient_sample(&stops, 2.0), (100, 200, 40));
        assert_eq!(gradient_sample(&[(7, 7, 7)], 0.5), (7, 7, 7));
    }

    #[test]
    fn wave_gradient_spans_colors_each_grapheme_and_animates() {
        let stops = [(0u8, 0u8, 0u8), (255u8, 255u8, 255u8)];
        let a = wave_gradient_spans("abc", 0.0, &stops, Color::Reset);
        assert_eq!(a.len(), 3, "one span per grapheme");
        assert!(
            a.iter()
                .all(|s| matches!(s.style.fg, Some(Color::Rgb(_, _, _)))),
            "every glyph gets a truecolor fg"
        );
        // Advancing the phase moves the crest → the first glyph recolors.
        let b = wave_gradient_spans("abc", 1.5, &stops, Color::Reset);
        assert_ne!(a[0].style.fg, b[0].style.fg, "the wave advances with phase");
        // CJK double-width graphemes still produce one span each.
        assert_eq!(
            wave_gradient_spans("水波", 0.0, &stops, Color::Reset).len(),
            2
        );
    }

    #[test]
    fn render_default_view_is_coding_session_first() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "artifact task".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: "artifact log line\n".into(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "Mock backend ready".into(),
            Some("local mock snapshot".into()),
            false,
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Octos TUI"));
        assert!(!text.contains("Protocol session"));
        assert!(!text.contains("ws://"));
        assert!(!text.contains("Transcript"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Tab agents"));
        assert!(!text.contains("Current Tasks"));
        assert!(!text.contains("tasks/status"));
        assert!(!text.contains("Sessions"));
        assert!(!text.contains("Artifacts"));
        assert!(!text.contains("Workspace"));
        assert!(!text.contains("Git"));
        assert!(!text.contains("INFO calling LLM"));
        assert!(!text.contains("parallel_tools"));
        assert!(!text.contains("tool_ids="));
    }

    #[test]
    fn render_artifact_detail_modal_shows_content() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.artifact_detail = crate::model::ArtifactDetailState {
            active: true,
            title: "notes.md".into(),
            subtitle: "agent ag-7 | markdown | ready".into(),
            content: "artifact body".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Artifact"));
        assert!(text.contains("notes.md"));
        assert!(text.contains("agent ag-7"));
        assert!(text.contains("artifact body"));
    }

    #[test]
    fn render_thread_graph_detail_modal_shows_threads() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.thread_graph_detail = crate::model::ThreadGraphDetailState {
            active: true,
            title: "Thread Graph".into(),
            subtitle: "1 thread(s) @ session:7".into(),
            content: "thread-1 | active | root seq 1 | 2 message(s)".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Threads"));
        assert!(text.contains("Thread Graph"));
        assert!(text.contains("thread-1"));
        assert!(text.contains("root seq 1"));
    }

    #[test]
    fn render_turn_state_detail_modal_shows_lifecycle() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_state_detail = crate::model::TurnStateDetailState {
            active: true,
            title: "Turn State".into(),
            subtitle: "turn 00000000-0000-0000-0000-000000000011".into(),
            content: "state: active\nthread: thread-1\ncommitted seqs: 1, 2".into(),
            scroll: 0,
        };

        let text = rendered_text(&app);

        assert!(text.contains("Turn"));
        assert!(text.contains("Turn State"));
        assert!(text.contains("state: active"));
        assert!(text.contains("thread-1"));
        assert!(text.contains("committed seqs"));
    }

    #[test]
    fn render_inspector_view_includes_m9_panes_without_hiding_chat() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "artifact task".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: "artifact log line\n".into(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "Mock backend ready".into(),
            Some("local mock snapshot".into()),
            false,
        );
        app.focus = FocusPane::Sessions;

        let text = rendered_text(&app);

        assert!(text.contains("Sessions"));
        assert!(text.contains("Tasks"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Artifacts"));
        assert!(text.contains("Workspace"));
        assert!(text.contains("Git"));
        assert!(text.contains("artifact task output tail"));
        assert!(text.contains("m9.7/mock-snapshot"));
        assert!(text.contains("api octos-app-ui/v1alpha1"));
        assert!(!text.contains("INFO calling LLM"));
        assert!(!text.contains("parallel_tools"));
        assert!(!text.contains("tool_ids="));
    }

    /// #337: `/ps` (focus == Tasks) renders the dedicated two-pane DOCK — the
    /// Tasks pane + transcript only — NOT the busy six-pane inspector. So the
    /// Tasks pane shows, but the Workspace/Git panes do not.
    #[test]
    fn ps_focus_renders_dedicated_dock_not_full_inspector() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "pipeline task".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.focus = FocusPane::Tasks; // what `/ps` sets

        let text = rendered_text(&app);

        // The dock shows the Tasks pane + transcript...
        assert!(text.contains("Tasks"), "dock must show the Tasks pane");
        assert!(text.contains("pipeline task"), "dock must list the task");
        // ...but NOT the other inspector panes.
        assert!(
            !text.contains("Workspace"),
            "the /ps dock must not show the Workspace pane; got:\n{text}"
        );
        assert!(
            !text.contains("Git  status"),
            "the /ps dock must not show the Git pane; got:\n{text}"
        );
    }

    #[test]
    fn render_chat_roles_use_gutter_anchor_and_distinct_styles() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::system("system secret should stay hidden"),
                    Message::user("please fix bubble colors"),
                    Message::assistant("done with bubble colors"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("please fix bubble colors"));
        assert!(text.contains("done with bubble colors"));
        assert!(!text.contains("system secret"));
        assert!(!text.contains("you    │"));
        assert!(!text.contains("octos  │"));
        assert!(!text.contains("system │"));

        let user_style = style_for_text(&buffer, "please fix bubble colors").expect("user style");
        let assistant_style =
            style_for_text(&buffer, "done with bubble colors").expect("assistant style");

        // Role-contrast contract: the user's words are the transcript's anchor
        // — accent gutter + bold body, NO bubble background (backgrounds are
        // unreliable in the pager / terminal theme / native scrollback).
        assert!(text.contains("▌ please fix bubble colors"));
        assert!(user_style.add_modifier.contains(Modifier::BOLD));
        assert_ne!(user_style.bg, Some(palette.diff_context_bg));
        // Assistant prose keeps its existing baseline rendering.
        assert_eq!(assistant_style.bg, Some(palette.surface));
        assert!(!text.contains("▌ done with bubble colors"));
    }

    #[test]
    fn render_default_view_keeps_turn_plan_in_chat_without_split_work_pane() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- [x] Inspect renderer\n- [ ] Patch sticky plan\n- [ ] Run tests",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("Plan"));
        assert!(text.contains("Inspect renderer"));
        assert!(text.contains("Patch sticky plan"));
        assert!(text.contains("Composer"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("No active plan"));
        assert!(
            row_index_containing(&rows, "Plan") < row_index_containing(&rows, "Composer"),
            "turn plan should stay in chat history above the composer"
        );
    }

    #[test]
    fn render_default_chat_hides_agent_round_plan() {
        let session_id = SessionKey("local:test".into());
        let completed_turn_id = TurnId::new();
        let active_turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("review the project code by code"),
                    Message::assistant("I inspected the first pass."),
                    Message::user("continue the review"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn_id.clone(),
                    text: "Continuing with deeper checks.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: completed_turn_id.clone(),
            request: Some("review the project code by code".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "list_dir", "complete")
                    .with_turn(completed_turn_id.clone())
                    .with_success(true),
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(completed_turn_id)
                    .with_success(true),
            ],
        });
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_turn(active_turn_id)
                .with_success(true),
        );

        let text = rendered_text(&app);

        assert!(text.contains("Continuing with deeper checks."));
        assert!(text.contains("2 completed"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Plan rounds"));
        assert!(!text.contains("Round 1: review the project code by code"));
        assert!(!text.contains("Current round: continue the review"));
    }

    #[test]
    fn render_plan_strips_source_checkboxes_and_marks_completed_live_items() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n1. [x] Inspect renderer\n2. [ ] Patch sticky plan",
                )],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Plan:\n1. [ ] Inspect renderer\n2. [ ] Patch sticky plan".into(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let plan = extract_plan_lines(&app);
        assert_eq!(
            plan,
            vec![
                RenderedPlanStep {
                    text: "Inspect renderer".into(),
                    completed: true,
                },
                RenderedPlanStep {
                    text: "Patch sticky plan".into(),
                    completed: false,
                },
            ]
        );
        let text = rendered_text(&app);

        assert!(text.contains("Inspect renderer"));
        assert!(text.contains("Patch sticky plan"));
        assert!(!text.contains("[ ] 1. [ ] Inspect renderer"));
    }

    #[test]
    fn render_plan_markdown_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- [x] **Hero** — build first viewport\n- [ ] `npm run build`",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Hero"));
        assert!(text.contains("npm run build"));
        assert!(!text.contains("**Hero**"));
        assert!(!text.contains("`npm run build`"));
    }

    #[test]
    fn render_markdown_headings_and_emphasis_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "# What I *can* access:\n\n#### 3.2 *Code Quality* & Maintainability\n\nThis is *available* and `local`.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("What I can access:"));
        assert!(text.contains("3.2 Code Quality & Maintainability"));
        assert!(text.contains("This is available and local."));
        assert!(!text.contains("*can*"));
        assert!(!text.contains("#### 3.2"));
        assert!(!text.contains("*available*"));
        assert!(!text.contains("`local`"));
    }

    #[test]
    fn render_markdown_checkboxes_as_numbered_choices() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "- [x] Point me to a project inside the workspace\n- [x] Share more about what you want reviewed",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("1. Point me to a project inside the workspace"));
        assert!(text.contains("2. Share more about what you want reviewed"));
        assert!(!text.contains("[x]"));
        assert!(!text.contains("[ ]"));
    }

    #[test]
    fn render_diff_preview_stays_in_transcript_before_composer() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Styles patch".into()),
                files: vec![DiffPreviewFile {
                    path: "styles.css".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "body {}".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });
        app.sessions[0].messages = vec![
            Message::user("build the site"),
            Message::assistant("Plan:\n- [x] **Hero**\n- [ ] Instruments"),
        ];
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_detail("styles.css")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let activity = row_index_containing(&rows, "Read");
        let diff = row_index_containing(&rows, "Diff Preview");
        let composer = row_index_containing(&rows, "Composer");

        assert!(
            activity < diff,
            "activity should precede diff in transcript"
        );
        assert!(
            diff < composer,
            "diff preview should stay in transcript above composer"
        );
        assert!(!rows.join("\n").contains("Work  sticky"));
        assert!(!rows.join("\n").contains("Activity"));
        assert!(!rows.join("\n").contains("**Hero**"));
    }

    fn mixed_hunk_line(
        kind: &str,
        content: &str,
        old_line: Option<u32>,
        new_line: Option<u32>,
    ) -> DiffPreviewLine {
        DiffPreviewLine {
            kind: kind.into(),
            content: content.into(),
            old_line,
            new_line,
        }
    }

    /// A hunk mixing context, a paired removed/added change, and a surplus
    /// removed line — the alignment cases side-by-side has to get right.
    fn mixed_hunk_lines() -> Vec<DiffPreviewLine> {
        vec![
            mixed_hunk_line("context", "use std::fmt;", Some(1), Some(1)),
            mixed_hunk_line("removed", "let x = alpha_old;", Some(2), None),
            mixed_hunk_line("removed", "let y = beta_old;", Some(3), None),
            mixed_hunk_line("added", "let x = alpha_new;", None, Some(2)),
            mixed_hunk_line("context", "done();", Some(4), Some(3)),
        ]
    }

    fn mixed_hunk_diff_result() -> DiffPreviewGetResult {
        DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Mixed patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1,4 +1,3 @@".into(),
                        lines: mixed_hunk_lines(),
                    }],
                }],
            },
        }
    }

    #[test]
    fn side_by_side_rows_align_mixed_hunk() {
        let lines = mixed_hunk_lines();
        let rows = side_by_side_rows(&lines);

        assert_eq!(rows.len(), 4, "5 unified lines pair into 4 aligned rows");
        // Context renders on both sides.
        assert_eq!(rows[0].0.expect("left context").content, "use std::fmt;");
        assert_eq!(rows[0].1.expect("right context").content, "use std::fmt;");
        // First removed line pairs with the added line.
        assert_eq!(
            rows[1].0.expect("left removed").content,
            "let x = alpha_old;"
        );
        assert_eq!(
            rows[1].1.expect("right added").content,
            "let x = alpha_new;"
        );
        // Surplus removed line keeps the left column only.
        assert_eq!(
            rows[2].0.expect("left removed").content,
            "let y = beta_old;"
        );
        assert!(rows[2].1.is_none(), "no added line to pair on the right");
        // Trailing context on both sides again.
        assert_eq!(rows[3].0.expect("left context").content, "done();");
        assert_eq!(rows[3].1.expect("right context").content, "done();");
    }

    #[test]
    fn fit_diff_cell_truncates_with_ellipsis_and_pads_to_cell_width() {
        assert_eq!(fit_diff_cell("short", 8), "short   ");
        assert_eq!(fit_diff_cell("exactly8", 8), "exactly8");
        assert_eq!(fit_diff_cell("very long content", 8), "very lo…");
        // A wide char never straddles the boundary; the cell stays exactly
        // `cell` display columns so the column separator keeps aligning.
        let cjk = fit_diff_cell("宽字符内容测试", 8);
        assert_eq!(UnicodeWidthStr::width(cjk.as_str()), 8);
        assert!(cjk.contains('…'));
    }

    /// codex-review (#362): the finalized-scrollback flush
    /// (`insert_history::sanitize_line_in_place`) expands tabs to FOUR
    /// spaces and strips other control chars AFTER the cell was padded to
    /// exact width, so measuring the raw `\t` (0 columns) let tab-bearing
    /// rows grow past the wrap width at insert time, hard-wrap in immutable
    /// native scrollback, and permanently misalign the old|new separator.
    /// Sanitize BEFORE measuring — the same order `finish_hanging_body`
    /// uses for assistant bodies.
    #[test]
    fn fit_diff_cell_expands_tabs_and_strips_controls_before_measuring() {
        // A tab counts as four columns in the width math and leaves no raw
        // `\t` behind for the scrollback sanitizer to widen later.
        assert_eq!(fit_diff_cell("a\tb", 8), "a    b  ");
        // Truncation operates on the EXPANDED text: the leading tab plus
        // three chars already fill the 8-col cell minus the ellipsis.
        assert_eq!(fit_diff_cell("\tabcdef", 8), "    abc…");
        // Other control chars (the ESC introducer here) are stripped,
        // defusing the escape sequence exactly like the sanitizer would.
        assert_eq!(fit_diff_cell("a\u{1b}[31mb", 8), "a[31mb  ");
    }

    /// Row-level guarantee for the scrollback flush: side-by-side rows carry
    /// no raw control characters (so `sanitize_line_in_place` is a no-op on
    /// them) and every row — full pair, tab-indented, or blank-half — shares
    /// one exact width within the wrap budget, the alignment the old│new
    /// separator depends on once rows land in native scrollback.
    #[test]
    fn side_by_side_rows_with_tabs_stay_aligned_after_scrollback_sanitize() {
        let wrap_width = 100usize;
        let hunk = vec![
            mixed_hunk_line("context", "\tfor path in paths {", Some(1), Some(1)),
            mixed_hunk_line("removed", "\t\tvisit(path);", Some(2), None),
            mixed_hunk_line("removed", "\t\tlog(path);", Some(3), None),
            mixed_hunk_line("added", "\t\tvisit(path)?;\u{7f}", None, Some(2)),
        ];
        let mut lines = Vec::new();
        push_diff_hunk_body(
            &mut lines,
            Palette::for_theme(ThemeName::Codex),
            &hunk,
            true,
            wrap_width,
            None,
        );
        assert_eq!(lines.len(), 3, "context + paired change + surplus removed");
        let widths: Vec<usize> = lines
            .iter()
            .map(|line| {
                let text: String = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect();
                assert!(
                    !text.chars().any(char::is_control),
                    "row must carry no raw control characters: {text:?}"
                );
                UnicodeWidthStr::width(text.as_str())
            })
            .collect();
        assert!(
            widths.iter().all(|w| *w == widths[0] && *w <= wrap_width),
            "rows must share one exact width within the wrap budget: {widths:?}"
        );
    }

    /// #362 review: alignment was only ever tested with surplus REMOVED
    /// lines. Pin the opposite direction — surplus ADDED lines keep the
    /// right column with a blank left half.
    #[test]
    fn side_by_side_rows_align_surplus_added_hunk() {
        let hunk = vec![
            mixed_hunk_line("removed", "let a = old();", Some(5), None),
            mixed_hunk_line("added", "let a = new();", None, Some(5)),
            mixed_hunk_line("added", "let extra = more();", None, Some(6)),
        ];
        let rows = side_by_side_rows(&hunk);
        assert_eq!(rows.len(), 2, "1R/2A pairs into 2 rows");
        assert_eq!(rows[0].0.expect("left removed").content, "let a = old();");
        assert_eq!(rows[0].1.expect("right added").content, "let a = new();");
        assert!(rows[1].0.is_none(), "no removed line to pair on the left");
        assert_eq!(
            rows[1].1.expect("surplus added").content,
            "let extra = more();"
        );
    }

    /// #362 review: a fixed `{n:>4}` gutter widened ONLY the rows whose
    /// line numbers reached five digits, shifting their separator out of
    /// column. The gutter now sizes to the hunk's largest line number, so
    /// small- and five-digit-numbered rows share one exact width and one
    /// separator column.
    #[test]
    fn side_by_side_rows_with_five_digit_line_numbers_stay_aligned() {
        let wrap_width = 100usize;
        let hunk = vec![
            mixed_hunk_line("context", "fn before() {}", Some(9998), Some(9999)),
            mixed_hunk_line("removed", "let x = old();", Some(9999), None),
            mixed_hunk_line("added", "let x = new();", None, Some(10001)),
        ];
        assert_eq!(side_by_side_gutter_width(&hunk), 5, "sized to 10001");
        let mut lines = Vec::new();
        push_diff_hunk_body(
            &mut lines,
            Palette::for_theme(ThemeName::Codex),
            &hunk,
            true,
            wrap_width,
            None,
        );
        assert_eq!(lines.len(), 2, "context + paired change");
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        let widths: Vec<usize> = rendered
            .iter()
            .map(|text| UnicodeWidthStr::width(text.as_str()))
            .collect();
        assert!(
            widths.iter().all(|w| *w == widths[0] && *w <= wrap_width),
            "rows must share one exact width: {widths:?}"
        );
        let separator_cols: Vec<usize> = rendered
            .iter()
            .map(|text| {
                let at = text.find(" │ ").expect("separator present");
                UnicodeWidthStr::width(&text[..at])
            })
            .collect();
        assert!(
            separator_cols.iter().all(|c| *c == separator_cols[0]),
            "the old│new separator must sit in one column: {separator_cols:?}"
        );
    }

    /// A hunk tall enough to overflow the collapsed 4-row cap in BOTH view
    /// modes: 10 removed + 10 added lines pair into 10 side-by-side rows
    /// (20 unified rows).
    fn tall_hunk_diff_result() -> DiffPreviewGetResult {
        let mut lines = Vec::new();
        for idx in 0..10u32 {
            lines.push(mixed_hunk_line(
                "removed",
                &format!("old line {idx}"),
                Some(idx + 1),
                None,
            ));
        }
        for idx in 0..10u32 {
            lines.push(mixed_hunk_line(
                "added",
                &format!("new line {idx}"),
                None,
                Some(idx + 1),
            ));
        }
        DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Tall patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/tall.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1,10 +1,10 @@".into(),
                        lines,
                    }],
                }],
            },
        }
    }

    /// codex-review (#362): the collapsed 4-row cap hides PAIRED ROWS in
    /// side-by-side mode (one row holds up to two unified lines), so
    /// reporting the count through the unified "diff line(s)" string
    /// understated what's hidden. Side-by-side counts rows; unified keeps
    /// counting lines.
    #[test]
    fn collapsed_side_by_side_cap_reports_hidden_rows_not_lines() {
        let mut app = app_with_diff(tall_hunk_diff_result());
        app.diff_preview.side_by_side = true;
        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 150, 42);
        let joined = rendered_rows(&buffer).join("\n");
        assert!(
            joined.contains("6 more diff row(s) hidden"),
            "side-by-side reports hidden PAIRED ROWS in row units: {joined}"
        );
        assert!(
            !joined.contains("diff line(s) hidden"),
            "row units replace the understating line units: {joined}"
        );

        let unified = app_with_diff(tall_hunk_diff_result());
        let buffer =
            rendered_buffer_with_size(&unified, Palette::for_theme(ThemeName::Codex), 150, 42);
        let joined = rendered_rows(&buffer).join("\n");
        assert!(
            joined.contains("16 more diff line(s) hidden"),
            "unified still counts hidden unified lines: {joined}"
        );
    }

    #[test]
    fn render_side_by_side_diff_pairs_old_and_new_columns_when_wide() {
        let mut app = app_with_diff(mixed_hunk_diff_result());
        app.diff_preview.side_by_side = true;

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 150, 42);
        let rows = rendered_rows(&buffer);

        let paired = rows
            .iter()
            .find(|row| row.contains("alpha_old"))
            .expect("removed line rendered");
        assert!(
            paired.contains("alpha_new"),
            "side-by-side pairs removed (left) with added (right): {paired:?}"
        );
        assert!(
            paired.find("alpha_old") < paired.find("alpha_new"),
            "old content stays in the left column: {paired:?}"
        );
        assert!(paired.contains('│'), "columns are visually separated");
        let left_only = rows
            .iter()
            .find(|row| row.contains("beta_old"))
            .expect("surplus removed line rendered");
        assert!(
            !left_only.contains("alpha"),
            "surplus removed line keeps a blank right column: {left_only:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("v unified")),
            "footer hint advertises the toggle back to unified"
        );
    }

    #[test]
    fn render_side_by_side_falls_back_to_unified_below_min_width() {
        let mut app = app_with_diff(mixed_hunk_diff_result());
        app.diff_preview.side_by_side = true;

        // Terminal width 101 -> wrap width 99: one column short of the
        // threshold, so the render must fall back to unified rows.
        let narrow = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 101, 42);
        let narrow_rows = rendered_rows(&narrow);
        let removed_row = narrow_rows
            .iter()
            .find(|row| row.contains("alpha_old"))
            .expect("diff rendered");
        assert!(
            !removed_row.contains("alpha_new"),
            "narrow render must fall back to unified rows: {removed_row:?}"
        );
        assert!(
            narrow_rows
                .iter()
                .any(|row| row.contains("needs 100+ cols")),
            "narrow render explains why side-by-side is unavailable"
        );

        // Terminal width 102 -> wrap width 100: exactly at the threshold, the
        // side-by-side request takes effect again.
        let wide = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 102, 42);
        let wide_rows = rendered_rows(&wide);
        let paired = wide_rows
            .iter()
            .find(|row| row.contains("alpha_old"))
            .expect("diff rendered");
        assert!(
            paired.contains("alpha_new"),
            "at the threshold the columns split again: {paired:?}"
        );
    }

    #[test]
    fn render_unified_diff_advertises_side_by_side_toggle_when_wide() {
        let app = app_with_diff(mixed_hunk_diff_result());

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 150, 42);
        let rows = rendered_rows(&buffer);

        let removed_row = rows
            .iter()
            .find(|row| row.contains("alpha_old"))
            .expect("diff rendered");
        assert!(
            !removed_row.contains("alpha_new"),
            "default mode stays unified: {removed_row:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("v side-by-side")),
            "footer hint advertises the side-by-side toggle"
        );
    }

    #[test]
    fn render_turn_anchored_diff_preview_stays_with_original_turn() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let preview_id = PreviewId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("Built the site."),
                    Message::user("done?"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(
                    ActivityKind::Progress,
                    "file_mutation",
                    "File mutation: modify src/styles.css",
                )
                .with_detail("modify src/styles.css | diff preview ready")
                .with_success(true)
                .with_turn(turn_id.clone()),
            ],
        });
        app.diff_preview
            .open_loading_for_turn(preview_id.clone(), Some(turn_id));
        app.diff_preview.apply_result(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id,
                preview_id,
                title: Some("Styles patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/styles.css".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "body {}".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let diff = row_index_containing(&rows, "Diff Preview");
        let latest_prompt = row_index_containing(&rows, "▌ done?");
        let composer = row_index_containing(&rows, "Composer");

        assert!(
            diff < latest_prompt,
            "old diff preview should stay with its original turn, not jump to latest prompt"
        );
        assert!(latest_prompt < composer);
    }

    #[test]
    fn render_inline_approval_shows_diff_choices_without_work_plan() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Plan:\n- one\n- two\n- three\n- four\n- five\n- six",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "diff_edit".into(),
            title: "Apply patch".into(),
            body: "approve?".into(),
            approval_kind: Some(approval_kinds::DIFF.into()),
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Apply patch"));
        assert!(text.contains("y = approve this command once"));
        assert!(text.contains("s = approve this command/scope for the session"));
        assert!(text.contains("n = deny it"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("more plan item(s) | Ctrl+O expand"));
    }

    fn app_with_user_question(questions: Vec<octos_core::ui_protocol::UserQuestion>) -> AppState {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("set up a project")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let event = octos_core::ui_protocol::UserQuestionRequestedEvent::new(
            SessionKey("local:test".into()),
            octos_core::ui_protocol::QuestionId::new(),
            TurnId::new(),
            "Pick a framework",
            "The agent needs your input.",
            questions,
        );
        app.user_question = Some(UserQuestionPickerState::from_event(event));
        app
    }

    fn user_question(
        header: &str,
        question: &str,
        labels: &[&str],
        multi_select: bool,
    ) -> octos_core::ui_protocol::UserQuestion {
        octos_core::ui_protocol::UserQuestion {
            header: header.into(),
            question: question.into(),
            options: labels
                .iter()
                .map(|label| octos_core::ui_protocol::UserQuestionOption {
                    label: (*label).into(),
                    description: String::new(),
                })
                .collect(),
            multi_select,
            allow_free_text: true,
        }
    }

    #[test]
    fn render_inline_single_select_user_question_shows_radios_and_other() {
        let app = app_with_user_question(vec![user_question(
            "Framework",
            "Which web framework?",
            &["axum", "actix"],
            false,
        )]);

        let text = rendered_text(&app);

        assert!(text.contains("Agent asked a question"));
        assert!(text.contains("Pick a framework"));
        assert!(text.contains("Which web framework?"));
        // Single-select uses a hollow radio marker, not a checkbox.
        assert!(text.contains("○ axum"));
        assert!(text.contains("○ actix"));
        assert!(!text.contains("▣ axum")); // not the multi-select marker
        // Prominence: the highlighted row (cursor defaults to the first
        // option) carries the ▌ accent bar; a non-active row does not.
        assert!(text.contains("▌ ○ axum"));
        assert!(!text.contains("▌ ○ actix"));
        // The always-present free-text "Other" row.
        assert!(text.contains("Other"));
        assert!(text.contains("Enter = submit answer(s)"));
    }

    #[test]
    fn fit_card_text_truncates_by_display_columns_not_chars() {
        // Fix #8: CJK glyphs are double-width; measuring chars() let a CJK
        // question option overflow the card. Budget is width - 4 (the caller's
        // 4-space prefix): width 12 -> 8 columns. "中文选项测试" is 6 chars
        // but 12 columns, so it must truncate (with the ellipsis) to <= 8.
        let fitted = fit_card_text("中文选项测试", 12);
        assert_eq!(fitted, "中文选…");
        assert!(fitted.width() <= 8, "fitted {fitted:?} overflows the card");

        // Within-budget text (by COLUMNS) is untouched: ASCII and a CJK string
        // sitting exactly on the budget.
        assert_eq!(fit_card_text("plain", 12), "plain");
        assert_eq!(fit_card_text("四字选项", 12), "四字选项");
    }

    /// User report: long question/option text was TRUNCATED with an ellipsis;
    /// it must wrap to new lines instead (width-aware, CJK-safe).
    #[test]
    fn question_card_wraps_long_options_and_questions_instead_of_truncating() {
        use super::transcript_build::{push_user_question_entry, wrap_display_width};
        let palette = Palette::for_theme(ThemeName::Slate);
        let entry = crate::model::UserQuestionEntry {
            header: String::new(),
            question: "Which of these very long strategies should we adopt for the multi-region rollout considering budget, latency, and team capacity?".into(),
            options: vec![octos_core::ui_protocol::UserQuestionOption {
                label: "Adopt the fully incremental region-by-region strategy".into(),
                description: "slower but derisks the migration and keeps rollback trivial at every stage of the multi-quarter plan".into(),
            }],
            multi_select: false,
            option_selected: vec![false],
            free_text: String::new(),
            cursor: 0,
            editing_free_text: false,
        };
        let mut lines = Vec::new();
        push_user_question_entry(&mut lines, palette, &entry, 40);
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            !rendered.iter().any(|row| row.contains('…')),
            "no ellipsis truncation in the card: {rendered:?}"
        );
        let joined = rendered
            .iter()
            .map(|row| row.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            joined.contains("team capacity?"),
            "the question tail survives via wrapping: {rendered:?}"
        );
        assert!(
            joined.contains("multi-quarter plan"),
            "the option tail survives via wrapping: {rendered:?}"
        );
        for row in &rendered {
            assert!(
                unicode_width::UnicodeWidthStr::width(row.as_str()) <= 40,
                "no rendered row exceeds the card width: {row:?}"
            );
        }

        // Width math: budget counts display columns (CJK double-width).
        let rows = wrap_display_width("宽宽宽宽 宽宽宽宽", 8);
        assert_eq!(rows, vec!["宽宽宽宽".to_string(), "宽宽宽宽".to_string()]);
        // A single over-budget word hard-breaks instead of overflowing.
        let rows = wrap_display_width("abcdefghij", 4);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn render_inline_multi_select_user_question_shows_checkboxes() {
        let app = app_with_user_question(vec![user_question(
            "Targets",
            "Which targets?",
            &["stable", "nightly"],
            true,
        )]);

        let text = rendered_text(&app);

        // Multi-select uses a hollow square marker (distinct from the radio).
        assert!(text.contains("▢ stable"));
        assert!(text.contains("▢ nightly"));
        assert!(!text.contains("○ stable")); // not the single-select marker
        assert!(text.contains("Other"));
    }

    #[test]
    fn render_garbled_user_question_renders_info_fallback_without_submit_affordance() {
        // No structured questions: must still render the mandatory title/body
        // fallback as an INFORMATIONAL card, but must NOT offer a "Type your
        // answer" affordance (input would be discarded and a submit cannot form a
        // valid respond). Only a dismiss hint is shown (DO-NOT-SHIP #2).
        let app = app_with_user_question(Vec::new());

        let text = rendered_text(&app);

        assert!(text.contains("Pick a framework"));
        assert!(text.contains("The agent needs your input."));
        assert!(text.contains("No answerable options were provided."));
        assert!(text.contains("Esc = dismiss"));
        // No input affordance is offered for the garbled fallback.
        assert!(!text.contains("Type your answer"));
        assert!(!text.contains("Enter = submit"));
    }

    #[test]
    fn render_default_chat_lists_queued_user_questions_without_work_pane() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do a full code review pls")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "Plan:\n- Review renderer\n- Run tests".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.pending_messages = vec![
            "also list queued user questions".into(),
            "check the sticky pane height".into(),
        ];

        let text = rendered_text(&app);

        assert!(text.contains("do a full code review pls"));
        assert!(text.contains("queued 2 messages after active turn"));
        assert!(!text.contains("Work  sticky"));
        assert!(text.contains("› also list queued user questions"));
        assert!(text.contains("› check the sticky pane height"));
    }

    #[test]
    fn render_launch_banner_shows_box_logo_and_greeting_on_empty_session() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("dspfac".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(
            launch_banner_active(&app),
            "empty session must show the launch banner"
        );
        let text = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 100, 30)
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(
            text.contains("╭"),
            "banner must draw a top-left rounded corner"
        );
        assert!(
            text.contains("╯"),
            "banner must draw a bottom-right rounded corner"
        );
        assert!(text.contains("octos"), "banner box title");
        assert!(
            text.contains("██████╗"),
            "banner must show the OCTOS figlet"
        );
        assert!(
            text.contains("Welcome back — dspfac"),
            "banner greeting names the profile"
        );
    }

    #[test]
    fn launch_banner_hidden_once_session_has_messages() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("dspfac".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(
            !launch_banner_active(&app),
            "banner must disappear once the conversation starts"
        );
    }

    #[test]
    fn render_status_uses_static_idle_label_without_spinner() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Idle"));
        for frame in ["◐", "◓", "◑", "◒"] {
            assert!(!text.contains(frame), "idle render must not animate");
        }
    }

    #[test]
    fn render_active_state_uses_bottom_status_without_split_progress_pane() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("build the site")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "Working on it.".into(),
                }),
            }],
            0,
            "thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 100, 28);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");
        let spinner_count = ["◐", "◓", "◑", "◒"]
            .into_iter()
            .map(|frame| text.matches(frame).count())
            .sum::<usize>();

        assert!(text.contains("Working on it."));
        // The in-progress status marker is the animated galaxy spinner now
        // (pinned so it survives a transcript that scrolls the chip off).
        assert!(
            SPINNER_FRAMES
                .iter()
                .any(|frame| text.contains(&format!("state {frame} Working"))),
            "status bar shows an octopus-spinner + Working:\n{text}"
        );
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert_eq!(
            spinner_count, 0,
            "normal chat layout should not animate a split progress pane:\n{text}"
        );
    }

    #[test]
    fn render_work_status_shows_supported_task_affordances() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run background task")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "background build".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Working on it.".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("Working"));
        assert!(text.contains("1 background task(s)"));
        assert!(text.contains("/ps to view"));
        assert!(text.contains("/stop to close"));
    }

    #[test]
    fn render_composer_does_not_embed_blocked_status_details() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("complete m9 contract")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_run_state_blocked("approval required");

        let text = rendered_text(&app);

        assert!(text.contains("state ! Blocked"));
        assert!(text.contains("approval required"));
        assert!(!text.contains("Blocked:"));
        assert!(!text.contains("y/s/n approval"));
    }

    #[test]
    fn render_assistant_markdown_hangs_body_without_marker_leakage() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "First paragraph\n\n- **Either** install Node.js\n\n| Page | Content |\n|---|---|\n| Home | `Hero` section |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let prose = row_containing(&rows, "First paragraph");
        let bullet = row_containing(&rows, "Either");
        let table = row_containing(&rows, "Page");
        let text = rows.join("\n");

        assert!(
            prose
                .find("•")
                .is_some_and(|idx| idx < prose.find("First paragraph").unwrap())
        );
        // Body rows hang 2 columns under the marker — never a second `• `.
        assert_eq!(bullet.find("- "), Some(2));
        // The table is now drawn as a real bordered grid, so its rows start with
        // the box border (after the hang) rather than the raw cell text — still
        // no marker leakage.
        assert!(table.starts_with("  │"));
        assert!(table.contains("Page"));
        assert!(!text.contains("|---|---|"));
        assert!(!text.contains("**Either**"));
        assert!(!text.contains("`Hero`"));
    }

    #[test]
    fn render_streaming_sentence_spacing_keeps_words_separated() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "We can implement.Now run tests.All pass. Build is ready:Next step. Rebuild:🎉 done.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(text.contains("implement. Now"));
        assert!(text.contains("tests. All"));
        assert!(text.contains("ready: Next"));
        assert!(text.contains("Rebuild: "));
        assert!(text.contains("🎉"));
        assert!(!text.contains("implement.Now"));
        assert!(!text.contains("tests.All"));
    }

    #[test]
    fn render_soft_newlines_in_prose_as_spaces() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "🎉 Build succeeded! All 5 pages built cleanly\nin 291ms:",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let row = row_containing(&rows, "Build succeeded");

        assert!(row.contains("Build succeeded! All 5 pages built cleanly in 291ms:"));
    }

    #[test]
    fn render_markdown_tables_inline_bold_and_inline_code() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "| File | Purpose |\n|---|---|\n| app.rs | **Renderer** and `layout` |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("File"));
        assert!(text.contains("Purpose"));
        assert!(text.contains("Renderer"));
        assert!(text.contains("layout"));
        assert!(!text.contains("|---|---|"));
        assert!(text.contains("│"));
        let bold_style = style_for_text(&buffer, "Renderer").expect("bold cell style");
        let code_style = style_for_text(&buffer, "layout").expect("inline code style");
        assert!(bold_style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(code_style.fg, Some(palette.highlight));
    }

    #[test]
    fn render_markdown_table_keeps_visible_columns() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "| File | Problem | Fix |\n|---|---|---|\n| Hero.astro | Orphan --- with no content or closing marker | Removed the --- line entirely |\n| Header.astro | Same — bare --- then HTML | Removed the --- line |",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);
        let header = row_containing(&rows, "Problem");
        let hero = row_containing(&rows, "Hero.astro");

        assert!(header.contains("File"));
        assert!(header.contains("Problem"));
        assert!(header.contains("Fix"));
        assert!(header.contains("│"));
        assert!(hero.contains("Hero.astro"));
        assert!(hero.contains("│"));
        assert!(!rows.join("\n").contains("|---|---|---|"));
    }

    #[test]
    fn render_markdown_table_draws_box_borders() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("| A | B |\n|---|---|\n| x | y |")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for ch in ["┌", "┬", "┐", "│", "├", "┼", "┤", "└", "┴", "┘"] {
            assert!(text.contains(ch), "bordered table missing `{ch}`");
        }
        // The old dashed header separator is gone (box-drawing replaces it).
        assert!(!text.contains("-+-"));
    }

    #[test]
    fn render_markdown_table_fits_and_truncates_on_narrow_width() {
        let wide = "| Column One | Column Two | Column Three |\n|---|---|---|\n| a very long first cell value | another long-ish value | a third long cell value |";
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(wide)],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 44, 30);
        let text = rendered_rows(&buffer).join("\n");
        // Still a bordered grid, but cells are ellipsized to fit the narrow pane.
        assert!(text.contains("┌"));
        assert!(text.contains("└"));
        assert!(text.contains("│"));
        assert!(text.contains("…"), "wide cells should be truncated to fit");
    }

    #[test]
    fn render_markdown_table_clips_many_columns_to_pane_width() {
        // codex P2: with enough columns, even minimum-width cells + borders
        // exceed a narrow pane. No produced line may be wider than the pane,
        // or ratatui wraps it and breaks the grid.
        let palette = Palette::for_theme(ThemeName::Codex);
        let header = (1..=8)
            .map(|i| format!("Col{i}"))
            .collect::<Vec<_>>()
            .join(" | ");
        let sep = ["---"; 8].join("|");
        let row = (1..=8)
            .map(|i| format!("value {i} text"))
            .collect::<Vec<_>>()
            .join(" | ");
        let content = format!("| {header} |\n|{sep}|\n| {row} |");
        let width = 30;
        let mut lines = Vec::new();
        push_formatted_body(
            &mut lines,
            palette,
            &content,
            "",
            Some(palette.surface),
            width,
        );
        for line in &lines {
            let line_width: usize = line
                .spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum();
            assert!(
                line_width <= width,
                "table line width {line_width} exceeds pane width {width}"
            );
        }
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref().to_string())
            .collect();
        assert!(text.contains("│"), "still a bordered table");
    }

    #[test]
    fn table_cell_width_uses_display_width_for_wide_characters() {
        // Regression: emoji/CJK have display width 2 but a single char, so
        // chars().count() under-padded their table columns and misaligned the
        // separators. Width math must use display width.
        assert_eq!(table_cell_width("ab"), 2);
        assert_eq!(table_cell_width("🐳"), 2);
        assert_eq!(table_cell_width("中文"), 4);
        assert_eq!(table_cell_width("a🐳b"), 4);
    }

    #[test]
    fn markdown_blockquote_detects_quote_lines() {
        assert_eq!(markdown_blockquote("> quoted text"), Some("quoted text"));
        assert_eq!(markdown_blockquote(">quoted"), Some("quoted"));
        assert_eq!(markdown_blockquote("not a quote"), None);
        assert_eq!(markdown_blockquote(">"), None);
    }

    #[test]
    fn render_markdown_blockquote_strips_marker() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("> a quoted line")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("a quoted line"));
        // The literal markdown `>` marker must not leak into the rendered prose.
        assert!(!text.contains("> a quoted line"));
        assert!(text.contains("▌"));
    }

    #[test]
    fn render_markdown_code_fence_uses_clean_gutter() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("```python\nprint('hi')\n```")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let buffer = rendered_buffer(&app, Palette::for_theme(ThemeName::Codex));
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("python"));
        assert!(text.contains("print('hi')"));
        // The verbose "end code … --------" footer is gone; a clean box gutter is used.
        assert!(!text.contains("end code"));
        assert!(text.contains("┌─"));
        assert!(text.contains("└─"));
    }

    #[test]
    fn render_diff_code_fence_highlights_added_removed_and_hunks() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "```diff\n--- before.json\n+++ after.json\n@@ -2,6 +2,6 @@\n-  \"scroll-mode\": \"pinned\",\n+  \"scroll-mode\": \"native\",\n```",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);

        let removed_style = style_for_text(&buffer, "pinned").expect("removed diff style");
        let added_style = style_for_text(&buffer, "native").expect("added diff style");
        let hunk_style = style_for_text(&buffer, "@@ -2,6 +2,6 @@").expect("hunk diff style");

        assert_eq!(removed_style.fg, Some(palette.danger));
        assert_eq!(removed_style.bg, Some(palette.danger_bg));
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
        assert_eq!(hunk_style.fg, Some(palette.accent));
        assert_eq!(hunk_style.bg, Some(palette.diff_context_bg));
    }

    #[test]
    fn render_unlabeled_unified_diff_fence_is_reclassified_from_code() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "```\n--- before.json\n+++ after.json\n@@ -1 +1 @@\n-  \"scroll-mode\": \"pinned\"\n+  \"scroll-mode\": \"native\"\n```",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let rows = rendered_rows(&buffer);

        assert!(row_containing(&rows, "┌─ diff").contains("diff"));
        let added_style = style_for_text(&buffer, "native").expect("added diff style");
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
    }

    #[test]
    fn render_pipe_commands_are_not_treated_as_markdown_tables() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant(
                    "Use `find . | xargs rm` only in a sandbox.",
                )],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        assert!(rendered_text(&app).contains("find . | xargs rm"));
    }

    #[test]
    fn render_first_launch_onboarding_is_not_mixed_with_empty_chat() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        assert!(text.contains("Welcome to Octos"));
        // The "stays local, no OTP" framing is no longer a dead menu row — it
        // moved to the right-hand teaching pane ("About this step"), and the
        // profile step is identified by its purpose line.
        assert!(text.contains("About this step"));
        assert!(text.contains("Create a local identity for Octos"));
        assert!(text.contains("Onboarding setup"));
        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    /// M22 (#58): the first-run onboarding surface renders the ASCII OCTOS
    /// wordmark in the MAIN window (not a right-side preview pane). This pins
    /// the splash so a future refactor cannot quietly drop the distinctive
    /// identity.
    #[test]
    fn render_first_launch_onboarding_includes_ascii_octos_splash() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        // ASCII figlet wordmark (a characteristic block-letter row) plus the
        // human-readable tagline render in the MAIN window.
        assert!(
            text.contains("██████╗"),
            "expected OCTOS figlet art in the main window, got:\n{text}"
        );
        assert!(text.contains("Welcome to Octos — Your Coding Buddy"));
    }

    /// At the soak's narrow 80x24 first-launch size, the OCTOS logo shows in the
    /// main window AND the onboarding menu — through its Continue action — stays
    /// fully visible (codex P2: the logo must never clip the menu).
    #[test]
    fn render_first_launch_onboarding_80x24_shows_logo_without_clipping_menu() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));
        let text =
            rendered_buffer_with_size(&store.state, Palette::for_theme(ThemeName::Slate), 80, 24)
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();
        assert!(
            text.contains("Welcome to Octos — Your Coding Buddy"),
            "logo/tagline must render at 80x24"
        );
        assert!(
            text.contains("Continue - Create profile"),
            "menu Continue must not be clipped at 80x24"
        );
    }

    /// UX2 A.1: the OCTOS banner header only consumes rows ABOVE what the menu
    /// needs, so the step list, its inputs, and the explanation pane are never
    /// clipped on short terminals. Full figlet box (11 rows) only with real
    /// surplus AND width; otherwise the compact tagline box (3 rows), then
    /// nothing.
    #[test]
    fn onboarding_header_height_takes_only_menu_surplus() {
        // Tall terminal, menu needs 14 rows → ample surplus → full figlet box.
        assert_eq!(onboarding_header_height(37, 120, 14), 11);
        // Short terminal (root[0] ~16-17 rows, menu needs 14): surplus 2-3 →
        // compact box only once there are 3 surplus rows; below that, nothing.
        assert_eq!(onboarding_header_height(17, 120, 14), 3);
        assert_eq!(onboarding_header_height(16, 120, 14), 0);
        // No surplus → no header at all (the menu takes everything).
        assert_eq!(onboarding_header_height(14, 120, 14), 0);
        // Narrow terminal → never the wide figlet; compact box at most.
        assert_eq!(onboarding_header_height(40, 40, 5), 3);
    }

    /// UX2 A: the three-region onboarding layout renders end-to-end on a wide
    /// terminal — TOP figlet banner header, MAIN step list, and the RIGHT
    /// teaching panel with the current step's explanatory prose (not a bare
    /// checklist). Asserts against the i18n source so it tracks wording/locale.
    #[test]
    fn render_first_launch_onboarding_shows_header_steps_and_explanation_pane() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.state.set_capabilities(UiProtocolCapabilities::new(
            &[crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE],
            &[],
        ));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text =
            rendered_buffer_with_size(&store.state, Palette::for_theme(ThemeName::Slate), 140, 44)
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>();

        // TOP: figlet banner header (a characteristic block-letter row) + the
        // bordered box corner.
        assert!(text.contains("██████╗"), "figlet header at top:\n{text}");
        assert!(text.contains('╭'), "header is a bordered window:\n{text}");
        // RIGHT: the teaching panel title + the current step's explanatory
        // prose. Assert against the i18n source (NOT a hardcoded literal) so the
        // test tracks wording/locale changes.
        let panel_title = t!("onboarding.wizard.explain_title", locale = "en");
        assert!(
            text.contains(&*panel_title),
            "teaching panel title in the right pane:\n{text}"
        );
        // The Profile-step explanation is a multi-line source string; assert on
        // its first word so soft-wrapping in the pane can't flake it.
        let explain_first_word = crate::menu::wizard::WizardStep::Profile
            .explanation()
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string();
        assert!(
            !explain_first_word.is_empty() && text.contains(&explain_first_word),
            "current-step explanation prose in the right pane (`{explain_first_word}`):\n{text}"
        );
    }

    #[test]
    fn render_first_launch_onboarding_child_menu_stays_on_onboarding_surface() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD_FAMILY,
        ));

        let text = rendered_text(&store.state);

        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    /// M22-A: when the backend advertises no onboarding methods,
    /// opening the onboarding menu must render a disabled-reason
    /// status surface — never a blank pane that swallows the
    /// first-launch flow.
    #[test]
    fn render_onboarding_without_capabilities_shows_disabled_reason_not_blank() {
        let mut store = Store {
            state: AppState::new(
                vec![],
                0,
                "Octos UI connected".into(),
                Some("stdio:octos serve --stdio".into()),
                false,
            ),
        };
        store
            .state
            .set_capabilities(UiProtocolCapabilities::new(&[], &[]));
        store.open_menu(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        let text = rendered_text(&store.state);

        // The status surface MUST surface a typed disabled reason.
        assert!(
            text.contains("Onboarding unavailable"),
            "expected disabled-reason title in rendered text:\n{text}"
        );
        // And it MUST NOT render the empty-chat scaffold under
        // first-launch (no sessions) — that would be the "blank pane"
        // regression the acceptance bullet bans.
        assert!(!text.contains("No session selected"));
        assert!(!text.contains("Ask Octos to change code"));
    }

    #[test]
    fn render_composer_shows_staged_messages() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("working")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "running tool".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.pending_messages = vec![
            "it did not do error recovery?".into(),
            "what is ip for mini5".into(),
        ];
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Queued messages (2) after active turn"));
        assert!(text.contains("Ctrl+U clear"));
        assert!(text.contains("it did not do error recovery?"));
        assert!(text.contains("what is ip for mini5"));
        assert_eq!(composer_height(&app), 5);
        let pending_style =
            style_for_text(&buffer, "it did not do error recovery?").expect("pending style");
        assert_eq!(pending_style.bg, Some(palette.diff_context_bg));
    }

    #[test]
    fn render_composer_is_tall_and_places_cursor_in_input() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "fix tests".into();
        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert_eq!(composer_height(&app), 5);
        assert!(text.contains("fix tests"));
        assert!(!text.contains("▌"));
        let rows = rendered_rows(&buffer);
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "› fix tests")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor")
        );
    }

    /// Live markdown highlighting in the composer is STYLE-ONLY: the draft's
    /// characters render verbatim (markers included, no reflow), the terminal
    /// cursor still lands exactly after the last typed char, and only span
    /// styles change (heading → bold title color, inline code → highlight
    /// color, prose → plain).
    #[test]
    fn composer_highlights_markdown_draft_without_changing_text_or_cursor() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.set_composer_text("# hi\n`code` tail");
        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let rows = rendered_rows(&buffer);

        // TEXT CONTENT unchanged: both draft lines render verbatim, markers
        // and all.
        let heading_row = row_index_containing(&rows, "› # hi");
        let code_row = row_index_containing(&rows, "`code` tail");

        // Cell-precise style checks. Columns are char-based, not byte-based:
        // every glyph left of the draft (border, `›`, spaces) is width-1, so
        // the char index equals the cell index.
        let width = usize::from(buffer.area.width);
        let cell_at = |row: usize, needle: &str| {
            let byte = rows[row].find(needle).expect("needle in row");
            let col = rows[row][..byte].chars().count();
            &buffer.content[row * width + col]
        };
        let hash = cell_at(heading_row, "# hi");
        assert_eq!(hash.fg, palette.accent, "heading takes the title color");
        assert!(hash.modifier.contains(Modifier::BOLD), "heading is bold");
        let backtick = cell_at(code_row, "`code`");
        assert_eq!(
            backtick.fg, palette.highlight,
            "inline code (backticks included) takes the code color"
        );
        let tail = cell_at(code_row, "tail");
        assert_eq!(tail.fg, palette.text, "text outside markers stays plain");
        assert!(tail.modifier.is_empty(), "plain text gains no modifier");

        // Cursor invariance: the terminal cursor sits exactly one cell after
        // the draft's last char — the cursor math reads the raw text only and
        // must be unaffected by highlighting.
        let code_byte = rows[code_row].find("`code` tail").expect("draft row");
        let after_tail =
            rows[code_row][..code_byte].chars().count() + "`code` tail".chars().count();
        assert_eq!(cursor, Position::new(after_tail as u16, code_row as u16));
    }

    /// Regression: the harness-row context `LineGauge` label (`ctx …/… ~N%`)
    /// must inherit the theme `surface` background. `LineGauge` paints its whole
    /// area with the widget base style *before* writing the (unstyled) label,
    /// so without `.style(bg: surface)` the label cells fall back to the raw
    /// terminal background — a mismatched solid block on the right of the
    /// harness row, directly above the composer.
    #[test]
    fn harness_gauge_label_inherits_surface_background() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 1,
                pending_continuations: 0,
                phase: Some("orchestrating".into()),
            },
        );
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 15_360,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer_with_size(&app, palette, 120, 42);

        // The gauge label is rendered on the harness row (the `ctx …` text).
        let label_style = style_for_text(&buffer, "ctx ").expect("gauge label rendered");
        assert_eq!(
            label_style.bg,
            Some(palette.surface),
            "gauge label must use the surface bg, not the raw terminal background"
        );

        // The whole gauge column (label + filled/unfilled line) must be a single
        // contiguous surface-backed band — no stray bg=Reset cells.
        let rows = rendered_rows(&buffer);
        let gauge_row = row_index_containing(&rows, "ctx ");
        let width = usize::from(buffer.area.width);
        let row_start = gauge_row * width;
        let first_label_col = rows[gauge_row].find("ctx ").expect("label col");
        for x in first_label_col..width {
            let cell = &buffer.content[row_start + x];
            assert_eq!(
                cell.bg,
                palette.surface,
                "gauge cell at x={x} (sym {:?}) leaked a non-surface background",
                cell.symbol()
            );
        }
    }

    #[test]
    fn render_composer_places_cursor_after_chinese_display_width() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        for ch in "你好世界".chars() {
            app.insert_composer_char(ch);
        }

        let rect = Rect::new(0, 36, 120, 5);
        let cursor = composer_cursor_position(&app, rect).expect("cursor");

        assert_eq!(app.composer, "你好世界");
        assert_eq!(cursor.x, 12);
        assert_eq!(cursor.y, 38);
    }

    #[test]
    fn render_composer_places_cursor_after_mixed_cjk_and_ascii() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.insert_composer_text("abc你好");

        let cursor = composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor");

        assert_eq!(cursor.x, 11);
        assert_eq!(cursor.y, 38);
    }

    #[test]
    fn render_composer_shows_short_multiline_prompt_rows() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "first instruction\nsecond instruction\nthird instruction".into();

        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let rows = rendered_rows(&buffer);

        assert_eq!(composer_height(&app), 7);
        assert!(rows.iter().any(|row| row.contains("› first instruction")));
        assert!(rows.iter().any(|row| row.contains("second instruction")));
        assert!(rows.iter().any(|row| row.contains("third instruction")));
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "third instruction")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 34, 120, 7)).expect("cursor")
        );
    }

    #[test]
    fn render_composer_keeps_common_paste_visible_and_resizes() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=8)
            .map(|idx| format!("pasted visible line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");

        let (buffer, cursor) = rendered_buffer_and_cursor_with_size(
            &app,
            Palette::for_theme(ThemeName::Codex),
            80,
            42,
        );
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert_eq!(composer_height_for_size(&app, 80, 42), 12);
        assert!(text.contains("pasted visible line 1"));
        assert!(text.contains("pasted visible line 8"));
        assert!(!text.contains("Large paste collapsed"));
        assert_eq!(
            row_index_containing(&rows, "pasted visible line 8"),
            usize::from(cursor.y)
        );
    }

    #[test]
    fn render_composer_shows_tail_when_input_exceeds_visible_budget() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=14)
            .map(|idx| format!("budgeted line {idx}"))
            .collect::<Vec<_>>()
            .join("\n");

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 42);
        let text = rendered_rows(&buffer).join("\n");

        assert_eq!(composer_height_for_size(&app, 80, 42), 16);
        assert!(text.contains("showing tail"));
        assert!(!text.contains("budgeted line 1 "));
        assert!(text.contains("budgeted line 14"));
    }

    #[test]
    fn render_composer_wraps_long_single_line_into_extra_rows() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = "x".repeat(180);

        assert_eq!(composer_height_for_size(&app, 80, 42), 7);
    }

    #[test]
    fn render_composer_draws_wrapped_tail_of_long_single_line() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // One logical line longer than the composer width: the tail must wrap
        // onto a 2nd visible row, not be clipped (and the reserved row left dark).
        app.composer = format!("HEAD{}TAIL", "x".repeat(160));

        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, _cursor) = rendered_buffer_and_cursor(&app, palette);
        let rows = rendered_rows(&buffer);

        let head_row = row_index_containing(&rows, "HEAD");
        let tail_row = row_index_containing(&rows, "TAIL");
        assert!(
            tail_row > head_row,
            "wrapped tail should render below the head (head={head_row}, tail={tail_row})"
        );
        // ...and it must be drawn in the visible text colour, not the surface bg.
        let tail_style = style_for_text(&buffer, "TAIL").expect("tail rendered");
        assert_eq!(
            tail_style.fg,
            Some(palette.text),
            "wrapped tail must use the composer text colour, not be invisible"
        );
    }

    #[test]
    fn tail_around_cursor_caps_window_to_row_budget() {
        let width = 10;
        let max_rows = 3;
        // A single logical line far taller than the budget.
        let text = "x".repeat(100);

        // Cursor at the very start: HEAD window, must not exceed the budget
        // (render_composer wraps the returned text, so an over-long return clips
        // the composer footer).
        let head = tail_around_cursor(&text, 0, width, max_rows);
        assert!(
            visual_rows_for_text(&head.text, width) <= max_rows,
            "head window must fit row budget, got {} rows",
            visual_rows_for_text(&head.text, width)
        );

        // Cursor at the end: TAIL window, also within budget, marked truncated.
        let tail = tail_around_cursor(&text, text.len(), width, max_rows);
        assert!(
            visual_rows_for_text(&tail.text, width) <= max_rows,
            "tail window must fit row budget, got {} rows",
            visual_rows_for_text(&tail.text, width)
        );
        assert!(tail.text.starts_with("..."), "tail window marks truncation");
    }

    #[test]
    fn render_empty_composer_shows_cursor_before_hint() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let palette = Palette::for_theme(ThemeName::Codex);
        let (buffer, cursor) = rendered_buffer_and_cursor(&app, palette);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("›  Ask Octos to change code"));
        assert!(!text.contains("▌"));
        let rows = rendered_rows(&buffer);
        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "›  Ask Octos")
        );
        assert_eq!(
            cursor,
            composer_cursor_position(&app, Rect::new(0, 36, 120, 5)).expect("cursor")
        );
    }

    /// P2 (tri-repo #246 ⊃ #232 #3, codex fold 4): the live viewport must
    /// leave at least TWO rows above it on terminals tall enough — DECSTBM
    /// requires top < bottom, so both a full-screen viewport (`CSI 1;0r`)
    /// and a one-row region (`CSI 1;1r`) are unusable for history flushes.
    /// The degenerate 1–2-row terminals keep one live row and are handled by
    /// insert_history's streaming fallback.
    #[test]
    fn live_ui_height_leaves_a_valid_scroll_region_above_the_viewport() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        for height in 3..=10u16 {
            let live = live_ui_height(&app, 80, height);
            assert!(
                live <= height - 2,
                "live UI must leave ≥2 history rows above the viewport: height={height} live={live}"
            );
        }
        // Degenerate 1–2-row terminals: the streaming fallback owns these.
        assert_eq!(live_ui_height(&app, 80, 2), 1);
        assert_eq!(live_ui_height(&app, 80, 1), 1);
    }

    #[test]
    fn render_queued_composer_places_cursor_on_text_row() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("working")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "still running".into(),
                }),
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.composer = "dsada d".into();
        app.pending_messages = vec!["queued prompt".into()];

        let (buffer, cursor) =
            rendered_buffer_and_cursor(&app, Palette::for_theme(ThemeName::Codex));
        let rows = rendered_rows(&buffer);

        assert_eq!(
            usize::from(cursor.y),
            row_index_containing(&rows, "› dsada d")
        );
        assert_ne!(
            usize::from(cursor.y),
            row_index_containing(&rows, "Queued messages (1)") + 2
        );
    }

    #[test]
    fn render_composer_collapses_large_paste_and_keeps_chrome_visible_when_narrow() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.composer = (1..=40)
            .map(|idx| format!("paste-line-{idx:02}-with-some-extra-context"))
            .collect::<Vec<_>>()
            .join("\n");

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 48, 18);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("Large paste collapsed"));
        assert!(text.contains("[paste] 40 lines"));
        assert!(text.contains("preview: paste-line-01"));
        assert!(!text.contains("paste-line-40"));
        assert!(text.contains("Composer"));
        assert!(text.contains("state"));
    }

    #[test]
    fn render_transcript_includes_activity_cards_and_dense_footer() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("gpt-5-codex".into()),
                messages: vec![Message::user("fix the UI")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "working".into(),
                }),
            }],
            0,
            "Tool started: shell".into(),
            Some("/repo/octos".into()),
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id)
                .with_tool_call("call-1")
                .with_detail("cargo test")
                .with_output_preview("running 6 tests\n6 passed")
                .with_success(true)
                .with_duration_ms(1250),
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Activity"));
        assert!(text.contains("⏺ Bash($ cargo test"));
        assert!(text.contains("running 6 tests"));
        assert!(text.contains("1 more line(s) hidden (Ctrl+O expand)"));
        assert!(text.contains("1.2s"));
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        // #267 no-call-id: the activity card must NOT display the `call <id>`
        // suffix (the tool_call_id field is retained, only the display is gone).
        assert!(!text.contains("call call-1"));
        assert!(text.contains("gpt-5-codex"));
        assert!(text.contains("state"));
        assert!(text.contains("running"));
        assert!(text.contains("approval"));
        assert!(text.contains("1 msgs/0 tasks"));
    }

    /// Regression (indent-not-honored): the agent-task child row used to be one
    /// long ratatui `Line` that overflowed the terminal width and wrapped back
    /// to column 0 (the transcript renders with `Wrap { trim: false }`, which
    /// has no hanging indent). Every rendered child line — the invocation row
    /// AND its output-preview lines — must now fit within `wrap_width` at ANY
    /// terminal width, measured with unicode-width so a long CJK command
    /// (double-width glyphs) still fits and never panics at a multibyte cut.
    #[test]
    fn agent_task_child_row_never_exceeds_wrap_width() {
        let long_ascii = format!("echo {}", "abcdefgh ".repeat(40));
        let long_cjk = format!("echo {}", "数据处理与网络请求".repeat(20));
        let items = [
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_arguments(serde_json::json!({ "cmd": long_ascii }))
                .with_tool_call("call_01_ABCDEFGHIJKLMNOP")
                .with_output_preview("=== 1. teams ===\nsome very long output line that keeps going and going and going and going and going")
                .with_success(true)
                .with_duration_ms(21),
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_arguments(serde_json::json!({ "cmd": long_cjk }))
                .with_tool_call("call_02_ZYXWVUTSRQPONMLK")
                .with_success(true)
                .with_duration_ms(21),
        ];
        for wrap_width in [20usize, 32, 48, 60, 80, 120] {
            for item in &items {
                let mut lines: Vec<Line<'static>> = Vec::new();
                push_agent_task_child(
                    &mut lines,
                    Palette::for_theme(ThemeName::Slate),
                    item,
                    true,
                    false,
                    wrap_width,
                    false,
                );
                assert!(
                    !lines.is_empty(),
                    "child row should render at least one line"
                );
                for line in &lines {
                    let w: usize = line
                        .spans
                        .iter()
                        .map(|span| span.content.as_ref().width())
                        .sum();
                    assert!(
                        w <= wrap_width,
                        "child line width {w} exceeds wrap_width {wrap_width}: {:?}",
                        lines_text(&lines)
                    );
                }
            }
        }
    }

    /// The bash row must surface the actual command (`$ echo …`), never the raw
    /// serialized arguments (`{"cmd":…}`), and must not append the `call <id>`
    /// noise (#267 established "no call-id" for CC-style activity cards; this
    /// agent-task-group child path predated that work and still leaked both).
    #[test]
    fn agent_task_bash_row_shows_command_not_raw_json_or_call_id() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_arguments(serde_json::json!({
                "cmd": "echo \"=== 1. teams ===\" && curl -sX POST http://localhost:4000/"
            }))
            .with_tool_call("call_01_UVIa9EBA331xAfxbPFPM4446")
            .with_success(false)
            .with_duration_ms(21);
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            &item,
            true,
            false,
            120,
            false,
        );
        let text = lines_text(&lines);
        assert!(
            text.contains("Bash($ echo"),
            "bash row must show the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\""),
            "bash row must not show raw JSON args: {text:?}"
        );
        assert!(
            !text.contains("call call_"),
            "bash row must not show call-id noise: {text:?}"
        );
        assert!(
            !text.contains("call_01_UVIa9EBA331xAfxbPFPM4446"),
            "the call-id must not be displayed: {text:?}"
        );
    }

    fn agent_task_child_text(item: &ActivityItem, wrap_width: usize) -> String {
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            item,
            true,
            false,
            wrap_width,
            false,
        );
        lines_text(&lines)
    }

    /// Live-capture regression (#273 follow-up): on the real server path the
    /// invocation comes from `detail` — the protocol #1606 `arguments_preview`
    /// echo, a JSON serialization of the tool args capped at ~700 bytes, so it
    /// often arrives CUT mid-string (no closing quote/brace, unparseable by
    /// strict serde). The shell row must still extract `$ <command>`; the raw
    /// `{"cmd":…` framing must never render.
    #[test]
    fn agent_task_bash_row_extracts_command_from_truncated_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail(
                r#"{"cmd":"grep -n '<img' /Users/yuechen/dev/2026-world-cup/client/src/pages/HomePage.tsx /Users/yuechen/dev/2026-world-cup/client/s"#,
            )
            .with_tool_call("call_01_ABCDEFGHIJKLMNOP")
            .with_success(true)
            .with_duration_ms(33);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ grep -n '<img'"),
            "truncated echo must still yield the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\""),
            "raw JSON echo must never render: {text:?}"
        );
    }

    /// A complete (untruncated) args echo in `detail` parses strictly and the
    /// shell row shows the command alone — sibling keys like `timeout` are
    /// noise the raw echo used to drag in.
    #[test]
    fn agent_task_bash_row_extracts_command_from_complete_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail(r#"{"cmd":"echo hi","timeout":5}"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ echo hi"),
            "complete echo must yield the command: {text:?}"
        );
        assert!(
            !text.contains("{\"cmd\"") && !text.contains("timeout"),
            "echo framing and sibling keys must not render: {text:?}"
        );
    }

    /// The envelope live lane parks the same args echo in `arguments` as a
    /// JSON String (detail carries the load-bearing thread marker there, and
    /// after archival the echo can surface via `arguments`). A string-typed
    /// `arguments` must be treated exactly like a detail echo — never
    /// re-serialized into `"{\"cmd\":…`.
    #[test]
    fn agent_task_bash_row_extracts_command_from_string_arguments_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_arguments(serde_json::Value::String(
                r#"{"cmd":"echo hi","timeout":5}"#.into(),
            ))
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            text.contains("$ echo hi"),
            "string-arguments echo must yield the command: {text:?}"
        );
        assert!(
            !text.contains("cmd") && !text.contains("\\\""),
            "echo framing must not render (raw or re-escaped): {text:?}"
        );
    }

    /// Non-shell tools: a complete args echo in `detail` renders the compact
    /// `key=value` form (same as the object-arguments path), and JSON string
    /// escapes (`\n`) never leak into the one-line row as literal two-char
    /// sequences.
    #[test]
    fn agent_task_edit_row_compacts_complete_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"{"path":"/a/App.tsx","new_string":"<Route/>\n  <Route/>"}"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        // serde_json maps iterate alphabetically (no preserve_order), so the
        // first meaningful field is `new_string`; its REAL newline (decoded by
        // the strict parse) must flatten to spaces in the one-line row.
        assert!(
            text.contains("new_string=<Route/>   <Route/>"),
            "complete echo must compact to key=value: {text:?}"
        );
        assert!(
            !text.contains("{\"path\""),
            "raw JSON echo must never render: {text:?}"
        );
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
    }

    /// Non-shell tools with a TRUNCATED echo (strict parse fails): the cleanup
    /// pass must strip the `{"` framing and decode the common escapes — the
    /// bar is NO raw `{"key":` prefix and NO literal `\n` in the row.
    #[test]
    fn agent_task_edit_row_scrubs_truncated_detail_echo() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"{"path":"/a/App.tsx","new_string":"<Route/>\n  <Ro"#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            !text.contains("{\"path\""),
            "raw JSON echo prefix must never render: {text:?}"
        );
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
        assert!(
            text.contains("/a/App.tsx"),
            "the echo's content should survive the scrub: {text:?}"
        );
    }

    /// The producer's `key: value` preview format (object args rendered as
    /// `path: "...", new_string: "..."`) JSON-encodes string values, so `\n`
    /// escapes leak as literal two-char sequences — the display pass must
    /// decode them (rows are one-line; an escaped newline becomes a space).
    #[test]
    fn agent_task_row_unescapes_key_value_echo_escapes() {
        let item = ActivityItem::new(ActivityKind::Tool, "edit_file", "complete")
            .with_detail(r#"path: "/a/App.tsx", new_string: "<Route/>\n  <Route/>""#)
            .with_success(true)
            .with_duration_ms(21);
        let text = agent_task_child_text(&item, 120);
        assert!(
            !text.contains("\\n"),
            "literal backslash-n must never render: {text:?}"
        );
        assert!(
            text.contains("path: \"/a/App.tsx\""),
            "non-JSON detail otherwise renders as-is: {text:?}"
        );
    }

    /// Plain (non-JSON) details are untouched.
    #[test]
    fn agent_task_row_keeps_plain_detail_verbatim() {
        let bang = ActivityItem::new(ActivityKind::Tool, "bash", "complete")
            .with_detail("! echo hi")
            .with_success(true);
        let text = agent_task_child_text(&bang, 120);
        assert!(
            text.contains("! echo hi"),
            "plain detail must render unchanged: {text:?}"
        );
    }

    /// Fidelity guard (codex review): `detail` ALSO carries already-decoded
    /// REAL invocation text — the `!`-bang echo and the live-lane
    /// `tool_invocation_detail` command summaries. A brace-group command must
    /// keep its `{` (only `{"…` is a JSON echo), and an intentional two-char
    /// `\n` in a real command (`printf '\n'`) must render verbatim — the
    /// escape decode applies to serialized echo shapes, not plain commands.
    #[test]
    fn agent_task_row_keeps_real_commands_verbatim() {
        for title in ["shell", "!"] {
            let brace_group = ActivityItem::new(ActivityKind::Tool, title, "complete")
                .with_detail("{ echo ok; }")
                .with_success(true);
            let text = agent_task_child_text(&brace_group, 120);
            assert!(
                text.contains("{ echo ok; }"),
                "brace-group command must render verbatim for {title}: {text:?}"
            );
        }
        let printf = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_detail(r#"printf '\n' | wc -l"#)
            .with_success(true);
        let text = agent_task_child_text(&printf, 120);
        assert!(
            text.contains(r#"printf '\n' | wc -l"#),
            "a real command's two-char escape must render verbatim: {text:?}"
        );
    }

    /// The lenient extractor never panics on multibyte content, respects a
    /// closing quote when one survived the cut, decodes escapes, and drops a
    /// dangling backslash left by the byte cap.
    #[test]
    fn lenient_echo_extraction_handles_multibyte_escapes_and_cuts() {
        let cases: &[(&str, &str)] = &[
            // CJK content cut with the producer's ellipsis, no closing quote.
            (
                "{\"cmd\":\"echo 日本語のコマンド…",
                "echo 日本語のコマンド…",
            ),
            // Closing quote survived the cut: trailing sibling junk dropped.
            (r#"{"cmd":"echo hi","timeo"#, "echo hi"),
            // Escaped quote/backslash decode; escaped newline becomes space.
            (r#"{"cmd":"echo \"hi\" \\ a\nb"#, "echo \"hi\" \\ a b"),
            // Dangling backslash at the cut is dropped.
            (r#"{"cmd":"echo hi\"#, "echo hi"),
            // `command` key works too.
            (r#"{"command":"ls -la","cwd":"/tmp"}"#, "ls -la"),
        ];
        for (echo, expected) in cases {
            let item = ActivityItem::new(ActivityKind::Tool, "bash", "complete").with_detail(*echo);
            let text = tool_invocation_text(&item).expect("invocation");
            assert_eq!(
                &text, expected,
                "echo {echo:?} must extract {expected:?}, got {text:?}"
            );
        }
    }

    /// The recovery-suggestion row (a non-Tool `Warning` activity) also predated
    /// the no-call-id convention — it must not append `call <id>` either (the
    /// exact fragment that wrapped to column 0 in the reported bug).
    #[test]
    fn agent_task_recovery_row_drops_call_id() {
        let item = ActivityItem::new(
            ActivityKind::Warning,
            "Recovery suggestion",
            "permission blocked; ask for the exact permission/escalation",
        )
        .with_tool_call("call_01_UVIa9EBA331xAfxbPFPM4446");
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_agent_task_child(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            &item,
            false,
            false,
            120,
            false,
        );
        let text = lines_text(&lines);
        assert!(
            text.contains("Recovery suggestion"),
            "recovery row should render: {text:?}"
        );
        assert!(
            !text.contains("call call_"),
            "recovery row must not show call-id noise: {text:?}"
        );
        assert!(
            !text.contains("call_01_"),
            "recovery row call-id must not be displayed: {text:?}"
        );
    }

    /// An armed loop fires real model turns on an interval — the status bar
    /// must say so at a glance (a forgotten loop otherwise burns tokens
    /// invisibly). Paused loops surface too so `/loop resume` is
    /// discoverable.
    #[test]
    fn status_bar_shows_loop_chip_when_session_has_loops() {
        fn loop_record(status: &str) -> octos_core::ui_protocol::UiLoopRecord {
            serde_json::from_value(serde_json::json!({
                "loop_id": "loop-1",
                "session_id": "local:loops",
                "prompt": "keep poking",
                "mode": "interval",
                "interval_seconds": 60,
                "status": status,
                "expires_at_ms": 0,
                "created_at_ms": 0,
                "updated_at_ms": 0,
            }))
            .expect("loop record")
        }
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:loops".into()),
                title: "loops".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let session_id = SessionKey("local:loops".into());

        app.set_session_loops(&session_id, vec![loop_record("active")]);
        let text = rendered_text(&app);
        assert!(
            text.contains("1 active loop"),
            "active loop chip missing: {text}"
        );

        app.set_session_loops(&session_id, vec![loop_record("paused")]);
        let text = rendered_text(&app);
        assert!(
            text.contains("1 paused loop"),
            "paused loop chip missing: {text}"
        );

        app.set_session_loops(&session_id, vec![]);
        let text = rendered_text(&app);
        assert!(
            !text.contains("loop(s)"),
            "chip must vanish with no loops: {text}"
        );
    }

    /// A deleted loop is a tombstone — `/loop delete` removed it, so it
    /// must not linger as a dimmed zombie chip in the sticky autonomy
    /// indicator. Deleted records can still arrive via the `loop/list`
    /// rehydration path, so `set_session_loops` must strip them exactly
    /// like `upsert_session_loop` does. Without the filter the row reads
    /// "0 running" (the active/paused counts exclude tombstones) yet
    /// still renders chips — the `#1576` delete-can't-clear-it symptom.
    #[test]
    fn deleted_loops_do_not_surface_as_zombie_chips() {
        fn loop_record(status: &str) -> octos_core::ui_protocol::UiLoopRecord {
            serde_json::from_value(serde_json::json!({
                "loop_id": "loop-1",
                "session_id": "local:loops",
                "prompt": "keep poking",
                "mode": "interval",
                "interval_seconds": 60,
                "status": status,
                "expires_at_ms": 0,
                "created_at_ms": 0,
                "updated_at_ms": 0,
            }))
            .expect("loop record")
        }
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:loops".into()),
                title: "loops".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let session_id = SessionKey("local:loops".into());

        // Positive control: an active loop DOES surface as a chip, so the
        // negative assertions below are meaningful.
        let retained = app.set_session_loops(&session_id, vec![loop_record("active")]);
        assert_eq!(retained, 1);
        assert!(
            rendered_text(&app).contains("keep poking"),
            "active loop chip should render"
        );

        // Regression: a deleted (tombstoned) loop must be dropped, not
        // stored and dimmed. The returned count must reflect the retained
        // loops so the refresh acknowledgment can't claim more than the
        // indicator shows (codex P2).
        let retained = app.set_session_loops(&session_id, vec![loop_record("deleted")]);
        assert_eq!(retained, 0, "deleted-only batch retains nothing");
        assert_eq!(
            app.session_autonomy_for(&session_id)
                .map(|state| state.loops.len()),
            Some(0),
            "deleted loop must be filtered out of the mirror"
        );
        assert_eq!(app.session_loop_counts(&session_id), (0, 0));
        let text = rendered_text(&app);
        assert!(
            !text.contains("keep poking"),
            "deleted loop must not render a chip: {text}"
        );

        // Mixed batch: only the non-deleted survivor is kept and counted.
        let retained = app.set_session_loops(
            &session_id,
            vec![loop_record("active"), loop_record("deleted")],
        );
        assert_eq!(retained, 1, "mixed batch retains only the non-deleted loop");
        assert_eq!(
            app.session_autonomy_for(&session_id)
                .map(|state| state.loops.len()),
            Some(1),
            "mixed batch must keep only the non-deleted loop"
        );
        assert_eq!(app.session_loop_counts(&session_id), (1, 0));
    }

    #[test]
    fn render_activity_is_anchored_after_latest_user_prompt() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("what is the status"),
                    Message::user("are you working"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_detail("cargo test")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        let first_prompt = text.find("what is the status").expect("first prompt");
        let latest_prompt = text.find("are you working").expect("latest prompt");
        let command = text.find("Bash($ cargo test").expect("activity command");

        assert!(first_prompt < latest_prompt);
        assert!(latest_prompt < command);
        assert!(!text.contains("Activity"));
    }

    #[test]
    fn render_completed_turn_activity_log_is_interleaved_with_chat_history() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("The site is built and ready."),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo build")
                    .with_output_preview("Finished dev build")
                    .with_success(true),
            ],
        });

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        let prompt = text.find("build the site").expect("user prompt");
        let work_log = text.find("Agent task completed").expect("agent task");
        let command = text.find("Bash($ cargo build").expect("tool command");
        let answer = text
            .find("The site is built and ready.")
            .expect("assistant answer");

        assert!(prompt < answer);
        assert!(answer < work_log);
        assert!(work_log < command);
        assert!(!text.contains("Activity"));
    }

    #[test]
    fn render_large_completed_turn_activity_log_is_compact_by_default() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let items = (1..=12)
            .map(|idx| {
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(turn_id.clone())
                    .with_tool_call(format!("read-{idx}"))
                    .with_detail(format!("src/file_{idx}.rs"))
                    .with_success(true)
            })
            .collect::<Vec<_>>();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("review everything"),
                    Message::assistant("Review complete."),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("review everything".into()),
            anchor_index: Some(0),
            items,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Agent task completed"));
        assert!(text.contains("... +9 more"));
        assert!(text.contains("12 completed"));
        assert!(!text.contains("src/file_1.rs"));
    }

    #[test]
    fn chip_stays_orchestrating_while_sub_agents_run_after_parent_calls_complete() {
        // Parallel-spawn regression: `spawn` returns immediately, so the parent
        // turn's tool calls are all "completed" while the spawned sub-agents
        // (session.tasks, Running) are still working. The chip must NOT say
        // "Agent task completed" — it should stay "Orchestrating…" and surface
        // the running sub-agent count.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("launch agents to study X, Y, Z")],
                tasks: vec![
                    crate::model::TaskView {
                        id: octos_core::TaskId::new(),
                        title: "hermes-research".into(),
                        state: TaskRuntimeState::Running,
                        runtime_detail: None,
                        output_tail: String::new(),
                        turn_id: None,
                    },
                    crate::model::TaskView {
                        id: octos_core::TaskId::new(),
                        title: "openclaw-research".into(),
                        state: TaskRuntimeState::Running,
                        runtime_detail: None,
                        output_tail: String::new(),
                        turn_id: None,
                    },
                ],
                // Parent turn has FINISHED (no live_reply) but the background
                // sub-agents it spawned are still running — the chip must still
                // attribute them (via latest-turn), not flip to "completed".
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("launch agents".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
                ActivityItem::new(ActivityKind::Tool, "glob", "complete")
                    .with_turn(turn_id)
                    .with_success(true),
            ],
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating"),
            "chip must stay Orchestrating while sub-agents run: {text:?}"
        );
        assert!(
            text.contains("2 sub-agent(s) running"),
            "chip should surface the running sub-agent count: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "chip must NOT report completed while sub-agents run: {text:?}"
        );
    }

    #[test]
    fn finalized_scrollback_render_of_subagent_live_turn_is_terminal_not_orchestrating() {
        // mini5 residual (the second face of the "duplicated orchestrating" bug):
        // a SETTLED turn whose spawned sub-agents are still running must not be
        // flushed into IMMUTABLE scrollback as "Orchestrating… N running". That
        // copy strands frozen (append-only scrollback can't be reclaimed): it
        // keeps lying "N sub-agent(s) running" after the sub-agent finishes, and
        // a menu-toggle reflush strands a second such copy above the live chip.
        // The FINALIZED render records only the parent turn's terminal outcome;
        // the live aggregate chip carries the volatile sub-agent status.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("launch agents to study X, Y, Z")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "hermes-research".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: None,
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("launch agents".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn_id)
                    .with_success(true),
            ],
        });

        // The LIVE render still surfaces the running sub-agent (unchanged path).
        assert!(
            rendered_text(&app).contains("Orchestrating"),
            "the live chip must still show sub-agent progress"
        );

        // The FINALIZED (scrollback) render must be terminal — no volatile status
        // that would freeze into append-only history.
        let flushed: String =
            finalized_history_lines(&app, Palette::for_theme(ThemeName::Slate), 100)
                .iter()
                .flat_map(|line| line.spans.iter())
                .map(|span| span.content.as_ref())
                .collect();
        assert!(
            !flushed.contains("Orchestrating"),
            "immutable scrollback must not bake in the volatile Orchestrating status: {flushed:?}"
        );
        assert!(
            !flushed.contains("sub-agent(s) running"),
            "scrollback must not strand a running-count that freezes: {flushed:?}"
        );
        assert!(
            flushed.contains("Agent task completed"),
            "the flushed turn-card records the parent turn's terminal outcome: {flushed:?}"
        );
    }

    #[test]
    fn covered_late_activity_flush_of_running_item_is_terminal_not_orchestrating() {
        // Third face of the "duplicated orchestrating" bug (after #339/#342):
        // the covered late-activity scrollback flush
        // (`finalized_late_activity_lines_for_coverages` ->
        // `push_turn_activity_log_section_unflushed` ->
        // `push_finalized_activity_items_section`) receives the turn's UNFLUSHED
        // items WITHOUT filtering by run state, so a still-RUNNING item (e.g. a
        // long `Bash($ sleep 45 …)` that keeps the turn live) was baked into
        // IMMUTABLE scrollback as "Orchestrating… (1 active)" with its spinner
        // frozen. That copy strands one frame behind the LIVE aggregate chip:
        // two "Orchestrating" lines, same turn, different spinner glyphs. The
        // finalized flush must record only TERMINAL activity; running items stay
        // in the live tail until they settle.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the pipeline")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            request: Some("run".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "running")
                    .with_turn(turn_id.clone()),
            ],
        });

        let coverage = LiveTurnFinalization {
            session_id: session_id.0.clone(),
            turn_id: turn_id.0.to_string(),
            reply_flushed_text: "streamed prefix".into(),
            activity_flushed_items: 0,
            activity_flushed_keys: Vec::new(),
        };
        let flushed: String = finalized_late_activity_lines_for_coverages(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            &[coverage],
        )
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect();
        assert!(
            !flushed.contains("Orchestrating"),
            "immutable scrollback must not bake an in-progress Orchestrating chip: {flushed:?}"
        );
        assert!(
            !flushed.contains("· 1 active"),
            "a running item must not bake a live '1 active' count into scrollback: {flushed:?}"
        );
    }

    #[test]
    fn finalized_turn_card_flush_with_running_item_is_terminal_not_orchestrating() {
        // Companion to the covered late-activity guard: the committed turn-card
        // flush (`finalized_history_lines` -> `push_turn_activity_log_section`
        // with `finalized = true`) also fed the running item into the header
        // counts, so a turn whose committed log still carried a running item
        // baked "Orchestrating… (1 active)" into immutable scrollback. #342
        // stripped the sub-agent titles here but not the running-item count.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the long job")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("run".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
                ActivityItem::new(ActivityKind::Tool, "shell", "running").with_turn(turn_id),
            ],
        });

        let flushed: String =
            finalized_history_lines(&app, Palette::for_theme(ThemeName::Slate), 100)
                .iter()
                .flat_map(|line| line.spans.iter())
                .map(|span| span.content.as_ref())
                .collect();
        assert!(
            !flushed.contains("Orchestrating"),
            "the finalized turn-card must not bake an in-progress chip: {flushed:?}"
        );
        assert!(
            !flushed.contains("· 1 active"),
            "immutable scrollback must not freeze a live '1 active' count: {flushed:?}"
        );
        // The terminal (completed) item is still recorded.
        assert!(
            flushed.contains("Agent task completed"),
            "the settled portion of the turn still records its terminal outcome: {flushed:?}"
        );
    }

    #[test]
    fn agent_task_group_title_with_pending_continuations_does_not_say_completed() {
        // Gap 2 fix #2: when the parent's tool calls are all settled (no active
        // items, no running sub-agents) but the server reports a pending
        // continuation, the title must NOT read "Agent task completed" — that
        // "looks done" lie hides the master re-entry. It must reflect
        // re-entering/continuing instead. The pending re-entry only applies to
        // the CURRENT/active group (`is_active_group = true`).
        let settled = agent_task_group_title(false, 0, 0, true);
        assert_eq!(settled, "Agent task completed", "baseline settled title");

        let reentering = agent_task_group_title(false, 0, 1, true);
        assert!(
            !reentering.to_lowercase().contains("completed")
                && !reentering.to_lowercase().contains("done"),
            "pending continuation must not read as completed/done: {reentering:?}"
        );
        assert!(
            reentering.to_lowercase().contains("re-enter")
                || reentering.to_lowercase().contains("continu"),
            "pending continuation must read as re-entering/continuing: {reentering:?}"
        );

        // In-progress still wins (orchestrating), and errors still surface even
        // with a pending continuation.
        assert!(agent_task_group_title(true, 0, 1, true).contains("Orchestrating"));
        assert!(
            agent_task_group_title(false, 2, 0, true)
                .to_lowercase()
                .contains("error")
        );
    }

    #[test]
    fn agent_task_group_title_pending_continuation_does_not_retitle_archived_group() {
        // Blocking bug 1: `pending_continuations` is the active session's queued
        // re-entry count. It is fed into EVERY group title call, including
        // ARCHIVED past-turn groups. A settled archived group (no live work)
        // must keep its "completed" title even while a continuation is pending —
        // only the CURRENT/active group may flip to "Re-entering". Guard via
        // `is_active_group = false`.
        let archived_completed = agent_task_group_title(false, 0, 1, false);
        assert_eq!(
            archived_completed, "Agent task completed",
            "archived completed group must NOT read as re-entering: {archived_completed:?}"
        );

        // An archived FAILED group must keep its failed title — `failed > 0`
        // must NOT be overridden by the active session's pending continuation.
        let archived_failed = agent_task_group_title(false, 2, 1, false);
        assert!(
            archived_failed.to_lowercase().contains("error"),
            "archived failed group must keep its failed title, not re-entering: {archived_failed:?}"
        );
        assert!(
            !archived_failed.to_lowercase().contains("re-enter"),
            "archived failed group must NOT read as re-entering: {archived_failed:?}"
        );
    }

    #[test]
    fn archived_completed_group_keeps_title_while_active_turn_continuation_pending() {
        // Blocking bug 1 (end-to-end render): a session has an ARCHIVED
        // completed turn (turn A) AND a live active turn (turn B). The server
        // reports a pending continuation for the session. The archived group
        // must STILL read "Agent task completed" — only the active turn's group
        // may flip to "Re-entering". (RED on f588b6f: the pending count was fed
        // to every group, retitling the archived completed turn.)
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let archived_turn = TurnId::new();
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("first request"),
                    Message::assistant("First answer."),
                    Message::user("second request"),
                ],
                tasks: vec![],
                // Active turn B is live (the current/active group).
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Archived completed group for turn A, anchored to the first request.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: archived_turn,
            request: Some("first request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete").with_success(true),
            ],
        });
        // Server has a continuation queued for the active session.
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Agent task completed"),
            "archived completed group must keep its title: {text:?}"
        );
    }

    #[test]
    fn archived_failed_group_keeps_failed_title_while_continuation_pending() {
        // Blocking bug 1 (end-to-end render): an ARCHIVED FAILED group must keep
        // its failed title even while a continuation is pending for the active
        // session — pending must NOT override `failed > 0` for a non-active
        // group. (RED on f588b6f: pending won over failed, losing the failed
        // title on archived groups.)
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let archived_turn = TurnId::new();
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("first request"),
                    Message::assistant("First answer."),
                    Message::user("second request"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: archived_turn,
            request: Some("first request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "failed").with_success(false),
            ],
        });
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Agent task finished with errors"),
            "archived failed group must keep its failed title: {text:?}"
        );
    }

    #[test]
    fn active_turn_group_with_pending_continuation_reads_reentering() {
        // Blocking bug 1 (pins intended behavior): the ACTIVE/current turn's
        // group (the live `live_reply` turn, archived to its log) DOES read
        // "Re-entering (continuing)…" when a continuation is pending. The active
        // group is identified by `log.turn_id == active_turn().turn_id`.
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let active_turn = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("only request")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: active_turn.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The active turn's settled tool calls are archived to its log (the
        // re-entry gap: parent calls done, continuation queued).
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: active_turn.clone(),
            request: Some("only request".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(active_turn)
                    .with_success(true),
            ],
        });
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 1,
                phase: Some("re-entering".into()),
            },
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Re-entering (continuing)"),
            "active turn group with pending continuation reads re-entering: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "active turn group must NOT read completed during the re-entry gap: {text:?}"
        );
    }

    #[test]
    fn task_group_counts_tally_full_set_not_display_cap() {
        // Render-cap bug: the chip header counted the DISPLAY-CAPPED slice (3 or
        // 12 rows), so a 66-action turn read "3 action(s) · 3 completed" even
        // though its sibling footer correctly tallied the full 66. The header
        // and footer now both call `task_group_counts` over the FULL set, so the
        // counts reflect 66 actions — not the cap.
        let mut items: Vec<ActivityItem> = Vec::new();
        // 60 completed earlier actions.
        for _ in 0..60 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "shell", "complete").with_success(true),
            );
        }
        // 2 active (still running) earlier actions.
        items.push(ActivityItem::new(
            ActivityKind::Tool,
            "run_pipeline",
            "running",
        ));
        items.push(ActivityItem::new(
            ActivityKind::Tool,
            "run_pipeline",
            "running",
        ));
        // 1 failed earlier action.
        items.push(ActivityItem::new(ActivityKind::Tool, "shell", "failed").with_success(false));
        // Last 3 (the only ones the chip renders as children) are completed.
        for _ in 0..3 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete").with_success(true),
            );
        }
        assert_eq!(items.len(), 66, "fixture sanity: 66 total actions");

        let full: Vec<&ActivityItem> = items.iter().collect();
        let (total, completed, active, failed) = task_group_counts(&full);
        assert_eq!(total, 66, "total must be the FULL set, not the display cap");
        assert_eq!(completed, 63, "60 early + 3 late completed");
        assert_eq!(active, 2, "two running actions");
        assert_eq!(failed, 1, "one failed action");

        // The display-capped slice (last 3) must NOT be what the header counts:
        // if the header tallied the cap it would read 3/3/0/0 — the original bug.
        let capped: Vec<&ActivityItem> = full.iter().rev().take(3).rev().copied().collect();
        let (cap_total, cap_completed, _, _) = task_group_counts(&capped);
        assert_eq!(cap_total, 3);
        assert_eq!(cap_completed, 3);
        assert_ne!(
            (total, completed),
            (cap_total, cap_completed),
            "header tally must differ from the display-cap tally"
        );
    }

    #[test]
    fn chip_header_counts_full_turn_set_and_agrees_with_footer() {
        // End-to-end render guard: a 66-action turn's chip HEADER must read the
        // full set ("66 action(s) · ... 66 completed") and AGREE with its sibling
        // "... +63 more" footer — not the display-capped "3 action(s) · 3
        // completed". RED on the pre-fix code: the header counted only the last
        // 3 rendered children.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut items: Vec<ActivityItem> = Vec::new();
        // 63 earlier actions, all completed.
        for _ in 0..63 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
            );
        }
        // Last 3 (the rendered children) completed too → 66 total, 66 completed.
        for _ in 0..3 {
            items.push(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_turn(turn_id.clone())
                    .with_success(true),
            );
        }
        assert_eq!(items.len(), 66);

        let app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("big turn")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let mut app = app;
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("big turn".into()),
            anchor_index: Some(0),
            items,
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("66 action(s)"),
            "header must read the full 66-action set, not the display cap: {text:?}"
        );
        assert!(
            !text.contains("3 action(s)"),
            "header must NOT read the capped 3-action slice: {text:?}"
        );
        // 63 of the 66 are hidden (only 3 rendered as children); footer tallies
        // the full set, so header and footer must agree.
        assert!(
            text.contains("+63 more"),
            "footer must report the 63 hidden actions: {text:?}"
        );
        assert!(
            text.contains("66 completed"),
            "header completed count must reflect the full set: {text:?}"
        );
    }

    #[test]
    fn agent_task_group_title_failed_active_turn_with_pending_reads_reentering() {
        // Precedence decision for the ACTIVE group: a failed active turn that
        // the server is genuinely continuing (pending_continuations > 0) reads
        // "Re-entering (continuing)…" — the queued continuation is the live
        // truth (the failure is being retried/continued), so it wins over the
        // failed title FOR THE ACTIVE GROUP ONLY.
        let active_failed_pending = agent_task_group_title(false, 1, 1, true);
        assert!(
            active_failed_pending.to_lowercase().contains("re-enter")
                || active_failed_pending.to_lowercase().contains("continu"),
            "failed active turn that is continuing reads re-entering: {active_failed_pending:?}"
        );

        // A failed active turn with NO pending continuation still reads as
        // failed (no continuation queued → it really did finish with errors).
        let active_failed = agent_task_group_title(false, 1, 0, true);
        assert!(
            active_failed.to_lowercase().contains("error"),
            "failed active turn with no continuation reads as failed: {active_failed:?}"
        );
    }

    #[test]
    fn leaked_running_item_in_terminal_turn_log_does_not_pin_orchestrating() {
        // Orphan activity-chip self-heal: a `ToolStarted` whose matching
        // `ToolCompleted` never arrived (a leaked spawn_only chip / any future
        // uncovered path) leaves a "running"-status item bound to the turn. When
        // the turn reaches its terminal state, `capture_completed_turn_activity`
        // archives the turn's activity AND reconciles the stranded running item
        // to a terminal status. With no live work and no running sub-agents, the
        // captured chip must NOT stay pinned on "Orchestrating…" — its turn is
        // over. This is the path that reappears after a reconnect: hydrate
        // replays the unbalanced started-state and the turn re-completes through
        // the same capture, healing the residual chip.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("run the background job"),
                    Message::assistant("Kicked off the background job."),
                ],
                // No live_reply → this turn is terminal / not the active turn,
                // and no sub-agent tasks remain.
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Leaked started-state in the turn's live activity: status never reached
        // terminal because no `ToolCompleted` arrived.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-leaked"),
        );
        // The turn went terminal: capturing it must self-heal the leaked item.
        assert!(app.capture_completed_turn_activity(&session_id, &turn_id));

        let text = rendered_text(&app);
        assert!(
            !text.contains("Orchestrating"),
            "a leaked running item in a terminal turn must not pin Orchestrating: {text:?}"
        );
        assert!(
            !text.contains("1 active"),
            "the leaked item must not be counted as active once its turn is terminal: {text:?}"
        );
    }

    #[test]
    fn leaked_running_item_in_active_turn_still_shows_orchestrating() {
        // Guard against over-suppression: a "running" item whose turn IS the
        // session's currently-active turn (live_reply present) is genuine
        // in-flight work and MUST still read as Orchestrating. The self-heal
        // only fires when the turn is captured as terminal; an active turn's
        // live activity is never captured/reconciled.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the live job")],
                tasks: vec![],
                // Active turn: live_reply present and pointing at turn_id.
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-live"),
        );

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating"),
            "the active turn's in-flight work must still read as Orchestrating: {text:?}"
        );
    }

    #[test]
    fn open_menu_suppresses_the_live_orchestrating_chip() {
        // "Duplicated orchestrating after slash commands": opening a reserved-row
        // menu squeezes the live tail to its 1-row floor, so the in-flight chip
        // renders as a lone truncated "Orchestrating…" header. The menu-open
        // viewport grow can scroll that squeezed header into real scrollback
        // where the menu-close clear can't reclaim it, stranding a frozen
        // duplicate above the fresh chip. The fix suppresses the chip while a
        // menu holds focus — with no squeezed header there is nothing to strand.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id,
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the live job")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "run_pipeline", "running")
                .with_turn(turn_id)
                .with_tool_call("call-live"),
        );

        // Baseline: no menu → the active turn reads as Orchestrating.
        assert!(
            rendered_text(&app).contains("Orchestrating"),
            "baseline: the in-flight chip must show while no menu is open"
        );

        // Open a reserved-row menu (a slash/command popup is `app.active_menu`).
        app.menu_stack.open("slash.test");
        app.active_menu = Some(crate::menu::MenuBuildResult::ready(
            crate::menu::MenuSpec::new(
                "slash.test",
                "Slash test",
                crate::menu::MenuMode::SingleSelect,
            )
            .with_items(vec![crate::menu::MenuItem::new(
                "slash.item.0",
                "Item 0",
                crate::menu::MenuAction::Noop,
            )]),
        ));

        // With the menu open the strandable squeezed chip is not painted.
        assert!(
            !rendered_text(&app).contains("Orchestrating"),
            "the live chip must be suppressed while a menu holds focus (nothing to strand)"
        );
    }

    #[test]
    fn subagents_attributed_per_turn_not_double_counted() {
        // C5 regression: two turns each spawn sub-agents. Before C1's `turn_id`
        // landed on the task wire, `running_subagent_titles_for_chip` returned the
        // GLOBAL active count for every chip matching active-OR-latest, so both
        // turns' chips lit up "Orchestrating" with the same total ("two chips").
        // Now each chip counts ONLY its own turn's running tasks; turn-less tasks
        // (server couldn't stamp them) fall back to a SINGLE current chip.
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let running = |title: &str, turn: Option<TurnId>| crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: title.into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: turn,
        };
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("two turns of agents")],
                tasks: vec![
                    running("a1", Some(turn_a.clone())),
                    running("b1", Some(turn_b.clone())),
                    running("b2", Some(turn_b.clone())),
                    // Turn-less (legacy / replay / synthetic) → single current chip.
                    running("orphan", None),
                ],
                // turn_a is the live/active turn.
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_a.clone(),
                    text: String::new(),
                }),
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_b.clone(),
            request: Some("earlier turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn_b.clone())
                    .with_success(true),
            ],
        });

        // turn_a (active) chip: its own 1 running task + the orphan (None → the
        // single current chip, which is the active turn).
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_a)).len(),
            2
        );
        // turn_b chip: its own 2 running tasks — NOT the global 4, NOT the orphan.
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_b)).len(),
            2
        );
        // The pre-C5 bug would have returned the global active count (4) for BOTH.
        assert_ne!(
            running_subagent_titles_for_chip(&app, Some(&turn_a)).len(),
            running_subagent_titles_for_chip(&app, Some(&turn_b)).len() + 2,
            "chips must not both report the global total"
        );
    }

    #[test]
    fn turnless_tasks_fall_back_to_active_session_not_a_newer_other_session_log() {
        // codex P2: the None-fallback chip is "this session's latest turn", not
        // the globally-latest log. A *different* session having the newest
        // activity log must not steal the active session's turn-less task.
        let turn_active = TurnId::new();
        let turn_other = TurnId::new();
        let active_id = SessionKey("local:active".into());
        let other_id = SessionKey("local:other".into());
        let orphan = crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: "orphan".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: None,
        };
        // Active session (index 0) has the turn-less task but NO live_reply and
        // NO log; the other session owns the globally-newest log.
        let mut app = AppState::new(
            vec![
                SessionView {
                    id: active_id.clone(),
                    title: "active".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("active session")],
                    tasks: vec![orphan],
                    live_reply: None,
                },
                SessionView {
                    id: other_id.clone(),
                    title: "other".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("other session")],
                    tasks: vec![],
                    live_reply: None,
                },
            ],
            0,
            "ready".into(),
            None,
            false,
        );
        // Active session's log first, then a NEWER log for the OTHER session.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: active_id,
            turn_id: turn_active.clone(),
            request: Some("active turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete").with_success(true),
            ],
        });
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: other_id,
            turn_id: turn_other.clone(),
            request: Some("other turn".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete").with_success(true),
            ],
        });

        // The orphan attaches to the active session's own latest turn…
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_active)).len(),
            1
        );
        // …and NOT to the other (globally-newest) session's turn.
        assert_eq!(
            running_subagent_titles_for_chip(&app, Some(&turn_other)).len(),
            0
        );
    }

    #[test]
    fn subagent_progress_folds_into_orchestrating_chip_not_a_second_chip() {
        // mini5 soak: a parallel-spawn turn rendered TWO "Orchestrating" chips —
        // the parent turn's chip (spawn calls + "N sub-agent(s) running") AND a
        // phantom turn-less chip made of the sub-agents' own progress rows. The
        // progress rows must fold into the parent chip as children → exactly ONE
        // orchestrating chip, with the sub-agents listed under it.
        let turn = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let running = |title: &str| crate::model::TaskView {
            id: octos_core::TaskId::new(),
            title: title.into(),
            state: TaskRuntimeState::Running,
            runtime_detail: None,
            output_tail: String::new(),
            turn_id: Some(turn.clone()),
        };
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run parallel agents")],
                tasks: vec![
                    running("openclaw-deep-analysis"),
                    running("hermes-deep-analysis"),
                ],
                // Parent turn finished; its sub-agents keep running.
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Parent turn's spawn tool-calls, logged + completed.
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn.clone(),
            request: Some("run parallel agents".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                    .with_turn(turn.clone())
                    .with_success(true),
            ],
        });
        // The sub-agents' own live progress rows (turn-less) — the phantom-chip
        // source. These must NOT form their own chip.
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "openclaw-deep-analysis",
            "running",
        ));
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            "hermes-deep-analysis",
            "running",
        ));

        let text = rendered_text(&app);
        assert_eq!(
            text.matches("Orchestrating").count(),
            1,
            "exactly one Orchestrating chip (the phantom must fold in): {text:?}"
        );
        assert!(
            text.contains("2 sub-agent(s) running"),
            "the orchestrating chip surfaces the count: {text:?}"
        );
        assert!(
            text.contains("openclaw-deep-analysis") && text.contains("hermes-deep-analysis"),
            "the running sub-agents are folded in as children: {text:?}"
        );
    }

    #[test]
    fn subagent_progress_suppressed_only_when_a_matching_task_exists() {
        // codex P2: a turn-less running progress row is folded (suppressed from
        // the flow) ONLY if a running sub-agent task with the same title exists —
        // otherwise it has nothing to fold into and must stay visible, not vanish.
        let turn = TurnId::new();
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("x")],
                tasks: vec![crate::model::TaskView {
                    id: octos_core::TaskId::new(),
                    title: "alpha".into(),
                    state: TaskRuntimeState::Running,
                    runtime_detail: None,
                    output_tail: String::new(),
                    turn_id: Some(turn),
                }],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        let matched = ActivityItem::new(ActivityKind::Progress, "alpha", "running");
        let orphan = ActivityItem::new(ActivityKind::Progress, "ghost", "running");
        assert!(
            is_subagent_progress(&app, &matched),
            "a progress row with a matching running task folds in → suppressed"
        );
        assert!(
            !is_subagent_progress(&app, &orphan),
            "a progress row with NO matching task must stay visible, not vanish"
        );
    }

    #[test]
    fn active_and_delivering_sub_agents_count_as_running() {
        // Regression: the server reports non-terminal task states beyond
        // running/queued (TaskRuntimeState::Active -> "active",
        // "delivering_outputs"). They must classify as running, else the
        // agent-task group title flips to "Agent task completed" while a
        // sub-agent is still working.
        for status in ["active", "delivering_outputs", "running", "queued", "42%"] {
            assert!(
                is_running_activity(&ActivityItem::new(ActivityKind::Tool, "spawn", status)),
                "status {status:?} should count as running"
            );
        }
        for status in [
            "completed",
            "complete",
            "done",
            "success",
            "failed",
            "error",
            "cancelled",
        ] {
            assert!(
                !is_running_activity(&ActivityItem::new(ActivityKind::Tool, "spawn", status)),
                "terminal status {status:?} should NOT count as running"
            );
        }

        // ...and the group title reflects it.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("do multi-agent work")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("do multi-agent work".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "spawn", "active").with_turn(turn_id.clone()),
                ActivityItem::new(ActivityKind::Tool, "deep_research", "delivering_outputs")
                    .with_turn(turn_id),
            ],
        });

        let text = rendered_text(&app);
        assert!(
            text.contains("Orchestrating..."),
            "active/delivering sub-agents must keep the running title: {text:?}"
        );
        assert!(
            !text.contains("Agent task completed"),
            "must NOT show completed while sub-agents are active/delivering"
        );
    }

    #[test]
    fn render_code_fences_show_language_and_bound_long_lines() {
        let palette = Palette::for_theme(ThemeName::Codex);
        let long_code = format!(
            "let value = \"{}TAIL_UNIQUE_SHOULD_NOT_RENDER\";",
            "x".repeat(180)
        );
        let content = format!("```rust\n{long_code}\n```");
        let mut lines = Vec::new();

        push_formatted_body(
            &mut lines,
            palette,
            &content,
            "",
            Some(palette.surface),
            120,
        );

        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("┌─ "));
        assert!(text.contains("rust"));
        assert!(text.contains("└─"));
        assert!(!text.contains("end code"));
        assert!(text.contains("let value ="));
        assert!(text.contains(" ..."));
        assert!(!text.contains("TAIL_UNIQUE_SHOULD_NOT_RENDER"));
    }

    #[test]
    fn tool_card_children_are_indented_under_the_group_header() {
        // A tool activity renders as a `⏺ Bash(...)` card, but it is always a
        // CHILD of the agent-task group header (`◠ Orchestrating…` / `• Agent
        // task …`). Its bullet must be indented so it nests under the header
        // instead of sitting flush at column 0 where it reads as a sibling
        // (user report: "bash commands should be indented").
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::assistant("done")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("wait-1")
                .with_detail("curl -L https://example.com -o /tmp/x.dmg")
                .with_success(true)
                .with_output_preview("downloaded 120MB")
                .with_duration_ms(3200),
        );

        // Expanded so the settled group shows its child rows (a live/running
        // turn shows them too — same non-collapsed path the user hit).
        app.expanded_tool_outputs = true;
        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&app, palette));

        let card_row = rows
            .iter()
            .find(|row| row.contains("Bash($ curl"))
            .unwrap_or_else(|| panic!("no Bash card row; got:\n{}", rows.join("\n")));
        assert!(
            card_row.starts_with("  ⏺")
                || card_row.starts_with("  ") && card_row.trim_start().starts_with('⏺'),
            "the tool card bullet must be indented as a group child, got: {card_row:?}"
        );
        assert!(
            !card_row.trim_end().starts_with("⏺"),
            "the tool card must NOT be flush at column 0: {card_row:?}"
        );

        // The `⎿` output preview nests one step further under the indented card.
        let preview_row = rows
            .iter()
            .find(|row| row.contains("downloaded 120MB"))
            .expect("preview row");
        assert!(
            preview_row.starts_with("    ⎿"),
            "the preview must nest under the indented card, got: {preview_row:?}"
        );
    }

    #[test]
    fn render_activity_uses_action_keywords_for_wait_and_file_tools() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show activity verbs")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("wait-1")
                .with_detail("sleep 20; tmux capture-pane")
                .with_success(true)
                .with_duration_ms(20_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "write_file", "complete")
                .with_tool_call("write-1")
                .with_detail("src/lib.rs")
                .with_success(true)
                .with_duration_ms(18),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("⏺ Bash($ sleep 20"));
        assert!(text.contains("20s"));
        assert!(text.contains("⏺ Write(src/lib.rs"));
        assert!(text.contains("18ms"));
        assert!(!text.contains("Command  ▸ shell"));
        assert!(!text.contains("Tool  ▸ write_file"));
    }

    #[test]
    fn render_activity_shows_bash_command_not_raw_json_args() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run a bash command")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The codex-style `bash` tool carries its command in `arguments.cmd`
        // (no `detail`), unlike `shell`/`exec` which set `detail`. It must
        // still render as a real `$ command` line, not the raw JSON blob.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_tool_call("bash-1")
                .with_arguments(serde_json::json!({
                    "cmd": "find . -name '*.ts' -newer server"
                }))
                .with_success(true)
                .with_duration_ms(8),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        // Claude-Code-style card: `⏺ Bash(cmd)`, clean command, no JSON.
        assert!(
            text.contains("⏺ Bash($ find . -name '*.ts' -newer server)"),
            "want Claude-Code-style bash card, got:\n{text}"
        );
        assert!(
            !text.contains("call bash-1"),
            "must not show the call id, got:\n{text}"
        );
        // No raw JSON arguments leaking through.
        assert!(
            !text.contains("{\"cmd\""),
            "must not show raw JSON args, got:\n{text}"
        );
    }

    #[test]
    fn render_spawn_and_multiline_tool_cards_claude_code_style() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("spawn + multiline")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // spawn's task (projected into `detail`) renders as `⏺ Spawn(task)`.
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "spawn", "complete")
                .with_tool_call("spawn-1")
                .with_detail("Restart the Vite dev server")
                .with_success(true)
                .with_duration_ms(2500),
        );
        // A multi-line command keeps both lines (indented under `(`).
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                .with_tool_call("bash-2")
                .with_detail("cd /srv\nnpm run dev")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(
            text.contains("⏺ Spawn(Restart the Vite dev server)"),
            "spawn must show its task, got:\n{text}"
        );
        assert!(text.contains("⏺ Bash($ cd /srv"), "got:\n{text}");
        assert!(
            text.contains("npm run dev)"),
            "multi-line command must keep its second line, got:\n{text}"
        );
        assert!(
            !text.contains("spawn-1") && !text.contains("bash-2"),
            "must not show call ids, got:\n{text}"
        );
    }

    #[test]
    fn compaction_notice_renders_prominently_with_marker() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id,
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // The persistent "context compacted" notice must stand out from the
        // muted activity stream so it isn't lost in a busy session.
        app.push_activity(ActivityItem::new(
            ActivityKind::Progress,
            t!("status.activity_context_compacted").into_owned(),
            "120k → 40k tokens",
        ));
        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);
        assert!(
            text.contains("✦ context compacted"),
            "compaction notice must render with a prominent marker, got:\n{text}"
        );
        assert!(text.contains("120k → 40k tokens"), "got:\n{text}");
    }

    #[test]
    fn compaction_completed_event_renders_prominent_notice_end_to_end() {
        use octos_core::app_ui::AppUiEvent;
        use octos_core::ui_protocol::{
            ContextCompactionCompletedEvent, UiContextCompactionRecord, UiContextState,
            UiNotification,
        };
        let session_id = SessionKey("local:test".into());
        // Compaction is reported DURING a turn — give the session a live reply
        // so the notice is turn-stamped (else it is suppressed mid-turn).
        let turn_id = TurnId::new();
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: session_id.clone(),
                    title: "test".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![Message::user("do heavy work")],
                    tasks: vec![],
                    live_reply: Some(crate::model::LiveReply {
                        turn_id,
                        text: String::new(),
                    }),
                }],
                0,
                "ready".into(),
                None,
                false,
            ),
        };

        store.apply_event(AppUiEvent::Protocol(
            UiNotification::ContextCompactionCompleted(ContextCompactionCompletedEvent {
                session_id: session_id.clone(),
                context_state: UiContextState {
                    session_id: session_id.clone(),
                    thread_id: None,
                    generation: 4,
                    transcript_hash: "abc123".into(),
                    item_count: 42,
                    token_estimate: 40_000,
                    recovery_state: "healthy".into(),
                    last_checkpoint_id: None,
                    last_compaction_id: Some("comp-001".into()),
                },
                compaction: UiContextCompactionRecord {
                    compaction_id: "comp-001".into(),
                    checkpoint_id: "chk-001".into(),
                    status: "applied".into(),
                    policy_id: "default".into(),
                    trigger: "token_budget".into(),
                    input_generation: 3,
                    output_generation: Some(4),
                    input_transcript_hash: "input-h".into(),
                    replacement_transcript_hash: Some("abc123".into()),
                    installed_transcript_hash: Some("abc123".into()),
                    input_item_count: 130,
                    retained_count: 42,
                    dropped_count: 88,
                    summary_item_id: Some("sum-1".into()),
                    token_estimate_before: 120_000,
                    token_estimate_after: Some(40_000),
                    error: None,
                },
            }),
        ));

        store.state.expanded_tool_outputs = true;
        let text = rendered_text(&store.state);
        // Full path: Completed event → persistent notice → prominent ✦ render.
        assert!(
            text.contains("✦ context compacted"),
            "a real compaction Completed event must render the prominent notice, got:\n{text}"
        );
    }

    #[test]
    fn render_file_mutation_progress_as_separate_activity_block() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show file mutation")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(
                ActivityKind::Progress,
                "file_mutation",
                "File mutation: modify /tmp/work/blue-origin/src/pages/index.astro",
            )
            .with_detail("modify /tmp/work/blue-origin/src/pages/index.astro | diff preview ready"),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("Changed"));
        assert!(text.contains(".../blue-origin/src/pages/index.astro"));
        assert!(!text.contains("preview ready"));
        assert!(!text.contains("File mutation: modify /tmp/work"));
    }

    #[test]
    fn render_short_terminal_keeps_user_prompt_visible_above_activity() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("keep this prompt visible")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "working".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("keep this prompt visible"));
        assert!(text.contains("Composer"));
    }

    #[test]
    fn render_transcript_scroll_bottom_counts_wrapped_rows_above_composer() {
        let long_body = (1..=18)
            .map(|idx| {
                format!(
                    "wrapped paragraph {idx:02} {}",
                    "中文内容 mixed ascii text ".repeat(5)
                )
            })
            .chain(std::iter::once(
                "final wrapped row should remain visible BOTTOM_VISIBLE_UNIQUE".to_string(),
            ))
            .collect::<Vec<_>>()
            .join("\n\n");
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("show long answer"),
                    Message::assistant(long_body),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 56, 20);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let rows = rendered_rows(&buffer);
        assert!(text.contains("BOTTOMVISIBLEUNIQUE"));
        assert!(text.contains("Composer"));
        assert!(!text.contains("Work  sticky"));
        let final_row = row_index_containing(&rows, "BOTTOMVISIBLEUNIQUE");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            final_row < composer_row,
            "final transcript row must stay above composer: final={final_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_long_active_turn_follows_tail_when_prompt_block_overflows() {
        let turn_id = TurnId::new();
        let live_reply = (1..=16)
            .map(|idx| format!("live answer row {idx:02} {}", "wrapped content ".repeat(4)))
            .chain(std::iter::once(
                "LIVETAILVISIBLEUNIQUE should stay visible above composer".to_string(),
            ))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("done?")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: live_reply,
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 24);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("LIVETAILVISIBLEUNIQUE"));
        assert!(text.contains("Composer"));
        let tail_row = row_index_containing(&rows, "LIVETAILVISIBLEUNIQUE");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            tail_row < composer_row,
            "active turn tail must stay above composer: tail={tail_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_active_turn_answer_precedes_progress_and_hides_stale_activity() {
        let old_turn_id = TurnId::new();
        let current_turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build the site"),
                    Message::assistant("Started the site build."),
                    Message::user("done?"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: current_turn_id,
                    text: "Not yet - the build is still running.".into(),
                }),
            }],
            0,
            "thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(old_turn_id)
                .with_detail("cargo build from prior turn")
                .with_success(true),
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 80, 24);
        let rows = rendered_rows(&buffer);
        let text = rows.join("\n");

        assert!(text.contains("done?"));
        assert!(text.contains("Not yet - the build is still running."));
        assert!(
            !text.contains("cargo build from prior turn"),
            "prior-turn activity must not render under the latest user prompt"
        );
        assert!(
            !rows
                .iter()
                .any(|row| matches!(row.trim(), "◐" | "◓" | "◑" | "◒")),
            "live assistant text must not render a second standalone spinner row"
        );
        let prompt_row = row_index_containing(&rows, "done?");
        let answer_row = row_index_containing(&rows, "Not yet - the build is still running.");
        let composer_row = row_index_containing(&rows, "Composer");
        assert!(
            prompt_row < answer_row && answer_row < composer_row,
            "latest prompt should be followed by live answer before composer: prompt={prompt_row}, answer={answer_row}, composer={composer_row}"
        );
    }

    #[test]
    fn render_live_answer_activity_without_sticky_round_plan() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("review the project")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "The project review found two issues.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_turn(turn_id)
                .with_tool_call("read-1")
                .with_detail("src/main.rs")
                .with_success(true),
        );

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Codex), 96, 28);
        let rows = rendered_rows(&buffer);
        let answer_row = row_index_containing(&rows, "The project review found two issues.");
        let activity_row = row_index_containing(&rows, "Agent task completed");

        assert!(
            answer_row < activity_row,
            "live answer should be followed by its activity log: answer={answer_row}, activity={activity_row}"
        );
        let text = rows.join("\n");
        assert!(!text.contains("Plan rounds"));
        assert!(!text.contains("Current round: review the project"));
        assert!(!text.contains("Work  sticky"));
    }

    #[test]
    fn render_tool_blocks_show_state_preview_failure_and_collapsed_detail() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show tool states")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("preview-1")
                .with_detail("cargo test")
                .with_output_preview("6 passed")
                .with_success(true)
                .with_duration_ms(1200),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "failed")
                .with_tool_call("fail-1")
                .with_detail("npm install")
                .with_success(false)
                .with_duration_ms(70_000),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                .with_tool_call("collapsed-1")
                .with_detail("src/lib.rs")
                .with_success(true),
        );

        app.expanded_tool_outputs = true;
        let text = rendered_text(&app);

        assert!(text.contains("✗"));
        assert!(text.contains("⏺"));
        assert!(text.contains("70s"));
        assert!(text.contains("6 passed"));
    }

    #[test]
    fn render_tool_output_expands_with_global_toggle_state() {
        let output = (1..=10)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show output")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_tool_call("preview-1")
                .with_detail("cargo test")
                .with_output_preview(output)
                .with_success(true),
        );

        let collapsed = rendered_text(&app);
        // New contract: a settled group collapses to its one-line header — no
        // child rows, no per-tool preview hint, until Ctrl+O expands.
        assert!(!collapsed.contains("line10"));
        assert!(!collapsed.contains("cargo test"));
        assert!(collapsed.contains("(1"));

        app.expanded_tool_outputs = true;
        let expanded = rendered_text(&app);
        assert!(expanded.contains("line10"));
        assert!(expanded.contains("expanded (Ctrl+O collapse)"));
    }

    #[test]
    fn render_expanded_tool_output_remains_bounded() {
        let output = (1..=40)
            .map(|line| format!("output-line-{line:02}-unique"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show bounded output")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.expanded_tool_outputs = true;
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_detail("cargo test -- --nocapture")
                .with_output_preview(output)
                .with_success(true),
        );

        let text = rendered_text(&app);

        assert!(text.contains("output-line-24-unique"));
        assert!(!text.contains("output-line-40-unique"));
        assert!(text.contains("16 more line(s) hidden (Ctrl+O collapse)"));
    }

    #[test]
    fn render_active_turn_progress_uses_spinner_without_logs_or_timestamps() {
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("think")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: String::new(),
                }),
            }],
            0,
            "Queued turn/start".into(),
            None,
            false,
        );

        let text = rendered_text(&app);

        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert!(!text.contains("INFO "));
        assert!(!text.contains("2026-"));
        assert!(!text.contains("tool_ids="));
    }

    #[test]
    fn render_inline_approval_card_names_request_and_session_actions() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::system("ready"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run command".into(),
            body: "cargo test".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Run command"));
        assert!(text.contains("shell"));
        assert!(text.contains("y = approve this command once"));
        assert!(text.contains("s = approve this command/scope for the session"));
        assert!(text.contains("n = deny it"));
    }

    #[test]
    fn render_blocked_turn_keeps_latest_user_prompt_visible_near_approval() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("older prompt"),
                    Message::assistant("older answer"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Planning a safe M9 scaffold over mock transport.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating prototype context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Mock approval boundary".into(),
            body: "approve?".into(),
            approval_kind: Some("command".into()),
            risk: Some("low".into()),
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Mock approval boundary"));
    }

    #[test]
    fn pending_decision_card_stays_visible_when_the_live_tail_overflows() {
        // The reported trap: a turn parked on an approval streams a wall of output
        // that pushes the (top-rendered) card off the height-clipped live tail, so
        // the user sees a bare "Waiting" with no prompt and a locked composer. The
        // card now renders LAST, so it sits in the always-visible bottom region.
        let streamed: String = (0..60)
            .map(|idx| format!("streamed line {idx:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the thing")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: streamed,
                }),
            }],
            0,
            "Working".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Delete the database".into(),
            body: "approve?".into(),
            approval_kind: Some("command".into()),
            risk: Some("high".into()),
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        // A short viewport forces the live tail to clip.
        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 120, 16);
        let rows = rendered_rows(&buffer);
        let screen = rows.join("\n");

        assert!(
            screen.contains("Approval Requested") && screen.contains("y = approve this command"),
            "the parked-decision card must survive live-tail clipping: {rows:?}"
        );
        // Prove the tail actually overflowed — the earliest streamed line clipped
        // off the top, so the card's visibility is not an artifact of everything
        // fitting on screen.
        assert!(
            !screen.contains("streamed line 00"),
            "expected the live tail to overflow (early streamed line clipped): {rows:?}"
        );
    }

    #[test]
    fn parked_decision_banner_appears_only_after_the_escalation_threshold() {
        // Fix 3: the watchdog surfaces a prominent banner once a turn has been
        // parked on a decision past the threshold. Below threshold it reserves no
        // rows; above threshold it reserves exactly one and advertises recovery.
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "t".into(),
                profile_id: Some("coding".into()),
                // A committed message keeps the fresh-launch banner off, so the
                // normal viewport (with the decision-banner pane) renders.
                messages: vec![Message::user("run the thing")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.approval = Some(ApprovalModalState {
            session_id: session_id.clone(),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run".into(),
            body: "approve?".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });
        app.run_state = SessionRunState::Blocked {
            message: "Run command".into(),
        };

        // Just parked (below threshold): no banner reserved.
        app.run_state_started_at = Some(std::time::Instant::now());
        assert_eq!(decision_banner_height(&app), 0);

        // Parked past the threshold: exactly one reserved banner row.
        app.run_state_started_at = std::time::Instant::now().checked_sub(
            std::time::Duration::from_secs(PARKED_DECISION_ESCALATE_SECS + 5),
        );
        assert_eq!(decision_banner_height(&app), 1);
        let rows = rendered_rows(&rendered_buffer(&app, Palette::for_theme(ThemeName::Slate)));
        assert!(
            rows.iter()
                .any(|row| row.contains("Parked on you") && row.contains("Ctrl+R/Alt+A")),
            "the escalation banner must advertise the recovery keys: {rows:?}"
        );

        // No pending decision → no banner even past the threshold.
        app.approval = None;
        assert_eq!(decision_banner_height(&app), 0);
    }

    #[test]
    fn render_compact_blocked_turn_keeps_latest_user_prompt_visible() {
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("older prompt"),
                    Message::assistant("older answer"),
                    Message::user("complete m9 contract"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: TurnId::new(),
                    text: "Planning a safe M9 scaffold over mock transport.".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        for idx in 0..8 {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "read_file", "complete")
                    .with_detail(format!("Hydrating prototype context {idx}"))
                    .with_output_preview("1 | pub fn demo() {}")
                    .with_success(true)
                    .with_duration_ms(420),
            );
        }
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Mock approval boundary".into(),
            body: "approve?".into(),
            approval_kind: Some("command".into()),
            risk: Some("low".into()),
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("complete m9 contract"));
        assert!(text.contains("Mock approval boundary"));
    }

    #[test]
    fn render_diff_preview_modal_omits_default_status_and_source_labels() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Roman numeral patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/roman.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "todo!()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "Ok(42)".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                        ],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("Diff Preview"));
        assert!(text.contains("Roman numeral patch"));
        // The default single-variant labels ("ready" / "pending_store") must
        // not render in the header. Slice the header region (the title through
        // the hint line that immediately follows it) so the word "ready" in the
        // unrelated bottom status bar can't mask a regression.
        let title = text.find("Roman numeral patch").expect("title in header");
        let hint = text.find("select hunk").expect("hint after header");
        let header_region = &text[title..hint];
        assert!(
            !header_region.contains("ready"),
            "default status label must be suppressed: {header_region:?}"
        );
        assert!(
            !header_region.contains("pending_store"),
            "default source label must be suppressed: {header_region:?}"
        );
        assert!(text.contains("modified"));
        assert!(text.contains("src/roman.rs"));
        assert!(text.contains("@@ -1 +1 @@"));
        assert!(text.contains("todo!()"));
        assert!(text.contains("Ok(42)"));
    }

    #[test]
    fn ctrl_o_expands_diff_preview_to_full_selected_hunk() {
        // The collapsed inline diff caps each hunk at 4 lines — the "Tab doesn't
        // expand the diff" complaint. Ctrl+O (expanded_tool_outputs) must reveal
        // the SELECTED hunk's complete body, and the hidden-lines hint must
        // point at that working key (was a misleading "(Tab inspector)").
        let make = || DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Big patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/big.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1,6 +1,6 @@".into(),
                        lines: (1u32..=6)
                            .map(|n| DiffPreviewLine {
                                kind: "added".into(),
                                content: format!("line {n} content"),
                                old_line: None,
                                new_line: Some(n),
                            })
                            .collect(),
                    }],
                }],
            },
        };

        // Collapsed (default): capped at 4 lines, hint points to Ctrl+O.
        let collapsed = rendered_text(&app_with_diff(make()));
        assert!(collapsed.contains("line 4 content"));
        assert!(
            !collapsed.contains("line 5 content"),
            "5th line hidden when collapsed: {collapsed:?}"
        );
        assert!(
            collapsed.contains("Ctrl+O expand"),
            "hidden-lines hint must point at the working key: {collapsed:?}"
        );

        // Expanded (Ctrl+O): full selected hunk, no truncation hint.
        let mut app = app_with_diff(make());
        app.expanded_tool_outputs = true;
        let expanded = rendered_text(&app);
        assert!(
            expanded.contains("line 5 content") && expanded.contains("line 6 content"),
            "all lines of the selected hunk shown when expanded: {expanded:?}"
        );
        assert!(
            !expanded.contains("more diff line(s) hidden"),
            "no truncation hint when expanded: {expanded:?}"
        );
    }

    #[test]
    fn diff_box_hidden_when_no_usable_hunks() {
        // C6 (mini5 soak): an auto-opened preview whose file carries no hunks
        // ("line diff unavailable for this mutation") must hide the whole box —
        // no "Diff Preview" header, no dead "[/] select hunk | c stage" UI.
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Empty mutation".into()),
                files: vec![DiffPreviewFile {
                    path: "src/empty.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(
            !text.contains("Diff Preview"),
            "diff box must be hidden when no usable hunks: {text:?}"
        );
        assert!(
            !text.contains("select hunk"),
            "dead hunk-select UI must not render: {text:?}"
        );
        assert!(!text.contains("line diff unavailable"));
    }

    #[test]
    fn render_inline_diff_uses_codex_style_add_delete_colors() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Color patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/color.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "old_value()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "new_value()".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                        ],
                    }],
                }],
            },
        });
        let palette = Palette::for_theme(ThemeName::Codex);
        let buffer = rendered_buffer(&app, palette);

        let removed_style = style_for_text(&buffer, "old_value()").expect("removed line style");
        let added_style = style_for_text(&buffer, "new_value()").expect("added line style");
        let hunk_style = style_for_text(&buffer, "@@ -1 +1 @@").expect("hunk style");

        assert_eq!(removed_style.fg, Some(palette.danger));
        assert_eq!(removed_style.bg, Some(palette.danger_bg));
        assert_eq!(added_style.fg, Some(palette.success));
        assert_eq!(added_style.bg, Some(palette.success_bg));
        assert_eq!(hunk_style.fg, Some(palette.accent));
        assert_eq!(hunk_style.bg, Some(palette.diff_context_bg));
        assert!(
            inline_diff_marker_style_for_test("added", palette)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(
            inline_diff_style_for_test("removed", palette).bg,
            Some(palette.danger_bg)
        );
    }

    #[test]
    fn render_inline_diff_header_shows_file_badge_and_counts() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Header patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![
                            DiffPreviewLine {
                                kind: "removed".into(),
                                content: "old_value()".into(),
                                old_line: Some(1),
                                new_line: None,
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "new_value()".into(),
                                old_line: None,
                                new_line: Some(1),
                            },
                            DiffPreviewLine {
                                kind: "added".into(),
                                content: "another_value()".into(),
                                old_line: None,
                                new_line: Some(2),
                            },
                        ],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("RUST"));
        assert!(text.contains("modified"));
        assert!(text.contains("+2"));
        assert!(text.contains("-1"));
        assert!(text.contains("src/lib.rs"));
    }

    #[test]
    fn render_inline_diff_shows_selected_hunk_not_always_first() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Selected hunk patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![
                        DiffPreviewHunk {
                            header: "@@ -1 +1 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "first_change()".into(),
                                old_line: None,
                                new_line: Some(1),
                            }],
                        },
                        DiffPreviewHunk {
                            header: "@@ -20 +20 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "second_change()".into(),
                                old_line: None,
                                new_line: Some(20),
                            }],
                        },
                    ],
                }],
            },
        });
        app.diff_preview.selected_hunk = 1;

        let text = rendered_text(&app);

        assert!(text.contains("@@ -20 +20 @@"));
        assert!(text.contains("second_change()"));
        assert!(!text.contains("first_change()"));
    }

    #[test]
    fn diff_preview_result_selects_first_changed_hunk() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Default hunk patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![
                        DiffPreviewHunk {
                            header: "@@ metadata @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "context".into(),
                                content: "unchanged metadata".into(),
                                old_line: Some(1),
                                new_line: Some(1),
                            }],
                        },
                        DiffPreviewHunk {
                            header: "@@ -20 +20 @@".into(),
                            lines: vec![DiffPreviewLine {
                                kind: "added".into(),
                                content: "first_real_change()".into(),
                                old_line: None,
                                new_line: Some(20),
                            }],
                        },
                    ],
                }],
            },
        });

        assert_eq!(app.diff_preview.selected_file, 0);
        assert_eq!(app.diff_preview.selected_hunk, 1);
    }

    #[test]
    fn render_inline_diff_and_approval_share_chat_flow() {
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Visible patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +1 @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "added".into(),
                            content: "new line".into(),
                            old_line: None,
                            new_line: Some(1),
                        }],
                    }],
                }],
            },
        });
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "diff_edit".into(),
            title: "Approval should be behind diff".into(),
            body: "approve?".into(),
            approval_kind: None,
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        let text = rendered_text(&app);

        assert!(text.contains("Diff Preview"));
        assert!(text.contains("Visible patch"));
        assert!(text.contains("Approval Requested"));
        assert!(text.contains("Approval should be behind diff"));
        assert!(text.contains("y = approve this command once"));
    }

    #[test]
    fn render_short_terminal_keeps_user_prompt_visible_above_inline_diff() {
        let diff_lines = (1..=10)
            .map(|line| DiffPreviewLine {
                kind: "added".into(),
                content: format!("generated line {line}"),
                old_line: None,
                new_line: Some(line),
            })
            .collect::<Vec<_>>();
        let mut app = app_with_diff(DiffPreviewGetResult {
            status: "ready".into(),
            source: "pending_store".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Large patch".into()),
                files: vec![DiffPreviewFile {
                    path: "src/generated.rs".into(),
                    old_path: None,
                    status: "modified".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ -1 +10 @@".into(),
                        lines: diff_lines,
                    }],
                }],
            },
        });
        app.sessions[0].messages = vec![Message::user("fix visible prompt")];

        let buffer = rendered_buffer_with_size(&app, Palette::for_theme(ThemeName::Slate), 80, 24);
        let text = buffer
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("fix visible prompt"));
        assert!(text.contains("Diff Preview"));
        assert!(text.contains("6 more diff line(s) hidden"));
        assert!(text.contains("Composer"));
        assert!(text.contains("Idle"));
    }

    #[test]
    fn render_diff_preview_modal_keeps_unknown_future_labels_visible() {
        let app = app_with_diff(DiffPreviewGetResult {
            status: "requires_refresh".into(),
            source: "future_cache".into(),
            preview: DiffPreview {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
                title: Some("Future diff".into()),
                files: vec![DiffPreviewFile {
                    path: "src/lib.rs".into(),
                    old_path: Some("src/old.rs".into()),
                    status: "copied".into(),
                    hunks: vec![DiffPreviewHunk {
                        header: "@@ metadata @@".into(),
                        lines: vec![DiffPreviewLine {
                            kind: "metadata".into(),
                            content: "mode change".into(),
                            old_line: None,
                            new_line: None,
                        }],
                    }],
                }],
            },
        });

        let text = rendered_text(&app);

        assert!(text.contains("requires_refresh"));
        assert!(text.contains("future_cache"));
        assert!(text.contains("copied"));
        assert!(text.contains("src/old.rs -> src/lib.rs"));
        assert!(text.contains("mode change"));
    }

    // M15-E follow-up: sticky goal/loop indicator above the composer.
    // See the M9/M15 audit gap — `SessionAutonomyState` was populated
    // by notification mirrors but never surfaced unless the user typed
    // `/goal` or `/loop list`.

    fn autonomy_app_state() -> AppState {
        AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::system("ready")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        )
    }

    /// An autonomy `AppState` whose active session carries an active goal with
    /// `objective`; the fold preference stays at its default (`Auto`).
    fn autonomy_app_with_goal(objective: &str) -> AppState {
        let mut app = autonomy_app_state();
        app.set_session_goal(
            &SessionKey("local:test".into()),
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: objective.into(),
                status: "active".into(),
                token_budget: 2_000_000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        app
    }

    fn sample_agent(id: &str, status: &str) -> octos_core::ui_protocol::UiAgentRecord {
        octos_core::ui_protocol::UiAgentRecord {
            agent_id: id.into(),
            parent_agent_id: None,
            session_id: SessionKey("local:test".into()),
            task_id: None,
            path: "/root".into(),
            role: "worker".into(),
            nickname: id.into(),
            title: None,
            backend_kind: "native".into(),
            status: status.into(),
            last_task: None,
            summary: None,
            output_tail: None,
            cwd: None,
            profile_id: "coding".into(),
            runtime_policy_stamp: None,
            artifact_count: 0,
            artifacts: vec![],
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    /// #333 (Phase 1): the Tasks pane that `/ps` opens must surface the LIVE
    /// sub-agent roster, not only the old `session.tasks` cache. Renders
    /// `render_tasks` into a buffer and asserts a spawned sub-agent's nickname +
    /// running glyph appear even though `session.tasks` is empty.
    #[test]
    fn tasks_pane_renders_live_subagent_roster() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        let mut agent = sample_agent("security-review", "running");
        agent.nickname = "security-review".into();
        agent.last_task = Some("Review the octos codebase for SSRF".into());
        app.upsert_session_agent(&sid, agent);

        // Sanity: the task cache is empty, so pre-#333 this pane showed nothing.
        assert!(app.active_session().unwrap().tasks.is_empty());
        assert_eq!(app.active_session_agents().len(), 1);

        let palette = Palette::for_theme(ThemeName::Slate);
        let area = Rect::new(0, 0, 80, 12);
        let mut buffer = Buffer::empty(area);
        ratatui::widgets::Widget::render(render_tasks(&app, palette), area, &mut buffer);
        let text = rendered_rows(&buffer).join("\n");

        assert!(
            text.contains("security-review"),
            "sub-agent nickname must render in the /ps Tasks pane; got:\n{text}"
        );
        assert!(
            text.contains('⏵'),
            "running glyph must render for the live sub-agent; got:\n{text}"
        );
    }

    /// #334 (Phase 2): the sub-agent detail view (peek) surfaces the child's
    /// DELIVERABLES from the roster record's artifacts — the `*-review.md` /
    /// analysis files it wrote — so the detail view shows what the sub-agent
    /// produced, not just its streamed log.
    /// #338: a background spawn appears in BOTH the roster and `session.tasks`.
    /// The Tasks pane must show it ONCE (as a sub-agent), not twice — the task
    /// whose id matches a roster agent's `task_id` is suppressed from the legacy
    /// list, while an unrelated (non-agent) task still renders.
    #[test]
    fn tasks_pane_dedups_roster_tasks_from_legacy_list() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());

        // A spawn: appears as a roster agent AND a session task with the same id.
        let shared_task_id = octos_core::TaskId::new();
        let mut agent = sample_agent("alpha-review", "completed");
        agent.nickname = "alpha-review".into();
        agent.task_id = Some(shared_task_id.0.to_string());
        app.upsert_session_agent(&sid, agent);

        let session = app.active_session_mut().expect("session");
        session.tasks = vec![
            crate::model::TaskView {
                id: shared_task_id, // same spawn as the roster agent → dedup
                title: "alpha-review".into(),
                state: TaskRuntimeState::Completed,
                runtime_detail: None,
                output_tail: String::new(),
                turn_id: None,
            },
            crate::model::TaskView {
                id: octos_core::TaskId::new(), // a non-agent pipeline task → keep
                title: "deep-research-pipeline".into(),
                state: TaskRuntimeState::Running,
                runtime_detail: None,
                output_tail: String::new(),
                turn_id: None,
            },
        ];

        let palette = Palette::for_theme(ThemeName::Slate);
        let area = Rect::new(0, 0, 80, 20);
        let mut buffer = Buffer::empty(area);
        ratatui::widgets::Widget::render(render_tasks(&app, palette), area, &mut buffer);
        let text = rendered_rows(&buffer).join("\n");

        // The spawn appears ONCE — as a sub-agent, not repeated in the legacy list.
        assert_eq!(
            text.matches("alpha-review").count(),
            1,
            "roster spawn must render once, not duplicated; got:\n{text}"
        );
        // The non-agent pipeline task still renders below.
        assert!(
            text.contains("deep-research-pipeline"),
            "a non-agent task must still show; got:\n{text}"
        );
    }

    #[test]
    fn agent_peek_renders_deliverable_artifacts() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        let mut agent = sample_agent("security-review", "completed");
        agent.artifacts = vec![octos_core::ui_protocol::UiAgentArtifact {
            id: "art-1".into(),
            title: "analysis-octos-security.md".into(),
            kind: "file".into(),
            status: "ready".into(),
            path: Some("/tmp/analysis-octos-security.md".into()),
            content: None,
            extra: Default::default(),
        }];
        app.upsert_session_agent(&sid, agent);

        let palette = Palette::for_theme(ThemeName::Slate);
        let lines = agent_overlay_lines(&app, palette, "security-review");
        let text = lines_text(&lines);

        assert!(
            text.contains("analysis-octos-security.md"),
            "the peek must list the child's deliverable artifact; got:\n{text}"
        );
    }

    #[test]
    fn chat_view_selector_cycles_main_then_agents() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("ag-1", "running"));
        app.upsert_session_agent(&sid, sample_agent("ag-2", "running"));

        assert_eq!(app.chat_view, ChatViewTarget::Main, "defaults to Main");

        // next: Main -> ag-1 -> ag-2 -> Main (wrap)
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Agent("ag-1".into()));
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Agent("ag-2".into()));
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Main);

        // prev from Main wraps back to the last agent
        app.select_prev_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Agent("ag-2".into()));
    }

    #[test]
    fn chat_view_selector_is_noop_without_agents() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Main);
        app.select_prev_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Main);
    }

    #[test]
    fn chat_view_normalizes_to_main_when_selected_agent_disappears() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("ag-1", "running"));
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Agent("ag-1".into()));

        // The agent completes and is pruned from the session.
        app.session_autonomy_mut(&sid).agents.clear();
        app.normalize_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Main);
    }

    #[test]
    fn chat_view_resets_to_main_on_session_switch() {
        use crate::model::ChatViewTarget;
        let mut app = AppState::new(
            vec![
                SessionView {
                    id: SessionKey("local:a".into()),
                    title: "a".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![],
                    tasks: vec![],
                    live_reply: None,
                },
                SessionView {
                    id: SessionKey("local:b".into()),
                    title: "b".into(),
                    profile_id: Some("coding".into()),
                    messages: vec![],
                    tasks: vec![],
                    live_reply: None,
                },
            ],
            0,
            "ready".into(),
            None,
            false,
        );
        app.upsert_session_agent(
            &SessionKey("local:a".into()),
            sample_agent("ag-1", "running"),
        );
        app.select_next_chat_view();
        assert_eq!(app.chat_view, ChatViewTarget::Agent("ag-1".into()));

        app.switch_selected_session(1);
        assert_eq!(
            app.chat_view,
            ChatViewTarget::Main,
            "agent selection must not carry across sessions"
        );
    }

    #[test]
    fn agent_view_overlay_renders_selected_agent_output() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("researcher", "running"));
        app.set_agent_output(
            &sid,
            "researcher",
            "Investigating the corpus\nFound 12 candidate sources".into(),
            octos_core::ui_protocol::OutputCursor { offset: 0 },
        );

        // Main view: the inline chat is shown, NOT the agent's output.
        assert!(!agent_view_active(&app));
        let main_text = rendered_text(&app);
        assert!(!main_text.contains("candidate sources"));

        // Peeking the agent: the overlay takes over and shows its output + hint.
        app.set_chat_view(ChatViewTarget::Agent("researcher".into()));
        assert!(agent_view_active(&app));
        assert!(wants_fullscreen_overlay(&app));
        let peek_text = rendered_text(&app);
        assert!(
            peek_text.contains("Investigating the corpus"),
            "overlay shows the agent's streamed output"
        );
        assert!(peek_text.contains("candidate sources"));
        assert!(
            peek_text.contains("Esc back to chat"),
            "overlay shows the navigation hint"
        );
    }

    #[test]
    fn agent_view_inactive_when_selected_agent_absent() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        // A selection pointing at a non-existent agent must not trigger the
        // full-screen takeover (the switcher normalizes such stragglers to Main).
        app.set_chat_view(ChatViewTarget::Agent("ghost".into()));
        assert!(!agent_view_active(&app));
        assert!(!wants_fullscreen_overlay(&app));
    }

    #[test]
    fn agent_view_yields_to_modal() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        app.set_chat_view(ChatViewTarget::Agent("worker".into()));
        assert!(agent_view_active(&app), "peek active with no modal");

        // A modal must take the screen and keyboard back from the peek, else it
        // renders behind the opaque overlay while still consuming keys.
        app.task_output.active = true;
        assert!(!agent_view_active(&app), "peek yields while a modal is up");

        app.task_output.active = false;
        assert!(
            agent_view_active(&app),
            "peek resumes after the modal closes"
        );
    }

    #[test]
    fn agent_roster_refresh_drops_peek_of_vanished_agent() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        app.set_chat_view(ChatViewTarget::Agent("worker".into()));
        assert!(agent_view_active(&app));

        // A refresh that no longer lists "worker" (completed and pruned by the
        // backend) must fall the peek back to the main chat.
        app.set_session_agents(&sid, vec![]);
        assert_eq!(app.chat_view, ChatViewTarget::Main);
        assert!(!agent_view_active(&app));
    }

    #[test]
    fn explicit_output_read_overwrites_the_cache_then_deltas_resume() {
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        // `set_agent_output` backs only the explicit `/agents output <id>`
        // command (peeking no longer auto-reads), so a user-requested snapshot
        // is authoritative: it replaces whatever the cache held and re-seeds the
        // cursor. Live deltas then resume appending from that cursor.
        app.append_agent_output(
            &sid,
            "worker",
            octos_core::ui_protocol::OutputCursor { offset: 10 },
            "partial streamed output\n",
        );
        app.set_agent_output(
            &sid,
            "worker",
            "full snapshot up to here\n".into(),
            octos_core::ui_protocol::OutputCursor { offset: 24 },
        );
        assert_eq!(
            app.active_agent_output("worker"),
            Some("full snapshot up to here\n"),
            "an explicit read replaces the cache with the fetched snapshot"
        );

        // A delta past the snapshot's cursor appends cleanly (shared offset space).
        app.append_agent_output(
            &sid,
            "worker",
            octos_core::ui_protocol::OutputCursor { offset: 33 },
            "next chunk\n",
        );
        assert_eq!(
            app.active_agent_output("worker"),
            Some("full snapshot up to here\nnext chunk\n")
        );

        // Into an empty cache the read simply fills it (e.g. a completed agent).
        app.set_agent_output(
            &sid,
            "idle",
            "final output of a completed agent".into(),
            octos_core::ui_protocol::OutputCursor { offset: 5 },
        );
        assert_eq!(
            app.active_agent_output("idle"),
            Some("final output of a completed agent")
        );
    }

    #[test]
    fn live_ui_height_reserves_agent_strip_row() {
        let mut app = autonomy_app_state();
        let without = live_ui_height(&app, 80, 40);
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        let with = live_ui_height(&app, 80, 40);
        assert_eq!(
            with,
            without + 2,
            "the vertical strip reserves a title row plus one row per agent"
        );
    }

    /// #407 review P1 (Blocker 2): the live-viewport reservation must include
    /// the peer dock rows, or the inline layout over-subscribes and Ratatui
    /// compresses a fixed row (clipped composer / scrollback ghosts).
    #[test]
    fn live_ui_height_reserves_peer_strip_rows() {
        let mut app = autonomy_app_state();
        let without = live_ui_height(&app, 80, 40);
        app.peer_session_meta.insert(
            SessionKey("local:tui#peer-ci-red".into()),
            crate::model::PeerMeta {
                slug: "ci-red".into(),
                brief_path: "/tmp/brief.md".into(),
                agent_staged: false,
                model_id: None,
                created: std::time::Instant::now(),
            },
        );
        let expected = peer_strip_height(&app, 40);
        assert!(expected > 0, "an open peer occupies dock rows");
        assert_eq!(
            live_ui_height(&app, 80, 40),
            without + expected,
            "the reservation basis must grow by exactly the dock's rendered rows"
        );
    }

    #[test]
    fn agent_strip_height_reflects_agent_presence() {
        let mut app = autonomy_app_state();
        assert_eq!(agent_strip_height(&app, 40), 0, "hidden with no agents");
        app.upsert_session_agent(
            &SessionKey("local:test".into()),
            sample_agent("ag-1", "running"),
        );
        assert_eq!(
            agent_strip_height(&app, 40),
            2,
            "title row + one agent row once agents exist on a normal terminal"
        );
    }

    #[test]
    fn completed_subagent_leaves_the_strip_immediately_without_a_tab_cycle() {
        // A Running sub-agent occupies the strip: title row + its one row.
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        assert_eq!(
            agent_strip_height(&app, 40),
            2,
            "running sub-agent shows in the strip"
        );
        let rendered = line_texts(&agent_strip_lines(&app, Palette::for_theme(app.theme), 1));
        assert!(
            rendered.iter().any(|line| line.contains("worker")),
            "the running agent's row is present"
        );

        // Its terminal `agent/updated` lands. With NO Tab-cycle and NO submit,
        // the strip drops it on the very next render — the whole strip collapses
        // because that was the only agent.
        app.upsert_session_agent(&sid, sample_agent("worker", "completed"));
        assert_eq!(
            agent_strip_height(&app, 40),
            0,
            "a completed sub-agent leaves the strip immediately"
        );

        // The roster itself keeps the record so `/ps`, the peek, and the
        // scrollback card still show the completed agent.
        assert!(
            app.active_session_agents()
                .iter()
                .any(|agent| agent.agent_id == "worker" && agent.status == "completed"),
            "the completed agent stays in the roster for /ps and the peek"
        );
    }

    #[test]
    fn strip_keeps_running_agents_when_a_sibling_completes() {
        // Two running siblings, then one completes: the strip keeps the still-
        // running one and only the finished sibling's row disappears.
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("alpha", "running"));
        app.upsert_session_agent(&sid, sample_agent("beta", "running"));
        assert_eq!(
            agent_strip_height(&app, 40),
            3,
            "title row + two agent rows"
        );

        app.upsert_session_agent(&sid, sample_agent("beta", "failed"));
        assert_eq!(
            agent_strip_height(&app, 40),
            2,
            "title row + the one still-running agent"
        );
        let rendered = line_texts(&agent_strip_lines(&app, Palette::for_theme(app.theme), 1));
        assert!(
            rendered.iter().any(|line| line.contains("alpha")),
            "the running sibling stays"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("beta")),
            "the terminal sibling's row is gone"
        );
    }

    #[test]
    fn peek_output_falls_back_to_the_record_tail_when_cache_is_empty() {
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        let mut rec = sample_agent("worker", "done");
        rec.output_tail = Some("tail snapshot from agent/list\n".into());
        app.upsert_session_agent(&sid, rec);

        // No live delta yet (e.g. a completed agent, or just after a reconnect):
        // the peek shows the record's `output_tail` instead of the placeholder.
        assert_eq!(
            app.active_agent_output_or_tail("worker"),
            Some("tail snapshot from agent/list\n")
        );

        // A live delta is strictly fresher and supersedes the snapshot.
        app.append_agent_output(
            &sid,
            "worker",
            octos_core::ui_protocol::OutputCursor { offset: 5 },
            "live delta wins\n",
        );
        assert_eq!(
            app.active_agent_output_or_tail("worker"),
            Some("live delta wins\n")
        );
    }

    #[test]
    fn home_clamps_to_measured_max_so_down_is_not_stuck() {
        let mut app = autonomy_app_state();
        // The overlay renderer measured a 20-row maximum on the last frame.
        app.record_agent_view_scroll_max(20);

        app.scroll_agent_view_up(usize::MAX); // Home
        assert_eq!(app.agent_view_scroll, 20, "Home lands exactly at the top");

        // Down moves the view immediately — there is no huge sentinel to unwind.
        app.scroll_agent_view_down(1);
        assert_eq!(app.agent_view_scroll, 19);

        // Selecting a new target relaxes the clamp to "unmeasured" so the next
        // frame's real bound applies rather than the previous agent's.
        app.set_chat_view(crate::model::ChatViewTarget::Agent("worker".into()));
        assert_eq!(app.agent_view_scroll_max.get(), usize::MAX);
    }

    #[test]
    fn down_recovers_when_home_ran_before_the_bound_was_measured() {
        // codex round-6 case: Tab then Home batched before the peek's first draw,
        // so the bound is still `usize::MAX` when Home stores its jump.
        let mut app = autonomy_app_state();
        app.scroll_agent_view_up(usize::MAX); // Home, unmeasured
        assert_eq!(app.agent_view_scroll, usize::MAX);

        // The first draw measures the real maximum...
        app.record_agent_view_scroll_max(12);
        // ...and Down snaps the stale over-shoot down before moving — not stuck
        // decrementing the sentinel.
        app.scroll_agent_view_down(1);
        assert_eq!(app.agent_view_scroll, 11);
    }

    #[test]
    fn wants_mouse_capture_stays_on_for_a_detail_modal_over_a_peek() {
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        app.set_chat_view(crate::model::ChatViewTarget::Agent("worker".into()));
        assert!(wants_mouse_capture(&app), "the peek captures the wheel");

        // A detail modal opens over the peek: it now owns the screen (the peek
        // yields), but it is still a full-screen wheel target, so capture must
        // stay on or the wheel would be silently dead over it.
        app.task_output.active = true;
        assert!(!agent_view_active(&app), "the peek yields to the modal");
        assert!(
            wants_mouse_capture(&app),
            "capture stays on so the wheel scrolls the detail modal"
        );
    }

    #[test]
    fn agent_strip_hidden_on_a_constrained_terminal() {
        let mut app = autonomy_app_state();
        app.upsert_session_agent(
            &SessionKey("local:test".into()),
            sample_agent("ag-1", "running"),
        );
        // Below the floor the strip would force Ratatui to collapse a fixed row,
        // so it is dropped (Tab still switches views without it).
        assert_eq!(
            agent_strip_height(&app, AGENT_STRIP_MIN_TERMINAL_ROWS - 1),
            0,
            "dropped when the terminal can't afford the row"
        );
        assert_eq!(
            agent_strip_height(&app, AGENT_STRIP_MIN_TERMINAL_ROWS),
            1,
            "restored at the floor"
        );
    }

    #[test]
    fn agent_strip_hidden_while_transcript_pager_open() {
        let mut app = autonomy_app_state();
        let sid = app.active_session().unwrap().id.clone();
        app.upsert_session_agent(&sid, sample_agent("worker", "running"));
        assert_eq!(agent_strip_height(&app, 40), 2, "shown in the inline flow");

        // In the pager the strip's Tab control is disabled and its extra rows
        // overcommit the `Min(8)` transcript layout, so it is dropped.
        app.transcript_pager_active = true;
        assert_eq!(agent_strip_height(&app, 40), 0, "hidden under the pager");
    }

    #[test]
    fn agent_status_glyph_maps_states() {
        assert_eq!(agent_status_glyph("running"), "⏵");
        assert_eq!(agent_status_glyph("completed"), "✔");
        assert_eq!(agent_status_glyph("failed"), "✖");
        assert_eq!(agent_status_glyph("cancelled"), "⊘");
        assert_eq!(agent_status_glyph("mystery"), "•");
    }

    #[test]
    fn agent_strip_height_scales_vertically_with_agents() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("edison", "running"));
        app.upsert_session_agent(&sid, sample_agent("thomas", "running"));
        assert_eq!(
            agent_strip_height(&app, 40),
            3,
            "title row + one row per agent"
        );

        for idx in 0..4 {
            app.upsert_session_agent(&sid, sample_agent(&format!("extra-{idx}"), "running"));
        }
        assert_eq!(
            agent_strip_height(&app, 40),
            1 + AGENT_STRIP_MAX_AGENT_ROWS,
            "agent rows are capped"
        );

        assert_eq!(
            agent_strip_height(&app, AGENT_STRIP_MIN_TERMINAL_ROWS),
            1,
            "at the minimum height the strip degrades to the title row alone"
        );
        assert_eq!(
            agent_strip_height(&app, AGENT_STRIP_MIN_TERMINAL_ROWS - 1),
            0,
            "below the minimum the strip stays hidden"
        );

        app.transcript_pager_active = true;
        assert_eq!(agent_strip_height(&app, 40), 0, "hidden in the pager");
    }

    #[test]
    fn agent_strip_window_keeps_selection_visible() {
        use crate::model::ChatViewTarget;
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        for idx in 0..6 {
            app.upsert_session_agent(&sid, sample_agent(&format!("ag-{idx}"), "running"));
        }

        let (window, hidden) = agent_strip_window(&app, 4);
        assert_eq!(window, 0..4, "main view shows the top of the roster");
        assert_eq!(hidden, 2);

        app.chat_view = ChatViewTarget::Agent("ag-5".into());
        let (window, hidden) = agent_strip_window(&app, 4);
        assert_eq!(window, 2..6, "window shifts to keep the selection visible");
        assert_eq!(hidden, 2);
    }

    #[test]
    fn agent_strip_lines_render_vertical_rows_with_detail() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        let mut edison = sample_agent("edison", "running");
        edison.nickname = "Edison".into();
        edison.last_task = Some("clone the repo\nsecond line ignored".into());
        app.upsert_session_agent(&sid, edison);
        // Both agents are live: a terminal agent would leave the strip at once,
        // so the multi-row / overflow rendering is exercised with running ones.
        app.upsert_session_agent(&sid, sample_agent("thomas", "running"));

        let lines = agent_strip_lines(&app, Palette::for_theme(ThemeName::Codex), 2);
        let flat: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert_eq!(flat.len(), 3, "title row + one row per agent");
        assert!(flat[0].contains("main"), "title row carries the main chip");
        assert!(
            flat[1].contains("Edison") && flat[1].contains("running"),
            "agent row shows name and raw status: {}",
            flat[1]
        );
        assert!(
            flat[1].contains("clone the repo") && !flat[1].contains("second line"),
            "detail is the first non-empty line of the last task: {}",
            flat[1]
        );
        assert!(
            flat[2].contains("thomas") && flat[2].contains("running"),
            "second agent row present: {}",
            flat[2]
        );

        // Overflow: six agents, four visible -> the title row carries +2.
        for idx in 0..4 {
            app.upsert_session_agent(&sid, sample_agent(&format!("extra-{idx}"), "running"));
        }
        let lines = agent_strip_lines(&app, Palette::for_theme(ThemeName::Codex), 4);
        let title: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            title.contains("+2") || title.contains("2 个"),
            "overflow marker names the hidden count: {title}"
        );
        assert_eq!(lines.len(), 5, "title + capped agent rows");
    }

    /// Agent Dock (#323): collapsed mode renders exactly one summary pill
    /// line with total/running counts, the unread segment only when
    /// something finished unseen, and reserves a single row of height.
    #[test]
    fn agent_dock_collapsed_renders_single_pill_line() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("edison", "running"));
        app.upsert_session_agent(&sid, sample_agent("thomas", "completed"));
        app.agent_dock_collapsed = true;

        assert_eq!(
            agent_strip_height(&app, 40),
            1,
            "collapsed dock reserves exactly the pill row"
        );
        let lines = agent_strip_lines(&app, Palette::for_theme(ThemeName::Codex), 0);
        assert_eq!(lines.len(), 1, "collapsed dock is a single line");
        let pill: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            pill.contains('2') && pill.contains('1'),
            "pill carries total=2 and running=1: {pill}"
        );
        // thomas transitioned to terminal while viewing Main -> unread.
        assert!(pill.contains("1●"), "pill shows the unread count: {pill}");
        assert!(
            pill.contains("Ctrl+G/Alt+G"),
            "pill hints the toggle key: {pill}"
        );

        // Peeking thomas clears the unread segment.
        app.set_chat_view(crate::model::ChatViewTarget::Agent("thomas".into()));
        app.set_chat_view(crate::model::ChatViewTarget::Main);
        let lines = agent_strip_lines(&app, Palette::for_theme(ThemeName::Codex), 0);
        let pill: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(!pill.contains('●'), "seen -> no unread segment: {pill}");
    }

    /// Expanded rows depth-indent LIVE children under their parent (#323). A
    /// finished agent has already left the strip, so the per-row unread dot no
    /// longer applies — the unread outcome is summarized on the title row while
    /// the completed agent's row is gone.
    #[test]
    fn agent_strip_rows_indent_live_children_and_title_summarizes_unread() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:test".into());
        app.upsert_session_agent(&sid, sample_agent("lead", "running"));
        let mut child = sample_agent("worker", "running");
        child.parent_agent_id = Some("lead".into());
        app.upsert_session_agent(&sid, child);
        // A sibling finishes while the user is on Main -> unread, and leaves the
        // strip immediately.
        app.upsert_session_agent(&sid, sample_agent("scout", "completed"));

        let lines = agent_strip_lines(&app, Palette::for_theme(ThemeName::Codex), 3);
        let rows: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        // The title row still summarizes the one unread completion...
        assert!(
            rows[0].contains("1●"),
            "title row unread count: {}",
            rows[0]
        );
        // ...but the completed agent itself is no longer a row.
        assert!(
            !rows.iter().any(|row| row.contains("scout")),
            "the finished agent left the strip: {rows:?}"
        );
        assert_eq!(rows.len(), 3, "title + the two live agent rows: {rows:?}");
        let parent_indent = rows[1].chars().take_while(|c| *c == ' ').count();
        let child_indent = rows[2].chars().take_while(|c| *c == ' ').count();
        assert!(
            child_indent > parent_indent,
            "live child indents deeper than parent: {parent_indent} vs {child_indent}"
        );
    }

    #[test]
    fn agent_depth_walks_parents_and_survives_cycles() {
        let mut a = sample_agent("a", "running");
        let mut b = sample_agent("b", "running");
        let mut c = sample_agent("c", "running");
        b.parent_agent_id = Some("a".into());
        c.parent_agent_id = Some("b".into());
        // a points at c: a cycle — must terminate at the cap, not hang.
        a.parent_agent_id = Some("c".into());
        let agents = vec![a, b, c];
        assert!(agent_depth(&agents, "c") <= 4, "cycle bounded");

        let mut root = sample_agent("root", "running");
        root.parent_agent_id = None;
        let mut kid = sample_agent("kid", "running");
        kid.parent_agent_id = Some("root".into());
        let mut stranger = sample_agent("stranger", "running");
        stranger.parent_agent_id = Some("not-in-roster".into());
        let agents = vec![root, kid, stranger];
        assert_eq!(agent_depth(&agents, "root"), 0);
        assert_eq!(agent_depth(&agents, "kid"), 1);
        assert_eq!(
            agent_depth(&agents, "stranger"),
            0,
            "unknown parent renders flat"
        );
    }

    #[test]
    fn format_short_duration_tiers() {
        assert_eq!(format_short_duration(0), "0s");
        assert_eq!(format_short_duration(41_000), "41s");
        assert_eq!(format_short_duration(134_000), "2m14s");
        assert_eq!(format_short_duration(3_720_000), "1h02m");
        assert_eq!(
            format_short_duration(-5_000),
            "0s",
            "clock skew floors at 0"
        );
    }

    fn sample_loop(
        loop_id: &str,
        prompt: &str,
        mode: &str,
        secs: Option<u64>,
    ) -> octos_core::ui_protocol::UiLoopRecord {
        octos_core::ui_protocol::UiLoopRecord {
            loop_id: loop_id.into(),
            session_id: SessionKey("local:test".into()),
            profile_id: None,
            prompt: prompt.into(),
            mode: mode.into(),
            interval_seconds: secs,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn render_autonomy_indicator_idle_reserves_no_rows() {
        let app = autonomy_app_state();
        assert_eq!(autonomy_indicator_height(&app, 100), 0);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert!(
            lines.is_empty(),
            "idle state should produce no indicator rows"
        );

        let text = rendered_text(&app);
        assert!(
            !text.contains("Goal:"),
            "idle render must not surface a goal label",
        );
        assert!(
            !text.contains("Loops:"),
            "idle render must not surface a loop label",
        );
    }

    #[test]
    fn plan_indicator_renders_checklist_tree_with_glyphs() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_plan(
            &session_id,
            Some(UiPlanRecord {
                title: Some("Building memory panel".into()),
                updated_at_ms: 0,
                items: vec![
                    UiPlanItem {
                        id: "1".into(),
                        title: "PWA manifest".into(),
                        status: PlanItemStatus::Completed,
                        priority: None,
                    },
                    UiPlanItem {
                        id: "2".into(),
                        title: "memory panel".into(),
                        status: PlanItemStatus::InProgress,
                        priority: Some("P3".into()),
                    },
                    UiPlanItem {
                        id: "3".into(),
                        title: "cron toggle".into(),
                        status: PlanItemStatus::Pending,
                        priority: None,
                    },
                ],
            }),
            None,
        );

        // header + 3 item rows, no goal/loops.
        assert_eq!(autonomy_indicator_height(&app, 100), 4);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert_eq!(lines.len(), 4);

        let text = rendered_text(&app);
        assert!(
            text.contains("Building memory panel"),
            "header activity title"
        );
        assert!(text.contains("(1/3)"), "done/total counter");
        assert!(text.contains('⎿'), "tree anchor glyph");
        assert!(text.contains('✔'), "completed glyph");
        assert!(text.contains('◼'), "pending glyph");
        assert!(text.contains("PWA manifest"));
        assert!(text.contains("P3"), "priority chip on the in-progress item");
    }

    #[test]
    fn plan_cleared_only_when_its_authoring_turn_completes() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        let turn = TurnId::new();
        let other_turn = TurnId::new();
        let plan = Some(UiPlanRecord {
            title: Some("plan".into()),
            updated_at_ms: 0,
            items: vec![UiPlanItem {
                id: "1".into(),
                title: "do it".into(),
                status: PlanItemStatus::InProgress,
                priority: None,
            }],
        });
        app.set_session_plan(&session_id, plan, Some(turn.clone()));
        assert_eq!(autonomy_indicator_height(&app, 100), 2);

        // A completion for a DIFFERENT turn must not clear the panel.
        app.clear_session_plan_for_turn(&session_id, &other_turn);
        assert_eq!(autonomy_indicator_height(&app, 100), 2);

        // The authoring turn's completion clears it.
        app.clear_session_plan_for_turn(&session_id, &turn);
        assert_eq!(autonomy_indicator_height(&app, 100), 0);
    }

    #[test]
    fn plan_indicator_truncates_long_checklist() {
        use octos_core::ui_protocol::{PlanItemStatus, UiPlanItem, UiPlanRecord};
        let mut app = autonomy_app_state();
        let items: Vec<_> = (0..12)
            .map(|i| UiPlanItem {
                id: i.to_string(),
                title: format!("item {i}"),
                status: PlanItemStatus::Pending,
                priority: None,
            })
            .collect();
        app.set_session_plan(
            &SessionKey("local:test".into()),
            Some(UiPlanRecord {
                title: Some("big plan".into()),
                updated_at_ms: 0,
                items,
            }),
            None,
        );
        // header + 8 shown + 1 overflow line.
        assert_eq!(autonomy_indicator_height(&app, 100), 10);
        assert!(rendered_text(&app).contains("+4 more"));
    }

    #[test]
    fn format_tokens_k_rounds_to_nearest_thousand() {
        assert_eq!(format_tokens_k(0), "0K");
        assert_eq!(format_tokens_k(499), "0K");
        assert_eq!(format_tokens_k(500), "1K");
        assert_eq!(format_tokens_k(12_000), "12K");
        assert_eq!(format_tokens_k(174_763), "175K");
        assert_eq!(format_tokens_k(2_000_000), "2000K");
        // No overflow / correct rounding at the u64 ceiling.
        assert_eq!(format_tokens_k(u64::MAX), "18446744073709552K");
    }

    #[test]
    fn format_tokens_human_switches_to_millions_above_1m() {
        // Below 1M it delegates to the K formatter, so a 128k/256k window in
        // the `/context` subtitle reads the same way as the goal chip.
        assert_eq!(format_tokens_human(0), "0K");
        assert_eq!(format_tokens_human(45_231), "45K");
        assert_eq!(format_tokens_human(128_000), "128K");
        assert_eq!(format_tokens_human(256_000), "256K");
        // The switch is on the raw value, not the rounded-K value, so a hair
        // under 1M still renders in K (rounding up to `1000K`).
        assert_eq!(format_tokens_human(999_999), "1000K");
        // At/above 1M it switches to millions and drops a trailing `.0` so a
        // 1,000,000-token window reads `1M`, not `1000K` or `1.0M`.
        assert_eq!(format_tokens_human(1_000_000), "1M");
        assert_eq!(format_tokens_human(1_500_000), "1.5M");
        assert_eq!(format_tokens_human(2_000_000), "2M");
    }

    #[test]
    fn context_window_usage_pairs_estimate_with_real_or_default_window() {
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();

        // No token estimate for the session yet → nothing to render.
        assert_eq!(context_window_usage(&app, &session_id), None);

        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 64_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        // No per-model window on the wire yet → pair the estimate with the
        // fixed default so the subtitle still shows an honest fraction.
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((64_000, DEFAULT_CONTEXT_WINDOW_TOKENS as u64)),
        );

        // Once the real window arrives it wins over the default.
        app.session_context_window
            .insert(session_id.clone(), 1_000_000);
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((64_000, 1_000_000)),
        );

        // A zero window is treated as unknown and falls back to the default.
        app.session_context_window.insert(session_id.clone(), 0);
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((64_000, DEFAULT_CONTEXT_WINDOW_TOKENS as u64)),
        );
    }

    /// The pre-first-turn fallback window is model-aware: a MiniMax-M3 session
    /// shows its real ~1M window, not the generic 128K default, before any
    /// `token_cost` update arrives. An unknown model keeps the 128K default.
    #[test]
    fn context_window_fallback_is_model_aware_before_first_cost_update() {
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 1,
            token_estimate: 1_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        // No model resolved yet → conservative 128K default.
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((1_000, DEFAULT_CONTEXT_WINDOW_TOKENS as u64)),
        );

        // MiniMax-M3 → real 1M window, not the 128K placeholder.
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id.clone(),
            "MiniMax-M3",
            "/tmp/work",
        ));
        let (_, window) = context_window_usage(&app, &session_id).expect("usage");
        assert!(
            window >= 1_000_000,
            "MiniMax-M3 fallback window must be ~1M, got {window}"
        );

        // The Kimi coding plan's bare `k3` id → 1M, like `kimi-k3`.
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id.clone(),
            "k3",
            "/tmp/work",
        ));
        let (_, k3_window) = context_window_usage(&app, &session_id).expect("usage");
        assert!(
            k3_window >= 1_000_000,
            "coding-plan k3 fallback window must be ~1M, got {k3_window}"
        );

        // An unknown model keeps the conservative default.
        app.set_runtime_status(runtime_status_with_model_cwd(
            session_id.clone(),
            "some-tiny-model",
            "/tmp/work",
        ));
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((1_000, DEFAULT_CONTEXT_WINDOW_TOKENS as u64)),
        );

        // The real per-model window on the wire still wins over the hint.
        app.session_context_window
            .insert(session_id.clone(), 500_000);
        assert_eq!(
            context_window_usage(&app, &session_id),
            Some((1_000, 500_000)),
        );
    }

    #[test]
    fn render_autonomy_indicator_goal_only_renders_one_row() {
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "finish the OAuth refactor".into(),
                status: "active".into(),
                token_budget: 50_000,
                tokens_used: 12_000,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );

        assert_eq!(autonomy_indicator_height(&app, 100), 1);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert_eq!(lines.len(), 1);

        let text = rendered_text(&app);
        assert!(
            text.contains("Goal:"),
            "goal row must surface 'Goal:' label"
        );
        assert!(text.contains("finish the OAuth refactor"));
        assert!(text.contains("active"));
        assert!(
            text.contains("12K/50K"),
            "goal tokens render in K units, got: {text}"
        );
        assert!(!text.contains("Loops:"), "loops row must be hidden");
    }

    #[test]
    fn render_autonomy_indicator_wraps_a_long_objective_across_rows() {
        // mini5: a long `/goal` objective was truncated to one clipped line.
        // When UNFOLDED it must wrap across multiple rows (bounded), and the
        // reserved height MUST equal the rendered row count (else the banner
        // clips or strands a blank band). (A long objective now folds BY
        // DEFAULT — see `goal_objective_folds_a_long_objective_by_default` — so
        // this test drives the explicit unfolded state Ctrl+P produces.)
        let mut app = autonomy_app_state();
        app.goal_objective_fold = GoalObjectiveFold::Unfolded;
        let session_id = SessionKey("local:test".into());
        let long_objective = "build a react website about the 2026 world cup finals with all 48 \
             teams, players, coaches, photos, group stage and knockout brackets, per-match \
             details, and a UX score of at least nine out of ten"
            .to_string();
        app.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: long_objective.clone(),
                status: "active".into(),
                token_budget: 2_000_000,
                tokens_used: 0,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );

        let height = autonomy_indicator_height(&app, 100);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert!(
            height > 1,
            "a long objective must reserve more than one row"
        );
        assert!(
            height as usize <= GOAL_OBJECTIVE_MAX_ROWS,
            "objective rows are bounded by the cap"
        );
        assert_eq!(
            height as usize,
            lines.len(),
            "reserved height must match rendered rows exactly"
        );
        // Far more of the objective is visible than a single 56-char line.
        let text = rendered_text(&app);
        assert!(text.contains("build a react website"));
        assert!(
            text.contains("knockout") || text.contains("group stage"),
            "later objective content must be visible via wrapping"
        );
    }

    #[test]
    fn goal_objective_folds_a_long_objective_by_default() {
        // The user's complaint: a huge pasted objective (shader code) dominated
        // the banner. With the default `Auto` fold it collapses to ONE compact
        // preview row — glyph + prefix + truncated preview + `…` + parenthetical
        // + a Ctrl+P hint — and the reserved height matches (reserve==render).
        let long = "build a react website about the 2026 world cup finals ".repeat(20);
        let app = autonomy_app_with_goal(&long);
        assert_eq!(
            app.goal_objective_fold,
            GoalObjectiveFold::Auto,
            "no explicit toggle yet — the default derives fold from length",
        );

        let height = autonomy_indicator_height(&app, 100);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert_eq!(height, 1, "a long objective folds to one row by default");
        assert_eq!(lines.len(), 1, "reserve==render in the folded state");
        assert!(
            app.goal_objective_folded_effective.get(),
            "the resolver records the effective fold for Ctrl+P",
        );

        let text = rendered_text(&app);
        assert!(text.contains("Goal:"), "folded row keeps the goal label");
        assert!(text.contains('…'), "folded row shows a truncation ellipsis");
        assert!(
            text.contains("2000K"),
            "status/budget parenthetical stays on-screen"
        );
        assert!(text.contains("Ctrl+P"), "folded row hints Ctrl+P expands");
    }

    #[test]
    fn goal_objective_shows_a_short_goal_in_full_by_default() {
        // A short goal (≤ the auto threshold of rows) must NEVER look truncated
        // by default — `Auto` shows it in full, with no ellipsis or expand hint.
        // A ~200-char objective wraps to 3 rows at width 100 — right at the
        // boundary that stays unfolded.
        let app = autonomy_app_with_goal(&"x".repeat(200));
        let height = autonomy_indicator_height(&app, 100);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert_eq!(
            height, 3,
            "a 3-row goal shows in full (not folded) by default"
        );
        assert_eq!(
            height as usize,
            lines.len(),
            "reserve==render when unfolded"
        );
        assert!(
            !app.goal_objective_folded_effective.get(),
            "a short goal is not folded by default",
        );
        let text = rendered_text(&app);
        assert!(
            !text.contains("Ctrl+P"),
            "an unfolded goal shows no expand hint",
        );
    }

    #[test]
    fn goal_fold_reserve_matches_render_in_both_states() {
        // The reserve==render discipline must hold whether the objective is
        // folded (default long) or explicitly unfolded — a mismatch clips the
        // banner or strands a blank band above the composer.
        let long = "explore the moduli space of stable maps and render every chart ".repeat(12);
        let mut app = autonomy_app_with_goal(&long);

        // Folded (Auto default for a long objective): exactly one row.
        assert_eq!(app.goal_objective_fold, GoalObjectiveFold::Auto);
        let folded_h = autonomy_indicator_height(&app, 90);
        let folded_lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 90);
        assert_eq!(folded_h, 1);
        assert_eq!(
            folded_h as usize,
            folded_lines.len(),
            "folded reserve==render"
        );

        // Unfolded: many rows, still exactly reserved.
        app.goal_objective_fold = GoalObjectiveFold::Unfolded;
        let open_h = autonomy_indicator_height(&app, 90);
        let open_lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 90);
        assert!(open_h > 1, "unfolded long objective spans many rows");
        assert_eq!(
            open_h as usize,
            open_lines.len(),
            "unfolded reserve==render"
        );
    }

    #[test]
    fn toggling_goal_fold_flips_between_compact_and_full() {
        // Ctrl+P (via `toggle_goal_objective_fold`) flips whatever is on screen:
        // a folded long goal expands to many rows; toggling again re-folds it.
        let long = "port the physically based renderer to wgpu with IBL and bloom ".repeat(12);
        let mut app = autonomy_app_with_goal(&long);

        // Render once so the effective fold (folded, via Auto) is recorded.
        assert_eq!(autonomy_indicator_height(&app, 100), 1);

        // First toggle → explicit Unfolded → many rows.
        app.toggle_goal_objective_fold();
        assert_eq!(app.goal_objective_fold, GoalObjectiveFold::Unfolded);
        let open_h = autonomy_indicator_height(&app, 100);
        assert!(open_h > 1, "Ctrl+P expands the folded goal");

        // Second toggle → explicit Folded → back to one row.
        app.toggle_goal_objective_fold();
        assert_eq!(app.goal_objective_fold, GoalObjectiveFold::Folded);
        assert_eq!(
            autonomy_indicator_height(&app, 100),
            1,
            "Ctrl+P re-folds it"
        );
    }

    #[test]
    fn goal_objective_uses_full_width_not_a_fixed_column() {
        // Regression (mini5): the objective wrapped at a fixed ~half-screen
        // column (56) regardless of terminal width. It must now wrap to the FULL
        // render width — a wider terminal fits more per row and reserves FEWER
        // rows, and rows exceed the old 56-column cap.
        let objective = "x ".repeat(120); // ~240 chars
        let narrow = goal_objective_chunks(&objective, 60, 0);
        let wide = goal_objective_chunks(&objective, 160, 0);
        assert!(
            wide.len() < narrow.len(),
            "a wider terminal must wrap into fewer rows (full-width): wide={} narrow={}",
            wide.len(),
            narrow.len(),
        );
        assert!(
            wide.first().is_some_and(|r| r.chars().count() > 56),
            "wide rows must exceed the old fixed 56-column wrap",
        );
    }

    #[test]
    fn goal_parenthetical_claims_its_own_row_when_it_would_not_fit() {
        // When the final objective row is full, the status/budget parenthetical
        // must drop to its own indented row instead of clipping off the edge.
        let width = 80u16;
        let body = goal_objective_body_width(width);
        let objective = "a".repeat(body * 2); // fills exactly two full rows
        assert_eq!(
            goal_objective_chunks(&objective, width, 0).len(),
            2,
            "objective alone fills exactly two rows",
        );
        let with_tail = goal_objective_chunks(&objective, width, 20);
        assert_eq!(
            with_tail.len(),
            3,
            "a non-fitting parenthetical must claim its own trailing row",
        );
        assert!(
            with_tail.last().is_some_and(String::is_empty),
            "the trailing row is empty — the parenthetical renders alone on it",
        );
    }

    #[test]
    fn render_autonomy_indicator_goal_and_loops_render_two_rows() {
        let mut app = autonomy_app_state();
        let session_id = SessionKey("local:test".into());
        app.set_session_goal(
            &session_id,
            Some(octos_core::ui_protocol::UiGoalRecord {
                profile_id: Some("coding".into()),
                goal_id: "goal_01".into(),
                objective: "finish OAuth refactor".into(),
                status: "active".into(),
                token_budget: 50_000,
                tokens_used: 12_000,
                time_used_seconds: 0,
                created_at_ms: 1,
                updated_at_ms: 2,
            }),
            Some("user".into()),
        );
        app.set_session_loops(
            &session_id,
            vec![
                sample_loop("l1", "deploy-check", "fixed_interval", Some(300)),
                sample_loop("l2", "PR-watch", "self_paced", None),
            ],
        );

        assert_eq!(autonomy_indicator_height(&app, 100), 2);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        assert_eq!(lines.len(), 2);

        let text = rendered_text(&app);
        assert!(text.contains("Goal:"));
        assert!(text.contains("finish OAuth refactor"));
        assert!(text.contains("Loops: 2 active"));
        assert!(text.contains("5m deploy-check"));
        assert!(text.contains("self-paced PR-watch"));
    }

    #[test]
    fn autonomy_indicator_hides_when_only_paused_loops_remain() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // Nothing is firing — paused-only sessions must not pin a loops row
        // above the composer (user report: three long-parked test loops kept
        // a permanent "0 active · 3 paused" banner). The status-bar chip
        // remains the discoverable hint that `/loop` has parked entries.
        let mut l1 = sample_loop("l1", "deploy-check", "fixed_interval", Some(300));
        l1.status = "paused".into();
        let mut l2 = sample_loop("l2", "PR-watch", "self_paced", None);
        l2.status = "paused".into();
        app.set_session_loops(&session_id, vec![l1, l2]);

        assert_eq!(
            autonomy_indicator_height(&app, 100),
            0,
            "paused-only loops must not reserve an indicator row"
        );
        let text = rendered_text(&app);
        assert!(
            !text.contains("Loops:"),
            "paused-only loops must hide the loops row, got:\n{text}"
        );
    }

    #[test]
    fn autonomy_indicator_keeps_paused_suffix_beside_active_loops() {
        let session_id = SessionKey("local:test".into());
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        // With at least one ACTIVE loop the row shows, and paused siblings
        // still reconcile with their (muted) chips.
        let l1 = sample_loop("l1", "deploy-check", "fixed_interval", Some(300));
        let mut l2 = sample_loop("l2", "PR-watch", "self_paced", None);
        l2.status = "paused".into();
        app.set_session_loops(&session_id, vec![l1, l2]);

        let text = rendered_text(&app);
        assert!(
            text.contains("Loops: 1 active · 1 paused"),
            "active row must keep the paused suffix, got:\n{text}"
        );
    }

    #[test]
    fn harness_context_ratio_uses_real_window_when_known() {
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 64_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        // No known window yet → fall back to the fixed default (64000/128000).
        assert_eq!(harness_context_ratio(&app), Some(0.5));

        // Once the real per-model window arrives on the wire (here 256k), the
        // SAME token estimate is honestly a quarter full — not a misleading 50%.
        app.session_context_window
            .insert(session_id.clone(), 256_000);
        assert_eq!(harness_context_ratio(&app), Some(0.25));

        // A tiny window clamps to a full gauge rather than overflowing.
        app.session_context_window.insert(session_id.clone(), 1_000);
        assert_eq!(harness_context_ratio(&app), Some(1.0));
    }

    #[test]
    fn harness_line_shows_the_persona_word_over_the_working_phase() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        // Active turn (word keys to it) with a plain working phase.
        let turn_id = octos_core::ui_protocol::TurnId::new();
        app.sessions[0].live_reply = Some(crate::model::LiveReply {
            turn_id: turn_id.clone(),
            text: String::new(),
        });
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        app.session_status_word
            .insert(session_id.clone(), (turn_id.clone(), "Conjuring".into()));

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            text.contains("Conjuring…"),
            "persona word shows with ellipsis: {text:?}"
        );
        assert!(
            !text.contains("Working"),
            "the flat Working phase is replaced: {text:?}"
        );

        // A REAL orchestrating phase (sub-agents) keeps its informative label,
        // not the decorative word.
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 2,
                pending_continuations: 0,
                phase: Some("orchestrating".into()),
            },
        );
        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            text.contains("Orchestrating"),
            "orchestrating phase kept: {text:?}"
        );
        assert!(
            !text.contains("Conjuring"),
            "word does not mask a real phase: {text:?}"
        );
    }

    #[test]
    fn harness_status_row_surfaces_orchestration_usage_and_context() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();

        // Idle: no orchestration, no active turn → row reserves no rows and is
        // absent from the render (so it cannot collide with the composer).
        assert_eq!(harness_status_height(&app), 0);
        assert!(harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true).is_empty());

        // Orchestrating: active, 2 running agents, 1 pending continuation.
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 2,
                pending_continuations: 1,
                phase: Some("orchestrating".into()),
            },
        );
        app.session_usage
            .insert(session_id.clone(), (Some(34_211), Some(374), Some(0.0123)));
        // Context usage (token_estimate) is inspector-only today — surface it
        // as ctx N% in the harness row.
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 64_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        assert_eq!(
            harness_status_height(&app),
            1,
            "active row reserves one row"
        );
        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(text.contains("Orchestrating"), "{text:?}");
        assert!(text.contains("2 agents"), "{text:?}");
        assert!(text.contains("re-entering"), "{text:?}");
        assert!(text.contains("↑34.2k"), "{text:?}");
        assert!(text.contains("↓374"), "{text:?}");
        assert!(text.contains("$0.0123"), "{text:?}");
        assert!(
            text.contains("ctx 64K/128K ~50%"),
            "ctx used/max token counts + estimate percent: {text:?}"
        );
        // Context ratio drives the LineGauge (64000 / 128000 = 0.5).
        assert_eq!(harness_context_ratio(&app), Some(0.5));

        // Even with the row ACTIVE the composer's top-border chrome survives —
        // the indicator lives on its own dedicated layout row, not the border
        // (the collision that caused the 249fe652 revert cannot recur).
        let rendered = rendered_text(&app);
        assert!(
            rendered.contains("Orchestrating"),
            "active row renders: {rendered:?}"
        );
        assert!(
            rendered.contains("Composer"),
            "composer chrome intact: {rendered:?}"
        );
        assert!(
            rendered.contains("Tab agents"),
            "composer hint not clobbered: {rendered:?}"
        );
        // Regression (duplicate ctx readout): on a wide terminal (rendered_text
        // uses 120 cols, so the gauge column is drawn) the context label must
        // render ONCE — as the LineGauge on the right, NOT also as the textual
        // `· ctx …` label on the left. Pre-fix this row showed both "· ctx ~50%"
        // and "ctx ~50% ───" on the same line.
        assert_eq!(
            rendered.matches("64K/128K").count(),
            1,
            "ctx readout must render exactly once (gauge only) on a wide terminal: {rendered:?}"
        );
    }

    #[test]
    fn harness_status_row_ctx_label_marks_estimate() {
        // Nit: ctx% uses a fixed DEFAULT_CONTEXT_WINDOW_TOKENS denominator (no
        // per-model window on the wire), so the label must read as an ESTIMATE
        // (`ctx ~N%`) rather than an exact figure that would mislead when the
        // real model window differs.
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 32_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(
            text.contains("ctx 32K/128K ~25%"),
            "ctx label must carry the used/max counts and the approximate marker: {text:?}"
        );
    }

    #[test]
    fn harness_status_row_shows_used_over_max_against_real_window() {
        use octos_core::ui_protocol::SessionOrchestrationEvent;
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        app.context_lifecycle_mut(&session_id).state = Some(crate::model::ContextLifecycleState {
            session_id: session_id.clone(),
            thread_id: None,
            generation: 1,
            transcript_hash: String::new(),
            item_count: 10,
            token_estimate: 128_000,
            recovery_state: "healthy".into(),
            last_checkpoint_id: None,
            last_compaction_id: None,
        });
        // Real per-model window on the wire → used/max reads against it, not the
        // fixed default: 128K of a 1M window is an honest ~13%.
        app.session_context_window
            .insert(session_id.clone(), 1_000_000);

        // Narrow terminal (text fallback) carries the full readout.
        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(
            text.contains("ctx 128K/1M ~13%"),
            "harness row shows used/max token counts + estimate percent: {text:?}"
        );

        // Wide terminal draws the same numbers in the gauge label.
        let rendered = rendered_text(&app);
        assert!(
            rendered.contains("128K/1M"),
            "wide gauge label carries the used/max counts: {rendered:?}"
        );
    }

    #[test]
    fn harness_status_row_surfaces_retry_state() {
        use octos_core::ui_protocol::{SessionOrchestrationEvent, UiRetryBackoff};
        let session_id = SessionKey("local:test".into());
        let mut app = autonomy_app_state();
        app.orchestration.insert(
            session_id.clone(),
            SessionOrchestrationEvent {
                session_id: session_id.clone(),
                active: true,
                running_agents: 0,
                pending_continuations: 0,
                phase: Some("working".into()),
            },
        );
        let mut retry = UiRetryBackoff::new();
        retry.attempt = Some(3);
        app.session_retry.insert(session_id, retry);

        let text: String = harness_status_lines(&app, Palette::for_theme(ThemeName::Codex), true)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.to_string())
            .collect();
        assert!(
            text.to_lowercase().contains("retry") || text.to_lowercase().contains("retrying"),
            "retry state must render in the harness row: {text:?}"
        );
        assert!(
            text.contains('3'),
            "retry attempt number must render: {text:?}"
        );
    }

    #[test]
    fn harness_status_row_does_not_collide_with_composer_when_idle() {
        // Idle render: the dedicated harness row takes height 0, so the
        // composer's top-border chrome ("Composer  Enter send | Tab agents")
        // is fully intact — the collision that caused the prior revert
        // (249fe652) cannot recur because the indicator is never on the border.
        let app = autonomy_app_state();
        assert_eq!(harness_status_height(&app), 0);
        let text = rendered_text(&app);
        assert!(text.contains("Composer"), "{text:?}");
        assert!(text.contains("Tab agents"), "{text:?}");
        assert!(
            !text.contains("Orchestrating"),
            "idle harness row must be absent: {text:?}"
        );
    }

    #[test]
    fn autonomy_loop_label_truncates_long_prompt_with_ellipsis() {
        let long = octos_core::ui_protocol::UiLoopRecord {
            loop_id: "l1".into(),
            session_id: SessionKey("local:test".into()),
            profile_id: None,
            prompt: "this prompt is intentionally far too long to fit in a chip".into(),
            mode: "self_paced".into(),
            interval_seconds: None,
            status: "active".into(),
            next_run_at_ms: None,
            last_run_at_ms: None,
            expires_at_ms: 999,
            created_at_ms: 1,
            updated_at_ms: 2,
        };
        let label = autonomy_loop_label(&long);
        assert!(
            label.chars().count() <= AUTONOMY_LOOP_LABEL_MAX,
            "label {label:?} should respect AUTONOMY_LOOP_LABEL_MAX",
        );
        assert!(label.ends_with('…'));
    }

    // ---- inline-viewport (scrollback) rendering ----

    fn chat_app(messages: Vec<Message>) -> AppState {
        AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages,
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        )
    }

    fn app_with_large_menu() -> AppState {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.menu_stack.open("geometry.test");
        let items = (0..20)
            .map(|idx| {
                crate::menu::MenuItem::new(
                    format!("geometry.item.{idx}"),
                    format!("Geometry item {idx}"),
                    crate::menu::MenuAction::Noop,
                )
            })
            .collect();
        app.active_menu = Some(crate::menu::MenuBuildResult::ready(
            crate::menu::MenuSpec::new(
                "geometry.test",
                "Geometry test",
                crate::menu::MenuMode::SingleSelect,
            )
            .with_items(items),
        ));
        app
    }

    #[test]
    fn chat_layout_areas_keep_composer_and_status_at_bottom() {
        let app = chat_app(vec![Message::user("hi"), Message::assistant("ready")]);
        let area = Rect::new(0, 0, 80, 24);

        let layout = chat_layout_areas(&app, area);

        assert_eq!(layout.status.y, area.y + area.height - 1);
        assert_eq!(layout.status.height, 1);
        assert_eq!(
            layout.composer.y + layout.composer.height,
            layout.status.y,
            "composer must sit immediately above the status row"
        );
        assert_eq!(layout.transcript.y, area.y);
        assert!(
            layout.transcript.y + layout.transcript.height <= layout.menu.y,
            "transcript and menu areas must not overlap"
        );
    }

    #[test]
    fn chat_layout_areas_clamp_menu_to_transcript_budget() {
        let app = app_with_large_menu();
        let area = Rect::new(0, 0, 80, 19);

        let layout = chat_layout_areas(&app, area);

        assert_eq!(
            layout.menu.height, 4,
            "large menus are clamped by the available surface budget"
        );
        assert!(
            layout.transcript.height >= min_transcript_height(area.height),
            "menu must not steal the transcript's minimum height"
        );
        assert_eq!(layout.status.y, area.y + area.height - 1);
        assert_eq!(layout.composer.y + layout.composer.height, layout.status.y);
    }

    #[test]
    fn render_chat_layout_matches_chat_layout_areas() {
        let mut app = chat_app(vec![
            Message::user("ask number 01"),
            Message::assistant("history message 01"),
        ]);
        app.transcript_pager_active = true;
        let area = Rect::new(0, 0, 80, 20);
        let layout = chat_layout_areas(&app, area);

        let buffer = rendered_buffer_with_size(
            &app,
            Palette::for_theme(ThemeName::default()),
            area.width,
            area.height,
        );
        let rows = rendered_rows(&buffer);
        let composer_row = row_index_containing(&rows, "Composer") as u16;
        assert!(
            composer_row >= layout.composer.y
                && composer_row < layout.composer.y + layout.composer.height,
            "composer title row {composer_row} must be inside {:?}",
            layout.composer
        );
        for y in layout.composer.y..layout.composer.y + layout.composer.height {
            assert!(
                !rows[usize::from(y)].contains("history message"),
                "transcript text must not render inside composer area at row {y}: {:?}",
                rows[usize::from(y)]
            );
        }
    }

    #[test]
    fn scrollbar_thumb_hidden_without_overflow() {
        let track = Rect::new(79, 0, 1, 10);
        let metrics = TranscriptScrollMetrics {
            visible_rows: 20,
            total_rows: 20,
            scroll_from_bottom: 0,
            max_scroll_from_bottom: 0,
        };

        assert_eq!(scrollbar_thumb(metrics, track), None);
    }

    #[test]
    fn scrollbar_thumb_places_bottom_at_track_end() {
        let track = Rect::new(79, 5, 1, 10);
        let metrics = TranscriptScrollMetrics {
            visible_rows: 20,
            total_rows: 100,
            scroll_from_bottom: 0,
            max_scroll_from_bottom: 80,
        };

        let thumb = scrollbar_thumb(metrics, track).expect("overflow thumb");

        assert_eq!(thumb.height, 2);
        assert_eq!(thumb.top + thumb.height, track.y + track.height);
    }

    #[test]
    fn scrollbar_thumb_moves_toward_top_when_scrolled_up() {
        let track = Rect::new(79, 5, 1, 10);
        let bottom = scrollbar_thumb(
            TranscriptScrollMetrics {
                visible_rows: 20,
                total_rows: 100,
                scroll_from_bottom: 0,
                max_scroll_from_bottom: 80,
            },
            track,
        )
        .expect("bottom thumb");
        let scrolled = scrollbar_thumb(
            TranscriptScrollMetrics {
                visible_rows: 20,
                total_rows: 100,
                scroll_from_bottom: 40,
                max_scroll_from_bottom: 80,
            },
            track,
        )
        .expect("scrolled thumb");

        assert!(
            scrolled.top < bottom.top,
            "scrolling up should move the thumb toward the top"
        );
    }

    #[test]
    fn hint_bar_model_defaults_to_statusbar_keys() {
        let app = chat_app(vec![Message::user("hi")]);

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::StatusbarKeys);
    }

    #[test]
    fn hint_bar_model_uses_pager_keys_at_bottom() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.transcript_pager_active = true;
        app.transcript_scroll = 0;

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::PagerKeys);
    }

    #[test]
    fn hint_bar_model_uses_reviewing_when_pager_scrolled() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.transcript_pager_active = true;
        app.transcript_scroll = 3;

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::PagerReviewing);
    }

    #[test]
    fn hint_bar_model_uses_menu_when_menu_is_active() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.menu_stack
            .open(crate::menu::MenuId::from(crate::menu::registry::MENU_HELP));

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Menu);
    }

    #[test]
    fn hint_bar_model_uses_onboarding_for_first_launch_menu() {
        let mut app = AppState::new(vec![], 0, "ready".into(), None, false);
        app.menu_stack.open(crate::menu::MenuId::from(
            crate::menu::registry::MENU_ONBOARD,
        ));

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Onboarding);
    }

    #[test]
    fn hint_bar_model_uses_approval_when_visible() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.approval = Some(ApprovalModalState {
            session_id: SessionKey("local:test".into()),
            approval_id: ApprovalId::new(),
            turn_id: TurnId::new(),
            tool_name: "shell".into(),
            title: "Run command?".into(),
            body: "cargo test".into(),
            approval_kind: Some(approval_kinds::COMMAND.into()),
            risk: None,
            typed_details: None,
            render_hints: None,
            visible: true,
        });

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::Approval);
    }

    #[test]
    fn hint_bar_model_uses_user_question_when_visible() {
        let mut app = chat_app(vec![Message::user("hi")]);
        app.user_question = Some(UserQuestionPickerState {
            session_id: SessionKey("local:test".into()),
            question_id: QuestionId::new(),
            turn_id: TurnId::new(),
            title: "Choose path".into(),
            body: "Which option?".into(),
            questions: vec![],
            active: 0,
            visible: true,
        });

        assert_eq!(hint_bar_model(&app).mode, HintBarMode::UserQuestion);
    }

    #[test]
    fn decision_banner_reserves_a_row_and_shows_submit_hint_for_a_pending_question() {
        let mut app = chat_app(vec![Message::user("hi")]);
        // No question → no banner row reserved.
        assert_eq!(decision_banner_height(&app), 0);
        assert!(pending_question_for_banner(&app).is_none());

        app.user_question = Some(UserQuestionPickerState {
            session_id: SessionKey("local:test".into()),
            question_id: QuestionId::new(),
            turn_id: TurnId::new(),
            title: "Choose path".into(),
            body: "Which option?".into(),
            questions: vec![crate::model::UserQuestionEntry {
                header: "Path".into(),
                question: "Which?".into(),
                options: vec![],
                multi_select: true,
                option_selected: vec![],
                free_text: String::new(),
                cursor: 0,
                editing_free_text: false,
            }],
            active: 0,
            visible: true,
        });
        // A visible, non-empty question reserves the always-on banner row so the
        // submit affordance can never scroll off the height-capped live tail.
        assert!(pending_question_for_banner(&app).is_some());
        assert_eq!(decision_banner_height(&app), 1);

        // Dismissed (Ctrl+R/Alt+A) → banner row released; the picker no longer owns the
        // reserved chrome.
        app.user_question.as_mut().unwrap().visible = false;
        assert_eq!(decision_banner_height(&app), 0);
        assert!(pending_question_for_banner(&app).is_none());
    }

    /// Render `render_viewport` into a buffer via the custom inline `Frame`, at
    /// the live-UI height the event loop would size it to. We render straight
    /// into a `Buffer` (no escape-emitting backend needed) so the test does not
    /// require a `Write` backend.
    fn viewport_rows(app: &AppState, width: u16, height: u16) -> Vec<String> {
        viewport_rows_with_finalization(app, width, height, None)
    }

    fn viewport_rows_with_finalization(
        app: &AppState,
        width: u16,
        height: u16,
        live_finalization: Option<&LiveTurnFinalization>,
    ) -> Vec<String> {
        let palette = Palette::for_theme(ThemeName::Slate);
        let live_height =
            super::super::live_ui_height_with_finalization(app, width, height, live_finalization);
        let area = Rect::new(0, 0, width, live_height);
        let mut buffer = Buffer::empty(area);
        let mut frame = crate::tui_terminal::Frame::for_test(area, &mut buffer);
        render_viewport_with_finalization(&mut frame, app, palette, height, live_finalization);
        rendered_rows(&buffer)
    }

    fn lines_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    /// #407 regression: peers must stay in the dock counts AFTER their
    /// `session/opened` lands — the prior implementation keyed off
    /// `pending_peer_kickoffs`, which is popped at open, so a running fleet
    /// read `👥 0`. With the durable `peer_session_meta` roster, a peer
    /// recorded via `take_pending_peer_kickoff` stays counted for life.
    #[test]
    fn peer_dock_counts_survive_session_opened() {
        let mut app = autonomy_app_state();
        // No peers yet → all counts zero, height 0.
        assert_eq!(peer_dock_counts(&app), (0, 0, 0, 0));
        assert_eq!(peer_strip_height(&app, 40), 0);

        // Stage a peer: insert into pending (simulates peer/staged) AND pop
        // it via take_pending_peer_kickoff (simulates session/opened landing).
        let sid = SessionKey("local:tui#peer-refactor".into());
        app.pending_peer_kickoffs.insert(
            sid.clone(),
            crate::model::PeerKickoff {
                brief: "refactor auth".into(),
                brief_path: "/tmp/brief.md".into(),
                go: false,
                agent_staged: false,
                model_id: None,
                created: std::time::Instant::now(),
            },
        );
        let _ = app.take_pending_peer_kickoff(&sid);
        // After the take, pending is empty BUT the durable roster has it.
        assert!(app.pending_peer_kickoffs.is_empty());
        assert_eq!(app.peer_session_meta.len(), 1, "durable roster recorded");

        // total counts the opened peer (review F1: previously 0 here).
        let (total, live, blocked, unread) = peer_dock_counts(&app);
        assert_eq!(total, 1, "opened peer counts toward total");
        assert_eq!(live, 0, "no live turn yet");
        assert_eq!(blocked, 0);
        assert_eq!(unread, 0);
        // Dock reserves rows now that a peer exists.
        assert_eq!(
            peer_strip_height(&app, 40),
            2,
            "title row + one peer row once a peer exists"
        );
        let _ = (live, blocked, unread);
    }

    /// #407 regression: the collapsed pill renders structured peer counts
    /// (not bare `+N`).
    #[test]
    fn peer_dock_pill_renders_total_segment() {
        let mut app = autonomy_app_state();
        let sid = SessionKey("local:tui#peer-cired".into());
        app.pending_peer_kickoffs.insert(
            sid.clone(),
            crate::model::PeerKickoff {
                brief: "ci-red".into(),
                brief_path: "/tmp/brief.md".into(),
                go: false,
                agent_staged: false,
                model_id: None,
                created: std::time::Instant::now(),
            },
        );
        let _ = app.take_pending_peer_kickoff(&sid);

        app.peer_dock_collapsed = true;
        let pill = peer_dock_pill_line(&app, Palette::for_theme(app.theme));
        let text: String = pill.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            text.contains("1 peer") || text.contains("peers"),
            "total segment renders; got: {text}"
        );
    }

    /// #407 regression: `peer_sessions`/roster sort is deterministic on
    /// `Instant` ties (review F10) — a fleet staged in one burst must not
    /// flicker row order across frames.
    #[test]
    fn peer_dock_roster_sort_is_deterministic_on_ties() {
        let mut app = autonomy_app_state();
        let now = std::time::Instant::now();
        // Three peers staged at the SAME instant (simulates a tight fleet
        // loop where `Instant::now()` ties on coarse clocks).
        for slug in ["zebra", "alpha", "mike"] {
            let sid = SessionKey(format!("local:tui#peer-{slug}"));
            app.peer_session_meta.insert(
                sid,
                crate::model::PeerMeta {
                    slug: slug.into(),
                    brief_path: "/tmp/brief.md".into(),
                    agent_staged: true,
                    model_id: None,
                    created: now,
                },
            );
        }
        let order_a: Vec<&str> = peer_dock_roster(&app)
            .iter()
            .map(|(_, m)| m.slug.as_str())
            .collect();
        // Run again — stable order regardless of HashMap iteration.
        let order_b: Vec<&str> = peer_dock_roster(&app)
            .iter()
            .map(|(_, m)| m.slug.as_str())
            .collect();
        assert_eq!(
            order_a, order_b,
            "roster order must not flicker across calls; got {order_a:?} vs {order_b:?}"
        );
        // Tie-break is by session key → alpha, mike, zebra (deterministic).
        assert_eq!(
            order_a,
            vec!["alpha", "mike", "zebra"],
            "tie-break on session key; got {order_a:?}"
        );
    }

    #[test]
    fn viewport_renders_live_ui_not_committed_history() {
        // Committed messages live in scrollback (finalized_history_lines), NOT
        // in the inline viewport. The viewport shows the composer + status.
        let app = chat_app(vec![
            Message::user("an old committed question"),
            Message::assistant("an old committed answer"),
        ]);
        let rows = viewport_rows(&app, 100, 40);
        let text = rows.join("\n");
        assert!(
            text.contains("Composer"),
            "viewport should show the composer chrome, got:\n{text}"
        );
        assert!(
            !text.contains("an old committed answer"),
            "committed history must go to scrollback, not the viewport:\n{text}"
        );
    }

    #[test]
    fn finalized_history_lines_contain_committed_messages() {
        let app = chat_app(vec![
            Message::user("question one"),
            Message::assistant("answer one"),
        ]);
        let palette = Palette::for_theme(ThemeName::Slate);
        let lines = finalized_history_lines(&app, palette, 80);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("question one"), "missing user msg: {text:?}");
        assert!(
            text.contains("answer one"),
            "missing assistant msg: {text:?}"
        );
    }

    #[test]
    fn active_turn_completed_activity_flushes_to_scrollback_and_leaves_live_tail() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("run the checks")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "Still checking the last item".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "running")
                .with_turn(turn_id.clone())
                .with_tool_call("call-running")
                .with_detail("cargo clippy --all-targets"),
        );
        app.push_activity(
            ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                .with_turn(turn_id)
                .with_tool_call("call-complete")
                .with_detail("cargo test")
                .with_success(true),
        );

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("Agent task completed") && inserted.contains("Bash($ cargo test"),
            "completed activity should be inserted into scrollback mid-turn: {inserted:?}"
        );
        assert!(
            !inserted.contains("cargo clippy --all-targets"),
            "running activity must stay in the live tail: {inserted:?}"
        );

        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("cargo test"),
            "flushed activity must not remain in the repainting viewport:\n{live}"
        );
        assert!(
            live.contains("cargo clippy --all-targets") && live.contains("Orchestrating"),
            "running activity should remain as the small live tail:\n{live}"
        );

        // Fix #7: EVERY live-tail row must be visible, top rows included. The
        // borderless live tail used to reserve a phantom 2-row border
        // allowance, scrolling its top 2 rows out of the area and leaving 2
        // dead rows at the bottom whenever the tail was >= 2 rows.
        let tail_lines = live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            98,
            update.live_tail_finalization.as_ref(),
        );
        assert!(
            tail_lines.len() >= 2,
            "precondition: the phantom allowance only bites on a >=2-row tail"
        );
        for line in &tail_lines {
            let text: String = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect();
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            assert!(
                live.contains(text),
                "live-tail row {text:?} must be rendered (top rows included):\n{live}"
            );
        }
    }

    #[test]
    fn glued_completed_segment_flushes_via_boundary_so_live_tail_holds_only_current_segment() {
        // Agentic narration segments are glued in live_reply (no blank line
        // between "…step one.step two:"), so the blank-line flush watermark never
        // advances and the whole growing reply piles up in the height-limited live
        // tail, clipping to its bottom ("intermediate truncated"). A completed
        // segment boundary (recorded when its tool call started) must flush the
        // finished segment so the live tail holds only the in-progress one.
        let turn_id = TurnId::new();
        let session = SessionKey("local:test".into());
        let head = "segment one glued.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: format!("{head}segment two still live"),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        // Boundary recorded at the tool call between segment one and two. There is
        // no blank line, so only this boundary can advance the watermark.
        app.live_reply_segment_boundaries
            .insert((session, turn_id), vec![head.len()]);

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("segment one glued."),
            "completed boundary-terminated segment must flush to scrollback even \
             without a blank line: {inserted:?}"
        );
        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("segment one glued."),
            "flushed segment must not remain in the live tail:\n{live}"
        );
        assert!(
            live.contains("segment two still live"),
            "the in-progress segment stays in the live tail:\n{live}"
        );
    }

    #[test]
    fn word_safe_boundary_rejects_mid_word_splits() {
        // Mid-word (both neighbors are word chars) -> rejected, so a message/persisted
        // event sampling the live buffer at "anim|ate" never splits/flushes mid-word.
        assert!(!boundary_is_word_safe("animate", 4));
        assert!(!boundary_is_word_safe("haloPhase", 7));
        // Adjacent to a delimiter -> accepted (real segment ends still pass).
        assert!(boundary_is_word_safe("loop: next", 5));
        assert!(boundary_is_word_safe("done. Now", 5));
        assert!(boundary_is_word_safe("a\nb", 2));
        assert!(boundary_is_word_safe("end", 3));
        // Non-char-boundary -> rejected (safe).
        assert!(!boundary_is_word_safe("五大", 1));
    }

    #[test]
    fn active_turn_completed_reply_lines_flush_to_scrollback_and_leave_only_suffix_live() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("summarize")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "finalized assistant line\n\nstreaming suffix still live".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let mut tracker = ScrollbackTracker::new();
        let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let inserted = lines_text(&update.lines_to_insert);
        assert!(
            inserted.contains("finalized assistant line"),
            "completed reply line should be inserted into scrollback mid-turn: {inserted:?}"
        );
        assert!(
            !inserted.contains("streaming suffix still live"),
            "unterminated reply suffix must stay live: {inserted:?}"
        );

        let rows =
            viewport_rows_with_finalization(&app, 100, 40, update.live_tail_finalization.as_ref());
        let live = rows.join("\n");
        assert!(
            !live.contains("finalized assistant line"),
            "flushed reply line must not remain in the repainting viewport:\n{live}"
        );
        assert!(
            live.contains("streaming suffix still live"),
            "only the active reply suffix should remain live:\n{live}"
        );
    }

    #[test]
    fn live_delta_segment_boundary_starts_fresh_markdown_block() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_segment = "### Step 1\n\nBody one.";
        let second_segment = "### Step 2\n\nBody two.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("build a demo")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: first_segment.into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let previous = next_live_turn_finalization(&app, None).expect("first watermark");
        assert_eq!(previous.reply_flushed_text, "### Step 1\n\n");

        app.live_reply_segment_boundaries.insert(
            (session_id.clone(), turn_id.clone()),
            vec![first_segment.len()],
        );
        app.sessions[0].live_reply.as_mut().unwrap().text =
            format!("{first_segment}{second_segment}");
        let next = next_live_turn_finalization(&app, Some(&previous)).expect("next watermark");

        let rendered = line_texts(&finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            &previous,
            &next,
        ));
        let body = rendered
            .iter()
            .position(|line| line == "  Body one.")
            .expect("first segment body should render before the boundary");
        let heading = rendered
            .iter()
            .position(|line| line == "  Step 2")
            .expect("second segment heading should render as markdown");

        assert_eq!(
            rendered.get(body + 1).map(String::as_str),
            Some(""),
            "segment boundary should force a blank paragraph break: {rendered:#?}"
        );
        assert_eq!(
            heading,
            body + 2,
            "Step 2 should be a discrete heading immediately after the boundary break: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("###")),
            "markdown heading markers must not leak in live scrollback: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("Body one.###")),
            "segment boundary must prevent body/header gluing: {rendered:#?}"
        );
    }

    #[test]
    fn committed_assistant_segment_boundary_starts_fresh_markdown_block() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_segment = "**Step 1:** a.";
        let second_segment = "**Step 2:** b.";
        let content = format!("{first_segment}{second_segment}");
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build a demo"),
                    Message::assistant(content.as_str()),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_prompt_anchors.push(TurnPromptAnchor {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            content: "build a demo".into(),
            anchor_index: 0,
            prior_matching_user_count: 0,
        });
        app.live_reply_segment_boundaries
            .insert((session_id, turn_id), vec![first_segment.len()]);

        let rendered = line_texts(&finalized_history_lines_range_dedup_live(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            1,
            &[],
        ));
        let first = rendered
            .iter()
            .position(|line| line == "• Step 1: a.")
            .expect("first segment should render as assistant prose");
        let second = rendered
            .iter()
            .position(|line| line == "  Step 2: b.")
            .expect("second segment should render as a discrete hanging markdown block");

        assert_eq!(
            rendered.get(first + 1).map(String::as_str),
            Some(""),
            "segment boundary should force a blank paragraph break: {rendered:#?}"
        );
        assert_eq!(
            second,
            first + 2,
            "Step 2 should not be glued onto Step 1: {rendered:#?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains("a.Step 2")),
            "committed assistant segment boundary must prevent gluing: {rendered:#?}"
        );
    }

    // ---- assistant body hanging indent ----
    // The reference shape (Claude Code): the `• ` marker sits on the FIRST
    // visual line of an assistant message, and EVERY other physical line of
    // the same message — paragraphs, list items, headings, code rows, wrapped
    // continuations — hangs two columns under it, so the body reads as one
    // contiguous block. Blank separators stay truly blank.

    /// The hanging indent must be exactly as wide as the `• ` marker, or the
    /// continuation lines don't align under the first line's text.
    #[test]
    fn assistant_hanging_indent_matches_marker_display_width() {
        assert_eq!(ASSISTANT_BODY_INDENT, "  ");
        assert_eq!(ASSISTANT_BODY_INDENT.width(), "• ".width());
    }

    /// The user-reported shape: a committed multi-paragraph assistant message
    /// used to drop every paragraph after the first back to column 0.
    #[test]
    fn committed_assistant_multi_paragraph_body_hangs_under_marker() {
        let app = chat_app(vec![
            Message::user("install android studio"),
            Message::assistant(
                "Homebrew is available. Let me install Android Studio via cask:\n\n\
                 Homebrew has permission issues in this sandbox. Let me download directly:\n\n\
                 The sandbox is blocking curl SSL and Homebrew. Let me try another way:",
            ),
        ]);
        let rendered = line_texts(&finalized_history_lines_range(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            1,
        ));
        assert_eq!(
            rendered,
            vec![
                "• Homebrew is available. Let me install Android Studio via cask:".to_string(),
                "".into(),
                "  Homebrew has permission issues in this sandbox. Let me download directly:"
                    .into(),
                "".into(),
                "  The sandbox is blocking curl SSL and Homebrew. Let me try another way:".into(),
            ],
            "the whole body must hang under the marker"
        );
    }

    /// A long single paragraph at a narrow wrap width: every wrapped
    /// continuation row carries the 2-column hang, no row exceeds the wrap
    /// width (unicode-width — CJK stays in budget), and no words are lost.
    #[test]
    fn assistant_wrapped_paragraph_rows_hang_and_fit_width() {
        let body = "The sandbox is blocking curl SSL and Homebrew so the 安装过程 must fall \
                    back to a manual download flow that keeps going for quite a while longer.";
        for wrap_width in [24usize, 40, 61, 80] {
            let mut lines = Vec::new();
            push_message_block(
                &mut lines,
                Palette::for_theme(ThemeName::Slate),
                "assistant",
                body,
                wrap_width,
            );
            let texts = line_texts(&lines);
            assert!(
                texts.len() > 1,
                "wrap_width {wrap_width} must produce wrapped rows: {texts:#?}"
            );
            for (idx, (line, text)) in lines.iter().zip(&texts).enumerate() {
                let w: usize = line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref().width())
                    .sum();
                assert!(
                    w <= wrap_width,
                    "row {idx} width {w} exceeds wrap_width {wrap_width}: {text:?}"
                );
                if idx == 0 {
                    assert!(
                        text.starts_with("• "),
                        "first row carries the marker: {text:?}"
                    );
                } else {
                    assert!(
                        text.starts_with("  ") && !text.starts_with("   "),
                        "wrapped continuation rows hang at exactly 2 columns: {text:?}"
                    );
                }
            }
            let mut rejoined = texts
                .iter()
                .map(|text| text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            rejoined = rejoined.trim_start_matches("• ").to_string();
            assert_eq!(
                rejoined.split_whitespace().collect::<Vec<_>>(),
                body.split_whitespace().collect::<Vec<_>>(),
                "wrapping must not drop or reorder words at wrap_width {wrap_width}"
            );
        }
    }

    /// The Claude-Code reference shape: a numbered-list body hangs its list
    /// rows AND their wrapped continuations under the marker line.
    #[test]
    fn assistant_numbered_list_hangs_items_and_wrapped_continuations() {
        let body = "Complete inventory, grouped by PR — the fixes:\n\n\
                    1. session_cost was turn-scoped and needed to become cumulative across \
                    the whole session lifetime\n\
                    2. the summary row double-counted cache reads";
        let wrap_width = 48usize;
        let mut lines = Vec::new();
        push_message_block(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            "assistant",
            body,
            wrap_width,
        );
        let texts = line_texts(&lines);
        assert!(
            texts[0].starts_with("• "),
            "first row carries the marker: {texts:#?}"
        );
        assert!(
            texts.iter().any(|text| text.starts_with("  1. ")),
            "list rows hang at 2 columns: {texts:#?}"
        );
        let item_row = texts
            .iter()
            .position(|text| text.starts_with("  1. "))
            .expect("numbered item row");
        assert!(
            texts[item_row + 1].starts_with("  ") && !texts[item_row + 1].trim().is_empty(),
            "the wrapped continuation of a long list item hangs too: {texts:#?}"
        );
        for (idx, (line, text)) in lines.iter().zip(&texts).enumerate() {
            let w: usize = line
                .spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum();
            assert!(
                w <= wrap_width,
                "row {idx} width {w} exceeds wrap_width {wrap_width}: {text:?}"
            );
            if text.trim().is_empty() {
                assert_eq!(text, "", "blank separators stay truly blank: {texts:#?}");
            } else if idx > 0 {
                assert!(
                    text.starts_with("  "),
                    "every non-blank body row hangs: {text:?}"
                );
            }
        }
    }

    /// A heading-first assistant message: the marker sits on the heading (the
    /// first visual line, CC-style) and the rest of the body hangs.
    #[test]
    fn assistant_heading_first_body_carries_marker_on_heading() {
        let mut lines = Vec::new();
        push_message_block(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            "assistant",
            "### Step 2\n\nNow I'll add a style block.",
            80,
        );
        assert_eq!(
            line_texts(&lines),
            vec!["• Step 2", "", "  Now I'll add a style block."],
            "the marker goes on the first visual line even when it is a heading"
        );
    }

    /// Fenced code inside an assistant body: the frame rows and code rows all
    /// hang under the marker line.
    #[test]
    fn assistant_code_block_rows_hang_under_marker() {
        let mut lines = Vec::new();
        push_message_block(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            "assistant",
            "Run this:\n\n```rust\nfn main() {}\n```",
            80,
        );
        let texts = line_texts(&lines);
        assert_eq!(texts[0], "• Run this:");
        assert!(
            texts.iter().any(|text| text.starts_with("  ┌─ rust")),
            "fence header hangs: {texts:#?}"
        );
        assert!(
            texts.iter().any(|text| text.starts_with("  │ fn main()")),
            "code rows hang: {texts:#?}"
        );
        assert!(
            texts.iter().any(|text| text.starts_with("  └─")),
            "fence footer hangs: {texts:#?}"
        );
    }

    /// Live streaming lane: the hang applies mid-stream. The first batch
    /// carries the marker; continuation batches hang every line and never
    /// re-issue the bullet.
    #[test]
    fn live_reply_batches_hang_under_marker_mid_stream() {
        let palette = Palette::for_theme(ThemeName::Slate);
        let mut first_batch = Vec::new();
        push_live_reply_block(
            &mut first_batch,
            palette,
            "Para one settled.\n\nPara two settled.",
            80,
            true,
        );
        assert_eq!(
            line_texts(&first_batch),
            vec!["• Para one settled.", "", "  Para two settled."],
            "first batch: marker on line 1, later paragraphs hang"
        );

        let mut continuation = Vec::new();
        push_live_reply_block(
            &mut continuation,
            palette,
            "Para three keeps going.",
            80,
            false,
        );
        assert_eq!(
            line_texts(&continuation),
            vec!["  Para three keeps going."],
            "continuation batches hang without re-issuing the bullet"
        );
    }

    /// Scrollback-flush lane (immutable — must be right the first time): the
    /// delta between two flush watermarks renders continuation chunks with the
    /// hang, and a wrapped-at-flush-time long paragraph hangs its rows.
    #[test]
    fn scrollback_flush_delta_hangs_continuation_lines() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let text = "first block done.\n\nsecond block, which is long enough that a narrow \
                    terminal must wrap it across several physical rows to fit.\n\n";
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: text.into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let wrap_width = 40usize;
        let mut mid = LiveTurnFinalization::new(&session_id, &turn_id);
        mid.reply_flushed_text = "first block done.\n\n".to_string();
        let next = next_live_turn_finalization(&app, Some(&mid)).expect("watermark");
        assert_eq!(next.reply_flushed_text, text, "fully settled text flushes");

        let second_batch = finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            wrap_width,
            &mid,
            &next,
        );
        let texts = line_texts(&second_batch);
        assert!(
            texts.iter().filter(|text| !text.trim().is_empty()).count() > 1,
            "the long continuation paragraph must wrap: {texts:#?}"
        );
        for (line, text) in second_batch.iter().zip(&texts) {
            let w: usize = line
                .spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum();
            assert!(
                w <= wrap_width,
                "flushed row width {w} exceeds wrap_width {wrap_width}: {text:?}"
            );
            if !text.trim().is_empty() {
                assert!(
                    text.starts_with("  ") && !text.starts_with("• "),
                    "flushed continuation rows hang, no re-issued bullet: {text:?}"
                );
            }
        }
    }

    /// Regression guard: the other roles keep their own prefix systems — no
    /// 2-space hang leaks into user prompts or tool bodies.
    #[test]
    fn non_assistant_roles_keep_their_prefixes_unchanged() {
        let palette = Palette::for_theme(ThemeName::Slate);

        let mut user_lines = Vec::new();
        push_message_block(&mut user_lines, palette, "user", "line one\nline two", 80);
        assert_eq!(
            line_texts(&user_lines),
            vec!["▌ line one", "▌ line two"],
            "user prompts keep the gutter, gain no hang"
        );

        let mut tool_lines = Vec::new();
        push_message_block(
            &mut tool_lines,
            palette,
            "tool",
            "para one.\n\npara two.",
            80,
        );
        assert_eq!(
            line_texts(&tool_lines),
            vec!["$ para one.", "$ ", "$ para two."],
            "tool bodies keep their `$ ` gutter on every row, gain no hang"
        );

        let mut pending_lines = Vec::new();
        push_formatted_body(
            &mut pending_lines,
            palette,
            "queued question",
            "› ",
            None,
            80,
        );
        assert_eq!(
            line_texts(&pending_lines),
            vec!["› queued question"],
            "pending-message rows keep their `› ` prefix"
        );
    }

    /// codex-review (r2 P2): tabs render as FOUR columns once
    /// `insert_history` sanitizes scrollback, but the body used to be
    /// measured with the raw `\t` (0–1 columns), so a tab-bearing code row
    /// passed the pre-wrap check and was then re-wrapped to a column-zero
    /// continuation at insert time — losing the hang. Assistant bodies must
    /// sanitize (expand tabs, strip controls) BEFORE measuring, mirroring
    /// insert_history's sanitize-first-wrap-after order, so rendered rows
    /// carry no raw controls and stay within the wrap width post-expansion.
    #[test]
    fn assistant_body_expands_tabs_before_prewrap_measurement() {
        let wrap_width = 16usize;
        let mut lines = Vec::new();
        push_message_block(
            &mut lines,
            Palette::for_theme(ThemeName::Slate),
            "assistant",
            "```\nab\tcdefgh\n```\n\nprose\twith tab",
            wrap_width,
        );
        let texts = line_texts(&lines);
        for (idx, (line, text)) in lines.iter().zip(&texts).enumerate() {
            assert!(
                !text.contains('\t') && !text.chars().any(char::is_control),
                "row {idx} must carry no raw control characters: {text:?}"
            );
            let w: usize = line
                .spans
                .iter()
                .map(|span| span.content.as_ref().width())
                .sum();
            assert!(
                w <= wrap_width,
                "row {idx} width {w} exceeds wrap_width {wrap_width} after tab expansion: {texts:#?}"
            );
        }
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("  ") && text.contains("cdefgh")),
            "the tab-bearing code row still hangs: {texts:#?}"
        );
    }

    /// Empty and whitespace-only assistant bodies stay inert (no marker-only
    /// line, no panic).
    #[test]
    fn assistant_empty_and_whitespace_bodies_stay_inert() {
        let palette = Palette::for_theme(ThemeName::Slate);

        let mut empty_lines = Vec::new();
        push_message_block(&mut empty_lines, palette, "assistant", "", 80);
        assert_eq!(line_texts(&empty_lines), vec!["<empty>"]);

        let mut blank_lines = Vec::new();
        push_message_block(&mut blank_lines, palette, "assistant", "   ", 80);
        assert!(
            !line_texts(&blank_lines)
                .iter()
                .any(|text| text.contains('•')),
            "a whitespace-only body must not render a dangling marker"
        );
    }

    /// Scrollback-flush lane: consecutive PURE-activity delta flushes (each a
    /// sub-agent completing mid-turn, no reply text between them) must stay
    /// blank-separated in native scrollback. Regression for the "agent task
    /// cards pack together" report — each delta flush builds a fresh buffer, so
    /// the leading-blank guard inside `push_finalized_activity_items_section`
    /// sees an empty buffer and is skipped, leaving cards abutting.
    #[test]
    fn consecutive_scrollback_agent_task_cards_are_blank_separated() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "working".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let mut tracker = ScrollbackTracker::new();
        let mut inserted: Vec<Line<'static>> = Vec::new();
        for (call, detail) in [("call-1", "first task"), ("call-2", "second task")] {
            app.push_activity(
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id.clone())
                    .with_tool_call(call)
                    .with_detail(detail)
                    .with_success(true),
            );
            let update = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
            inserted.extend(update.lines_to_insert);
        }

        let texts = line_texts(&inserted);
        let cards = texts
            .iter()
            .enumerate()
            .filter_map(|(idx, text)| text.contains("Agent task completed").then_some(idx))
            .collect::<Vec<_>>();
        assert_eq!(
            cards.len(),
            2,
            "both completions flush as their own scrollback card: {texts:#?}"
        );
        assert!(
            texts[cards[0] + 1..cards[1]]
                .iter()
                .any(|text| text.trim().is_empty()),
            "consecutive scrollback agent-task cards must be blank-separated: {texts:#?}"
        );
    }

    #[test]
    fn streamed_code_fence_separator_survives_chunk_boundary() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let flushed_fence = "```rust\nfn main() {}\n```\n";
        let full = format!("{flushed_fence}\nAfter the block.\n\n");
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("show code")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: full,
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();

        let previous = LiveTurnFinalization::new(&session_id, &turn_id);
        let mut fence = LiveTurnFinalization::new(&session_id, &turn_id);
        fence.reply_flushed_text = flushed_fence.to_string();
        let next = next_live_turn_finalization(&app, Some(&fence)).expect("watermark");

        let mut streamed = finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            80,
            &previous,
            &fence,
        );
        streamed.extend(finalized_live_turn_lines_between(
            &app,
            Palette::for_theme(ThemeName::Slate),
            80,
            &fence,
            &next,
        ));

        let rendered = streamed
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let close = rendered
            .iter()
            .position(|line| line.contains("└─"))
            .expect("code fence close");
        let after = rendered
            .iter()
            .position(|line| line.contains("After the block."))
            .expect("paragraph after fence");
        assert_eq!(
            &rendered[close + 1..after],
            [""],
            "streaming should keep exactly one blank between code and prose: {rendered:#?}"
        );
    }

    #[test]
    fn committed_turn_does_not_duplicate_live_flushed_reply_or_activity() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let first_activity = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_turn(turn_id.clone())
            .with_detail("cargo test")
            .with_success(true);
        let second_activity_running = ActivityItem::new(ActivityKind::Tool, "shell", "running")
            .with_turn(turn_id.clone())
            .with_detail("cargo clippy --all-targets");
        let second_activity_done = ActivityItem::new(ActivityKind::Tool, "shell", "complete")
            .with_turn(turn_id.clone())
            .with_detail("cargo clippy --all-targets")
            .with_success(true);
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("finish the turn")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id: turn_id.clone(),
                    text: "already flushed line\n\nfinal answer tail".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app.push_activity(first_activity.clone());
        app.push_activity(second_activity_running);

        let mut tracker = ScrollbackTracker::new();
        let first = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let first_text = lines_text(&first.lines_to_insert);
        assert!(first_text.contains("already flushed line"));
        assert!(first_text.contains("Bash($ cargo test"));

        app.sessions[0].live_reply = None;
        app.sessions[0].messages.push(Message::assistant(
            "already flushed line\n\nfinal answer tail",
        ));
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id,
            request: Some("finish the turn".into()),
            anchor_index: Some(0),
            items: vec![first_activity, second_activity_done],
        });
        app.activity.clear();
        app.set_run_state_success();

        let second = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let second_text = lines_text(&second.lines_to_insert);
        assert!(
            !second_text.contains("already flushed line"),
            "committed assistant must not duplicate the live-flushed prefix: {second_text:?}"
        );
        assert!(
            second_text.contains("final answer tail"),
            "committed assistant should flush the unflushed suffix: {second_text:?}"
        );
        assert!(
            !second_text.contains("Bash($ cargo test"),
            "committed activity log must not duplicate the live-flushed action: {second_text:?}"
        );
        assert!(
            second_text.contains("cargo clippy --all-targets"),
            "committed activity log should flush the previously-running action: {second_text:?}"
        );

        app.sessions[0].messages.push(Message::user("new turn"));
        app.sessions[0].messages.push(Message::assistant(
            "already flushed line\nunrelated later answer",
        ));
        let third = tracker.sync(&app, Palette::for_theme(ThemeName::Slate), 100);
        let third_text = lines_text(&third.lines_to_insert);
        assert!(
            third_text.contains("already flushed line")
                && third_text.contains("unrelated later answer"),
            "stale live-prefix coverage must not suppress a later assistant message: {third_text:?}"
        );
    }

    #[test]
    fn committed_agentic_turn_keeps_later_assistant_messages_discrete() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let first = "### Step 1\n\nI'll create demo.html with an HTML5 skeleton.";
        let second = "### Step 2\n\nNow I'll add a style block.";
        let third = "### Step 3\n\nFinally, I'll add an <h1>.";
        let mut app = AppState::new(
            vec![SessionView {
                id: session_id.clone(),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("build a demo page"),
                    Message::assistant(first),
                    Message::assistant(second),
                    Message::assistant(third),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.turn_prompt_anchors.push(TurnPromptAnchor {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            content: "build a demo page".into(),
            anchor_index: 0,
            prior_matching_user_count: 0,
        });

        let coverage = LiveTurnFinalization {
            session_id: session_id.0,
            turn_id: turn_id.0.to_string(),
            reply_flushed_text: "### Step ".into(),
            ..Default::default()
        };
        let rendered = line_texts(&finalized_history_lines_range_dedup_live(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            1,
            &[coverage],
        ));

        assert_eq!(
            rendered,
            vec![
                "  1",
                "",
                "  I'll create demo.html with an HTML5 skeleton.",
                "",
                "• Step 2",
                "",
                "  Now I'll add a style block.",
                "",
                "• Step 3",
                "",
                "  Finally, I'll add an <h1>.",
            ],
            "later assistant messages must render as fresh markdown blocks (own marker, hanging body), not live-reply continuations"
        );
    }

    #[test]
    fn pending_prompt_present_in_session_history_renders_once() {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:test".into()),
                title: "test".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("active prompt"),
                    Message::assistant("partial answer"),
                    Message::user("queued next"),
                ],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: "still working".into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.pending_messages.push("queued next".into());
        app.set_run_state_in_progress();

        let rendered = line_texts(&live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            100,
            None,
        ));

        assert_eq!(
            rendered,
            vec![
                "• still working",
                "",
                "queued 1 messages after active turn",
                "› queued next",
            ],
            "a prompt that is still pending must not also render as recent user context"
        );
    }

    #[test]
    fn finalized_history_lines_range_skips_already_flushed() {
        let app = chat_app(vec![
            Message::user("q1"),
            Message::assistant("a1"),
            Message::user("q2"),
            Message::assistant("a2"),
        ]);
        let palette = Palette::for_theme(ThemeName::Slate);
        let tail = finalized_history_lines_range(&app, palette, 80, 2);
        let text: String = tail
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(text.contains("q2") && text.contains("a2"));
        assert!(
            !text.contains("a1"),
            "range(2) must not re-emit already-flushed messages: {text:?}"
        );
    }

    #[test]
    fn finalized_history_lines_include_anchored_activity_logs() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("build the site"),
            Message::assistant("The site is built."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("build the site".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo build")
                    .with_output_preview("Finished dev build")
                    .with_success(true),
            ],
        });

        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines_range(&app, palette, 80, 1);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");

        assert!(
            text.contains("The site is built."),
            "missing answer: {text:?}"
        );
        assert!(
            text.contains("Agent task completed"),
            "missing activity log: {text:?}"
        );
        assert!(
            text.contains("Bash($ cargo build"),
            "missing tool detail: {text:?}"
        );
    }

    #[test]
    fn scrollback_renders_full_tool_output_without_ctrl_o_hint() {
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("run tests"),
            Message::assistant("Done."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("run tests".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "bash", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_output_preview("line1\nline2\nline3\nline4\nline5")
                    .with_success(true),
            ],
        });

        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines_range(&app, palette, 80, 1);
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("\n");

        // Frozen scrollback shows the FULL output (the toggle can't repaint it)…
        for n in 1..=5 {
            assert!(
                text.contains(&format!("line{n}")),
                "missing line{n}:\n{text}"
            );
        }
        // …and never the un-actionable Ctrl+O hint.
        assert!(
            !text.contains("Ctrl+O"),
            "scrollback must not show a dead Ctrl+O hint:\n{text}"
        );
        assert!(
            !text.contains("hidden"),
            "scrollback must not hide output:\n{text}"
        );
    }

    #[test]
    fn finalized_history_lines_place_each_activity_log_after_own_reply() {
        let session_id = SessionKey("local:test".into());
        let turn_a = TurnId::new();
        let turn_b = TurnId::new();
        let mut app = chat_app(vec![
            Message::user("first prompt"),
            Message::assistant("First answer."),
            Message::user("second prompt"),
            Message::assistant("Second answer."),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id: session_id.clone(),
            turn_id: turn_a.clone(),
            request: Some("first prompt".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_a)
                    .with_detail("cargo test --first")
                    .with_success(true),
            ],
        });
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_b.clone(),
            request: Some("second prompt".into()),
            anchor_index: Some(2),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_b)
                    .with_detail("cargo test --second")
                    .with_success(true),
            ],
        });

        let lines = finalized_history_lines(&app, Palette::for_theme(ThemeName::Codex), 100);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let first_reply = rendered
            .iter()
            .position(|line| line.contains("First answer."))
            .expect("first reply");
        let second_prompt = rendered
            .iter()
            .position(|line| line.contains("second prompt"))
            .expect("second prompt");
        let second_reply = rendered
            .iter()
            .position(|line| line.contains("Second answer."))
            .expect("second reply");
        let cards = rendered
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| line.contains("Agent task completed").then_some(idx))
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 2, "expected two activity cards: {rendered:#?}");
        let first_card = cards[0];
        let second_card = cards[1];

        assert_eq!(
            first_card,
            first_reply + 2,
            "first card should follow first reply with one blank: {rendered:#?}"
        );
        assert!(line_is_blank(lines.get(first_reply + 1)));
        assert_eq!(
            second_card,
            second_reply + 2,
            "second card should follow second reply with one blank: {rendered:#?}"
        );
        assert!(line_is_blank(lines.get(second_reply + 1)));
        assert!(
            first_card < second_prompt
                && second_prompt < second_reply
                && second_reply < second_card,
            "activity cards must stay in turn order: {rendered:#?}"
        );
    }

    #[test]
    fn finalized_history_lines_carry_no_theme_background() {
        // Bug 3a: scrollback content must render on the terminal's native
        // background. The Codex theme's `surface` / `surface_alt` and the user
        // message's `diff_context_bg` would otherwise paint solid "brown blocks"
        // into the terminal's real scrollback. Every finalized line/span must
        // have `bg == None` so `insert_history` emits the default background.
        let turn_id = TurnId::new();
        let session_id = SessionKey("local:test".into());
        let mut app = chat_app(vec![
            Message::user("a user message"),
            Message::assistant("an assistant reply\nwith two lines"),
        ]);
        app.turn_activity_logs.push(TurnActivityLog {
            session_id,
            turn_id: turn_id.clone(),
            request: Some("a user message".into()),
            anchor_index: Some(0),
            items: vec![
                ActivityItem::new(ActivityKind::Tool, "shell", "complete")
                    .with_turn(turn_id)
                    .with_detail("cargo test")
                    .with_output_preview("tests passed")
                    .with_success(true),
            ],
        });
        // Use a theme with a non-default (brownish) surface, the regression case.
        let palette = Palette::for_theme(ThemeName::Codex);
        let lines = finalized_history_lines(&app, palette, 80);
        assert!(!lines.is_empty(), "expected finalized history lines");
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("");
        assert!(
            text.contains("Agent task completed"),
            "activity log must be part of finalized history: {text:?}"
        );
        for line in &lines {
            assert_eq!(
                line.style.bg, None,
                "finalized line carries a theme bg (brown block): {line:?}"
            );
            for span in &line.spans {
                assert_eq!(
                    span.style.bg, None,
                    "finalized span carries a theme bg (brown block): {span:?}"
                );
            }
        }
    }

    #[test]
    fn live_ui_height_is_bounded_below_screen_height() {
        // Even with a huge live tail, the inline viewport must leave scrollback
        // visible above it (so the user can always select/scroll prior output).
        let mut app = chat_app(vec![Message::user("hi")]);
        app.pending_messages = (0..50).map(|i| format!("queued {i}")).collect();
        let height = 30;
        let h = super::super::live_ui_height(&app, 100, height);
        assert!(
            h <= height.saturating_sub(super::super::LIVE_VIEWPORT_MIN_SCROLLBACK),
            "live UI height {h} must leave >= {} rows of scrollback on a {height}-row screen",
            super::super::LIVE_VIEWPORT_MIN_SCROLLBACK
        );
        assert!(h >= 1);
    }

    #[test]
    fn wants_fullscreen_overlay_tracks_inspector_and_modals() {
        let mut app = chat_app(vec![Message::user("hi")]);
        assert!(
            !super::super::wants_fullscreen_overlay(&app),
            "plain chat should use the inline viewport, not alt-screen"
        );
        app.focus = FocusPane::Workspace;
        assert!(
            super::super::wants_fullscreen_overlay(&app),
            "inspector panes should use the full-screen overlay"
        );
        app.focus = FocusPane::Composer;
        app.task_output.active = true;
        assert!(
            super::super::wants_fullscreen_overlay(&app),
            "an active detail modal should use the full-screen overlay"
        );
    }

    #[test]
    fn committed_fingerprint_changes_on_append_and_session_switch() {
        let app1 = chat_app(vec![Message::user("hi")]);
        let fp1 = committed_messages_fingerprint(&app1);
        let app2 = chat_app(vec![Message::user("hi"), Message::assistant("yo")]);
        let fp2 = committed_messages_fingerprint(&app2);
        assert_ne!(fp1, fp2, "appending a message must change the fingerprint");
        assert_eq!(fp1.session_id, fp2.session_id);
        assert_eq!(fp2.message_count, 2);
    }

    // ===== scrollback scar mitigation (specs/task-scrollback-scar.spec) =====

    fn active_turn_app(reply: &str) -> AppState {
        let turn_id = TurnId::new();
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go")],
                tasks: vec![],
                live_reply: Some(crate::model::LiveReply {
                    turn_id,
                    text: reply.into(),
                }),
            }],
            0,
            "Thinking".into(),
            None,
            false,
        );
        app.set_run_state_in_progress();
        app
    }

    #[test]
    fn status_shows_thinking_while_reasoning_then_working_once_answering() {
        // User ask: the status must read "Thinking" while the octopus swims
        // (reasoning started, no answer yet) and "Working" once the answer or
        // a tool begins — the octopus and the label share one predicate.
        // Reasoning phase: empty live_reply.text + non-empty live_reasoning.
        let mut thinking = active_turn_app("");
        let (sid, tid) = thinking
            .active_turn()
            .map(|(s, t)| (s.clone(), t.clone()))
            .unwrap();
        thinking
            .live_reasoning
            .insert((sid, tid), "reasoning about the request".to_string());
        thinking.status = "ready".into(); // neutral status message
        assert!(active_turn_is_thinking(&thinking), "predicate must be true");
        let palette = Palette::for_theme(ThemeName::Codex);
        let rows = rendered_rows(&rendered_buffer(&thinking, palette));
        let status = row_containing(&rows, " state ");
        assert!(
            status.contains("Thinking"),
            "status bar must read Thinking: {status:?}"
        );
        assert!(
            !status.contains("Working"),
            "status bar not Working while thinking: {status:?}"
        );

        // Answer streaming: live_reply.text non-empty -> Working.
        let mut answering = active_turn_app("here is the answer");
        let (sid, tid) = answering
            .active_turn()
            .map(|(s, t)| (s.clone(), t.clone()))
            .unwrap();
        answering
            .live_reasoning
            .insert((sid, tid), "reasoning about the request".to_string());
        answering.status = "ready".into();
        assert!(!active_turn_is_thinking(&answering));
        let rows = rendered_rows(&rendered_buffer(&answering, palette));
        let status = row_containing(&rows, " state ");
        assert!(
            status.contains("Working"),
            "status bar must read Working: {status:?}"
        );
        assert!(
            !status.contains("Thinking"),
            "status bar not Thinking while answering: {status:?}"
        );

        // In progress but NO reasoning yet (e.g. straight to tools) -> Working.
        let mut no_reason = active_turn_app("");
        no_reason.status = "ready".into();
        assert!(!active_turn_is_thinking(&no_reason));
        let rows = rendered_rows(&rendered_buffer(&no_reason, palette));
        let status = row_containing(&rows, " state ");
        assert!(
            status.contains("Working"),
            "no reasoning -> status bar Working: {status:?}"
        );

        // Reasoning present but run_state is Error -> the Error label is not
        // masked by Thinking (codex P2).
        let mut errored = active_turn_app("");
        let (sid, tid) = errored
            .active_turn()
            .map(|(s, t)| (s.clone(), t.clone()))
            .unwrap();
        errored
            .live_reasoning
            .insert((sid, tid), "reasoning".to_string());
        errored.status = "ready".into();
        errored.run_state = SessionRunState::Error {
            message: "boom".into(),
        };
        let rows = rendered_rows(&rendered_buffer(&errored, palette));
        let status = row_containing(&rows, " state ");
        assert!(
            !status.contains("Thinking"),
            "an Error state must not be masked by Thinking: {status:?}"
        );
    }

    #[test]
    fn live_reasoning_before_answer_renders_swimming_octopus_without_text() {
        // Codex-style live render: with non-empty live_reasoning and NO answer
        // streamed yet, push_turn_flow surfaces a single dimmed swimming-octopus
        // indicator (no "thinking" label) and NEVER the verbose reasoning prose.
        const VERBOSE: &str =
            "Let me carefully reason step by step about the user's request in great detail";
        // Empty live_reply.text => the answer has not started streaming yet.
        let app = active_turn_app("");
        let (session_id, turn_id) = app
            .active_turn()
            .map(|(sid, tid)| (sid.clone(), tid.clone()))
            .expect("active turn present (live_reply is Some)");
        let mut app = app;
        app.live_reasoning
            .insert((session_id, turn_id), VERBOSE.to_string());

        let palette = Palette::for_theme(ThemeName::Slate);
        let session = app.active_session().expect("active session").clone();
        let mut lines = Vec::new();
        push_turn_flow(&mut lines, palette, &app, &session, 80, None);
        let rendered = lines_text(&lines);

        assert!(
            OCTOPUS_SWIM_FRAMES
                .iter()
                .any(|frame| rendered.contains(frame)),
            "the indicator should show the swimming octopus; got: {rendered:?}"
        );
        // The octopus alone signals the thinking phase — no "thinking" label.
        assert!(
            !rendered.to_lowercase().contains("thinking"),
            "the indicator must carry no `thinking` text; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose live reasoning text must NOT be rendered; got: {rendered:?}"
        );
    }

    #[test]
    fn live_reasoning_after_answer_started_renders_neither() {
        // Once the answer has begun streaming (live_reply.text non-empty), the
        // codex-style live render drops the thinking indicator too (and never
        // shows the verbose reasoning).
        const VERBOSE: &str = "internal chain of thought that should stay hidden";
        let app = active_turn_app("the answer has begun");
        let (session_id, turn_id) = app
            .active_turn()
            .map(|(sid, tid)| (sid.clone(), tid.clone()))
            .expect("active turn present");
        let mut app = app;
        app.live_reasoning
            .insert((session_id, turn_id), VERBOSE.to_string());

        let palette = Palette::for_theme(ThemeName::Slate);
        let session = app.active_session().expect("active session").clone();
        let mut lines = Vec::new();
        push_turn_flow(&mut lines, palette, &app, &session, 80, None);
        let rendered = lines_text(&lines);

        assert!(
            !OCTOPUS_SWIM_FRAMES
                .iter()
                .any(|frame| rendered.contains(frame)),
            "the swimming-octopus indicator must drop once the answer streams; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose live reasoning text must NOT be rendered; got: {rendered:?}"
        );
        assert!(
            rendered.contains("the answer has begun"),
            "the streamed answer should still render; got: {rendered:?}"
        );
    }

    #[test]
    fn committed_assistant_reasoning_content_is_not_rendered_in_scrollback() {
        // Codex-style committed render: a finalized assistant message carrying
        // reasoning_content must NOT spill the verbose reasoning into scrollback.
        const VERBOSE: &str = "Here is my long winded committed reasoning that should never show";
        let mut assistant = Message::assistant("The final answer.");
        assistant.reasoning_content = Some(VERBOSE.to_string());

        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("go"), assistant],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "Idle".into(),
            None,
            false,
        );

        let palette = Palette::for_theme(ThemeName::Slate);
        let lines = finalized_history_lines_range(&app, palette, 80, 0);
        let rendered = lines_text(&lines);

        assert!(
            rendered.contains("The final answer."),
            "the committed answer should still render; got: {rendered:?}"
        );
        assert!(
            !rendered.contains(VERBOSE),
            "verbose committed reasoning must NOT appear in scrollback; got: {rendered:?}"
        );
    }

    #[test]
    fn live_tail_trims_trailing_blank_rows() {
        // Direct unit: trailing blanks popped, interior blanks kept.
        let mut lines = vec![
            Line::from("a"),
            Line::from(""),
            Line::from("b"),
            Line::from("   "),
            Line::from(""),
        ];
        trim_trailing_blank_lines(&mut lines);
        assert_eq!(lines.len(), 3);
        assert!(
            !line_is_blank(lines.last()),
            "tail must end on real content"
        );

        // End-to-end: the live-tail builder never returns a trailing blank.
        let app = active_turn_app("a streamed answer line");
        let tail =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        assert!(!tail.is_empty());
        assert!(
            !line_is_blank(tail.last()),
            "live tail must not end on a spacer row (scar source)"
        );
    }

    #[test]
    fn collapse_blank_runs_reduces_multi_blank_gaps_to_one() {
        // The reported bug: concatenated block builders stack into 5-6 blank
        // gaps. A run of any length collapses to a single blank; single blanks,
        // content, and order are untouched. Mixed plain + styled (whitespace
        // span) blanks count the same.
        let mut lines = vec![
            Line::from("user"),
            Line::from(""),
            Line::from("   "), // styled-ish blank (whitespace)
            Line::from(""),
            Line::from(""),
            Line::from(""), // 5-blank run (the "6-blank user→reply" shape)
            Line::from("• reply"),
            Line::from(""), // a lone interior blank — must survive
            Line::from("more"),
        ];
        collapse_blank_runs(&mut lines);

        let rendered: Vec<String> = lines
            .iter()
            .map(|l| {
                if line_is_blank(Some(l)) {
                    "<blank>".to_string()
                } else {
                    l.spans.iter().map(|s| s.content.as_ref()).collect()
                }
            })
            .collect();
        assert_eq!(
            rendered,
            vec!["user", "<blank>", "• reply", "<blank>", "more"],
            "every blank run collapses to exactly one; content + order preserved"
        );
    }

    #[test]
    fn collapse_blank_runs_seeded_closes_cross_flush_seam() {
        // Reply text streams to scrollback across many small flushes. Flush 1
        // ends on its trailing blank separator; flush 2 opens on a blank. Per
        // flush each is fine, but at the seam they stack to a 2-line gap — the
        // exact mini5-observed bug. Seeding flush 2 with "prev ended blank"
        // drops its leading blank.
        let mut flush1 = vec![Line::from("paragraph one"), Line::from("")];
        let ends_blank = collapse_blank_runs_seeded(&mut flush1, false);
        assert!(ends_blank, "flush 1 ends on a blank separator");

        let mut flush2 = vec![Line::from(""), Line::from("paragraph two")];
        let ends_blank2 = collapse_blank_runs_seeded(&mut flush2, ends_blank);
        let rendered: Vec<String> = flush2
            .iter()
            .map(|l| {
                if line_is_blank(Some(l)) {
                    "<blank>".to_string()
                } else {
                    l.spans.iter().map(|s| s.content.as_ref()).collect()
                }
            })
            .collect();
        assert_eq!(
            rendered,
            vec!["paragraph two"],
            "seam blank dropped: scrollback shows one blank between the chunks, not two"
        );
        assert!(!ends_blank2, "flush 2 ends on content");

        // An all-blank batch after a blank collapses to nothing and leaves the
        // seam state blank (the separator already in scrollback stands).
        let mut flush3 = vec![Line::from(""), Line::from("  ")];
        assert!(collapse_blank_runs_seeded(&mut flush3, true));
        assert!(flush3.is_empty(), "redundant blanks after a blank all drop");
    }

    #[test]
    fn orphan_guard_drops_only_multi_line_leading_blank_runs() {
        let mut orphaned = vec![
            Line::from(""),
            Line::from(" "),
            Line::from(""),
            Line::from("▌ next prompt"),
        ];
        let ends_blank = collapse_blank_runs_seeded_orphan_guard(&mut orphaned, false, true);
        let rendered = line_texts(&orphaned);

        assert_eq!(
            rendered,
            vec!["▌ next prompt"],
            "a live-tail shrink must not carry an orphaned guardian blank run into scrollback"
        );
        assert!(!ends_blank);

        let mut legitimate_separator = vec![Line::from(""), Line::from("▌ next prompt")];
        collapse_blank_runs_seeded_orphan_guard(&mut legitimate_separator, false, true);
        assert_eq!(
            line_texts(&legitimate_separator),
            vec!["", "▌ next prompt"],
            "a single separator between distinct turns must survive"
        );
    }

    #[test]
    fn collapse_blank_runs_is_idempotent_and_preserves_edges() {
        // Already-collapsed input is unchanged (idempotent), and a single
        // leading/trailing blank is kept (collapse only removes *excess*).
        let mut lines = vec![
            Line::from(""),
            Line::from("a"),
            Line::from(""),
            Line::from("b"),
            Line::from(""),
        ];
        let before = lines.len();
        collapse_blank_runs(&mut lines);
        assert_eq!(lines.len(), before, "no runs to collapse → unchanged");
        collapse_blank_runs(&mut lines);
        assert_eq!(lines.len(), before, "idempotent on a second pass");
    }

    #[test]
    fn tail_height_cap_scales_with_terminal() {
        // Blank-separated paragraphs (each its own block) so the content is a
        // tall stack of rows that overruns any cap — not one wrapped paragraph.
        let huge = (1..=80)
            .map(|i| format!("para {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let app = active_turn_app(&huge);
        let tall = live_tail_height_with_finalization(&app, 80, 50, None);
        let short = live_tail_height_with_finalization(&app, 80, 24, None);
        assert!(
            tall <= 25,
            "tall cap must not exceed half the terminal: {tall}"
        );
        assert_ne!(tall, 18, "cap must no longer be the fixed 18");
        assert!(
            tall > short,
            "the cap scales with terminal height: {tall} vs {short}"
        );
    }

    #[test]
    fn long_stream_stays_half_capped_even_with_a_btw_aside_open() {
        // Regression (codex P2 on the aside-height fix): raising the tail cap for
        // a `/btw` aside must lift ONLY the aside's own reservation — a long
        // in-flight stream behind a short aside must still be half-capped so it
        // can't grow the viewport and displace scrollback.
        let huge = (1..=80)
            .map(|i| format!("para {i}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let mut app = active_turn_app(&huge);
        let session_id = app.active_session().expect("active session").id.clone();
        // A short aside (just "Answering…"), far smaller than half the screen.
        app.set_btw_answering(&session_id, "quick q".into());
        let tail = live_tail_height_with_finalization(&app, 80, 50, None);
        assert!(
            tail <= 25,
            "a long stream must stay half-capped despite an open aside: {tail}"
        );
    }

    #[test]
    fn live_ui_height_matches_rendered_tail() {
        // The height path must reflect exactly the builder the render path uses:
        // live_tail_height == capped visual rows of live_tail_lines (same source
        // render reads), so there is no off-by blank gap between them.
        let app = active_turn_app("one short answer line");
        let (w, h) = (80u16, 40u16);
        let wrap = usize::from(w.saturating_sub(2)).max(1);
        let lines = live_tail_lines_with_finalization(
            &app,
            Palette::for_theme(ThemeName::Slate),
            wrap,
            None,
        );
        let raw_rows = transcript_visual_rows(&lines, wrap) as u16;
        let cap = (h / 2).max(3);
        let expected = raw_rows.min(cap);
        assert_eq!(
            live_tail_height_with_finalization(&app, w, h, None),
            expected,
            "height path must equal capped rows of the shared live-tail builder"
        );
    }

    #[test]
    fn settled_turn_leaves_bounded_blank_rows() {
        // Active turn → settle: once idle with no live reply, the live tail is
        // empty (no trailing blanks carried over), so the viewport collapses to
        // chrome and no fresh blank rows are emitted.
        let mut app = active_turn_app("answer body");
        let _ =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        // Settle the turn.
        app.set_run_state_idle();
        app.sessions[0].live_reply = None;
        app.sessions[0]
            .messages
            .push(Message::assistant("answer body"));

        let tail =
            live_tail_lines_with_finalization(&app, Palette::for_theme(ThemeName::Slate), 80, None);
        assert!(
            tail.iter().all(|line| line_is_blank(Some(line))) || tail.is_empty(),
            "a settled turn must not strand content-bearing tail rows: {}",
            lines_text(&tail)
        );
        assert!(
            !tail.last().is_some_and(|line| line_is_blank(Some(line))),
            "and never a trailing blank row"
        );
    }

    #[test]
    fn committed_history_stays_in_scrollback() {
        // Non-pager inline render must not repaint committed history into the
        // viewport (it lives in native scrollback) — the invariant the scar
        // mitigation must not regress.
        let app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:scar".into()),
                title: "scar".into(),
                profile_id: Some("coding".into()),
                messages: vec![
                    Message::user("earlier question"),
                    Message::assistant("COMMITTED_HISTORY_MARKER reply"),
                ],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        assert!(!app.transcript_pager_active);
        let rows = viewport_rows(&app, 80, 24);
        assert!(
            !rows
                .iter()
                .any(|row| row.contains("COMMITTED_HISTORY_MARKER")),
            "committed history must stay in scrollback, not the inline viewport: {rows:#?}"
        );
    }

    // ===== markdown link/strikethrough edge cases (issue #207) =====

    #[test]
    fn markdown_link_and_strike_parsers_validate_content() {
        // Well-formed link: returns (text, url, bytes consumed incl. delimiters).
        assert_eq!(parse_markdown_link("[a](b)rest"), Some(("a", "b", 6)));
        // Empty text or url → not a link (fall through to plain text).
        assert_eq!(parse_markdown_link("[](b)"), None);
        assert_eq!(parse_markdown_link("[a]()"), None);
        assert_eq!(parse_markdown_link("plain"), None);
        // Strikethrough requires non-whitespace content.
        assert_eq!(parse_markdown_strikethrough("~~x~~y"), Some(("x", 5)));
        assert_eq!(parse_markdown_strikethrough("~~~~"), None);
        assert_eq!(parse_markdown_strikethrough("~~  ~~"), None);
    }

    #[test]
    fn degenerate_strikethrough_keeps_literal_tildes() {
        let style = Style::default();
        // `~~~~` and `~~ ~~` have no real content: the markers must NOT be eaten
        // — the literal tildes survive and nothing is struck through.
        for input in ["~~~~", "~~ ~~"] {
            let spans = inline_markdown_spans(input, style, style, style);
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert_eq!(text, input, "degenerate `{input}` must render literally");
            assert!(
                spans
                    .iter()
                    .all(|s| !s.style.add_modifier.contains(Modifier::CROSSED_OUT)),
                "degenerate `{input}` must produce no struck span"
            );
        }
        // A real strikethrough still renders struck.
        let spans = inline_markdown_spans("~~gone~~", style, style, style);
        assert!(
            spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::CROSSED_OUT)
                    && s.content.as_ref() == "gone"),
            "a non-empty strikethrough must still be struck"
        );
    }

    #[test]
    fn table_cell_width_matches_rendered_link() {
        // The width path (`plain_inline_markdown`) must measure the RENDERED
        // link form, not the raw `[text](url)` markup, or a link in a table
        // cell mis-sizes its column (issue #207).
        assert_eq!(
            plain_inline_markdown("[Octos](https://octos.dev)"),
            "Octos (https://octos.dev)"
        );
        // When the text already IS the url it collapses to a single url — the
        // measured width must collapse the same way (was measuring `[url](url)`).
        assert_eq!(
            plain_inline_markdown("[https://octos.dev](https://octos.dev)"),
            "https://octos.dev"
        );

        // Measured text equals the concatenated rendered span text — same parser
        // drives both, so they cannot drift.
        let style = Style::default();
        for input in [
            "see [Octos](https://octos.dev) here",
            "[https://octos.dev](https://octos.dev)",
            "a ~~struck~~ b",
        ] {
            let rendered: String = inline_markdown_spans(input, style, style, style)
                .iter()
                .map(|s| s.content.as_ref())
                .collect();
            assert_eq!(
                plain_inline_markdown(input),
                rendered,
                "width measurement must equal rendered text for `{input}`"
            );
        }
    }

    // ===== composer multi-line (specs/task-composer-multiline.spec) =====

    #[test]
    fn composer_height_grows_with_newlines() {
        // The composer box must reserve more rows as newlines are added, so a
        // multi-line draft is fully visible instead of being clipped.
        let mut app = AppState::new(vec![], 0, "ready".into(), None, false);
        app.composer = "one".into();
        let single = composer_height_for_size(&app, 80, 40);
        app.composer = "one\ntwo\nthree".into();
        let multi = composer_height_for_size(&app, 80, 40);
        assert!(
            multi > single,
            "composer height must grow with newlines: {multi} vs {single}"
        );
    }

    #[test]
    fn multiline_composer_not_capped_in_inline_viewport() {
        // Regression: the inline render derived the composer row cap from the
        // small viewport-region height (flooring at 3 rows), so a 6-line draft
        // dropped its earliest lines. The cap must come from the FULL terminal
        // height — the same basis `live_ui_height` reserved against — so every
        // line stays visible.
        let mut app = AppState::new(
            vec![SessionView {
                id: SessionKey("local:composer".into()),
                title: "composer".into(),
                profile_id: Some("coding".into()),
                messages: vec![Message::user("hi")],
                tasks: vec![],
                live_reply: None,
            }],
            0,
            "ready".into(),
            None,
            false,
        );
        app.focus = crate::model::FocusPane::Composer;
        app.composer = "L1\nL2\nL3\nL4\nL5\nL6".into();
        let rows = viewport_rows(&app, 80, 40);
        let joined = rows.join("\n");
        for marker in ["L1", "L6"] {
            assert!(
                joined.contains(marker),
                "composer line {marker} must stay visible (not capped); rows: {rows:#?}"
            );
        }
    }
}
mod running_row_regression {
    use super::super::*;
    use super::*;
    use crate::model::*;
    use crate::store::Store;
    use octos_core::app_ui::AppUiEvent;
    use octos_core::ui_protocol::*;

    #[test]
    fn running_bash_tool_started_renders_cc_card_not_legacy_verb_row() {
        let session_id = SessionKey("local:t".into());
        let mut store = Store {
            state: AppState::new(
                vec![SessionView {
                    id: session_id.clone(),
                    title: "t".into(),
                    profile_id: Some("dev".into()),
                    messages: vec![Message::user("go")],
                    tasks: vec![],
                    live_reply: None,
                }],
                0,
                "ready".into(),
                None,
                false,
            ),
        };
        let turn_id = TurnId::new();
        store.apply_event(AppUiEvent::Protocol(UiNotification::TurnStarted(
            TurnStartedEvent {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                timestamp: chrono::Utc::now(),
                topic: None,
            },
        )));
        store.apply_event(AppUiEvent::Protocol(UiNotification::ToolStarted(
            ToolStartedEvent {
                session_id: session_id.clone(),
                topic: None,
                turn_id,
                tool_call_id: "c1".into(),
                tool_name: "bash".into(),
                arguments: Some(serde_json::json!({
                    "cmd": "sleep 20 && echo never-finishes",
                    "timeout_ms": 30000
                })),
            },
        )));
        let palette = Palette::for_theme(crate::cli::ThemeName::Codex);
        let backend = ratatui::backend::TestBackend::new(140, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("t");
        terminal
            .draw(|frame| render(frame, &store.state, palette))
            .expect("render");
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            text.push('\n');
        }
        assert!(!text.contains("Using bash"), "old verb leaked:\n{text}");
        assert!(!text.contains("{\"cmd\""), "raw JSON leaked:\n{text}");
    }
}
