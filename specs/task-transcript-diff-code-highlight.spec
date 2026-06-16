spec: task
name: "Transcript diff code block semantic highlight"
inherits: project
tags: [tui, rendering, transcript, diff]
depends: [task-code-syntax-highlight]
estimate: 0.5d
---

## Intent

Assistant replies often include git/unified diffs as fenced blocks. When those
blocks fall back to generic `code` rendering, `+`, `-`, and `@@` lines become a
muted wall of text and the user cannot scan the actual change. Inspired by
Livediff's TUI diff view, render transcript diff blocks with semantic line
styles: added lines green, removed lines red, hunk headers accent, file headers
bold.

## Decisions

- Do not import Livediff dependencies or architecture.
- Reuse existing octos-tui diff palette (`success`, `danger`, `diff_context_bg`)
  so inline protocol diff previews and transcript diff snippets read the same.
- Recognize both explicit diff fences (`diff`, `patch`, `udiff`) and unlabeled
  fences that structurally look like unified diffs (`---/+++/@@` plus `+` or
  `-` lines). This covers model output that omits the language tag.
- Keep regular language fences on the existing syntect path.

## Completion Conditions

Scenario: Explicit diff fences receive semantic colors
  Test: render_diff_code_fence_highlights_added_removed_and_hunks
  Given an assistant message containing a ```diff fenced block
  When the transcript renders
  Then removed content is danger styled, added content is success styled, and
       hunk headers use accent styling.

Scenario: Unlabeled unified diff fences are reclassified
  Test: render_unlabeled_unified_diff_fence_is_reclassified_from_code
  Given an assistant message containing an unlabeled fenced unified diff
  When the transcript renders
  Then the block header shows `diff` and added/removed rows are semantically
       highlighted instead of muted generic code.

## Non-goals

- No character-level intra-line diff.
- No file watching or Livediff-style split-pane UI.
- No changes to AppUI protocol diff preview payloads.
