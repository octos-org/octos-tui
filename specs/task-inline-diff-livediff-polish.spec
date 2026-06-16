spec: task
name: "Inline diff preview Livediff polish"
inherits: project
tags: [tui, rendering, diff, activity]
depends: [task-transcript-diff-code-highlight]
estimate: 0.5d
---

## Intent

Livediff's most transferable idea is not its file watcher architecture, but the
way it makes a diff scannable: a compact file badge, visible `+/-` counts,
strong file/hunk headers, and selection that lands on the first real change.
Apply those local rendering patterns to octos-tui's inline AppUI diff preview.

## Decisions

- Keep AppUI as the source of truth. Do not add file watching, ignore engines,
  or a Livediff-style split pane.
- Reuse the existing `DiffPreviewPaneState` selected file/hunk state.
- Render the selected file/hunk in the inline preview rather than always the
  first file/hunk.
- Prefer the first hunk containing an added/removed line when a diff result is
  applied. Metadata-only hunks remain visible but are not the default target
  when a real code change exists.
- Use simple ASCII file type badges (`RUST`, `TOML`, `JSON`, `FILE`, ...);
  avoid Nerd Font dependency.

## Completion Conditions

Scenario: Inline file header is scan-friendly
  Test: render_inline_diff_header_shows_file_badge_and_counts
  Given an inline diff preview for a Rust file
  When the transcript renders
  Then the file header includes a file type badge, status, additions/deletions,
       and path.

Scenario: Selected hunk is rendered
  Test: render_inline_diff_shows_selected_hunk_not_always_first
  Given a diff preview with multiple hunks
  When selected hunk is the second hunk
  Then the inline preview shows the second hunk and hides the first hunk body.

Scenario: Result defaults to first real change
  Test: diff_preview_result_selects_first_changed_hunk
  Given a diff preview whose first hunk is metadata/context-only
  When the result is applied
  Then the selected hunk is the first hunk with added/removed lines.

## Non-goals

- No character-level intra-line diff.
- No live file watcher or ignore management.
- No full-screen split-pane diff reviewer.
