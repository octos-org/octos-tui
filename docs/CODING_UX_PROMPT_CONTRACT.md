# Coding UX Prompt Contract

`octos-tui` must render AppUI events defensively, but some coding UX quality
depends on the agent emitting predictable human-facing text. This contract is
intended for the Octos server profile, coding harness, or agent system prompt.
It should not be injected by the TUI client.

## Output Shape

Every user turn must produce a human-facing answer. Tool activity, diff previews,
status changes, and file edits are not a substitute for an assistant answer. If
the model fails before producing a final answer, the runtime or client should
surface a structured fallback summary rather than leaving the user with only
activity rows.

When starting implementation work, emit one concise checklist:

```text
Plan:
- [ ] Inspect the failing behavior
- [ ] Patch the production code
- [ ] Run focused tests
- [ ] Summarize files changed and validation
```

Rules:

- Use 3-6 items.
- Keep each item under one terminal line when possible.
- Use `- [ ]` for pending items and `- [x]` for completed items.
- Do not put reasoning, evidence, alternatives, or test output inside a plan
  item.
- Do not repeat the checkbox marker after a number, such as `1. [ ]`.
- For comparison, review, status, file-change, or problem/fix answers, prefer a
  Markdown table over prose.
- For direct user questions, answer the question first, then add a short list or
  table only when it improves scanability.

## Structured Output Purposes

The coding prompt should force predictable shapes for common UX states:

| Purpose | Required Shape | Notes |
| --- | --- | --- |
| Ask for input | `Question` plus `A`, `B`, `C` choices | Each choice states the impact or tradeoff. |
| Status | Table: `Area`, `State`, `Next` | Use for "status", "are you working", and "what remains". |
| Work summary | `Session Summary` bullets | Include files, validation, risks, and next step. |
| Shell/tool use | `Command`, `Purpose`, `Expected result` | Use before non-trivial commands, not every trivial read. |
| File edits | Table: `File`, `Change`, `Validation` | Keep paths exact and concise. |
| Code generation | Planned diff then actual diff summary | Do not paste large diffs unless asked. |
| Background tasks | Checklist with state words | Use `pending`, `running`, `blocked`, or `done`. |
| Review findings | Table: `Finding`, `Impact`, `Fix` | Findings lead; summary is secondary. |

Markdown tables must use real pipe-table syntax:

```markdown
| File | Problem | Fix |
| --- | --- | --- |
| Hero.astro | Orphan frontmatter marker | Removed the bare marker |
```

## While Working

- Keep progress prose short and decision-oriented.
- Prefer a single sentence before tool use only when it helps the user
  understand intent.
- Do not restate raw tool output unless it changes the next action.
- When blocked on approval or input, say exactly what is needed and stop.

## Completion

Finish with:

```text
Session Summary
- Files changed: ...
- Validation: ...
- Risks / follow-up: ...
```

If the plan remains visible in the final answer, mark completed items as
`- [x]`. Do not leave stale unchecked work after reporting success.

If the user asked multiple questions in one turn, the final answer must address
each question explicitly. A successful work turn that only says "done" is a UX
bug.

## Ownership Boundary

The TUI is responsible for robust rendering, slash commands, activity cards,
approval prompts, and AppUI state. The server/harness prompt is responsible for
encouraging clean plan and summary text from the model. Protocol correctness
must never depend on the model following this prompt.

## Client Rendering Requirements

The coding TUI must keep the live work area readable during long turns:

- The work plan is sticky above the composer. It must not also appear as a
  duplicate inline card in the transcript.
- Tool/activity cards, approval cards, live progress, and diff preview cards are
  rendered in the chat flow, not inside the sticky plan pane.
- Live activity for the current turn is anchored directly after the most recent
  user prompt. The transcript must not stack several user prompts first and then
  render a disconnected activity pane later.
- Completed turn activity is captured as transcript-owned state and rendered
  under the matching user request, before the assistant answer for that turn.
- Related tool/progress/file rows are grouped under one `• Agent task ...`
  parent row. Child activity rows are indented with `⎿` on the first row and
  aligned continuation rows so the activity reads as one task, not as detached
  peer messages.
- Assistant prose paragraphs start with `•` for scanability. Markdown tables,
  code blocks, numbered lists, and explicit checklist rows keep their native
  structure.
- Diff previews stay in the transcript below recent activity/progress and above
  the sticky work plan.
- Streaming output must not force-scroll the transcript while the user is
  reading older content. Follow the tail only when the scroll position is
  already at the latest line.
- Plan text rendered from model markdown strips formatting markers such as
  `**bold**` and inline backticks in compact TUI rows.
- If a turn completes or fails without a final assistant message, the TUI must
  insert a structured fallback `Session Summary` so the user sees what happened
  instead of only raw activity.

Composer requirements:

- Terminal paste is a first-class input path. Bracketed paste appends literal
  text to the composer and must not dispatch slash commands or submit partial
  lines while pasting.
- When a terminal or tmux path degrades paste into a rapid key burst,
  newline/Enter events inside that burst are treated as pasted newlines, not as
  send actions.
- Common multi-line paste remains visible inline. Only very large pasted blocks
  may collapse, and collapsed blocks must state that Enter sends the full text.
- The composer height grows with input length, wrapping width, and terminal
  height. It may cap the visible rows to preserve transcript space, but it must
  show the tail of over-budget input so the current cursor position is visible.
- Long single-line prompts wrap into additional composer rows instead of staying
  visually one-line.
- `Ctrl+U` clears composer/pending input without changing active AppUI state.
- Composer editing supports readline-style shortcuts: `Ctrl+A/E/B/F` for
  line/cursor movement, `Ctrl+W` and `Alt+Backspace` for previous-word delete,
  `Alt+B/F` for word movement, `Alt+D` for next-word delete, `Ctrl+D` for
  delete-under-cursor, and `Ctrl+K` for kill-to-line-end.

Testing requirements:

- Unit render tests must cover common multi-line paste, large-paste collapse,
  long-line wrapping, sticky plan non-duplication, markdown marker stripping,
  activity anchoring after the latest user prompt, fallback final summaries, and
  diff/activity ordering.
- Live tmux soaking must include an interactive paste check and capture the
  resulting `tui-capture.txt` so hidden-line regressions are visible.
