# Coding UX Prompt Contract

`octos-tui` must render AppUI events defensively, but some coding UX quality
depends on the agent emitting predictable human-facing text. This contract is
intended for the Octos server profile, coding harness, or agent system prompt.
It should not be injected by the TUI client.

## Output Shape

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

## Ownership Boundary

The TUI is responsible for robust rendering, slash commands, activity cards,
approval prompts, and AppUI state. The server/harness prompt is responsible for
encouraging clean plan and summary text from the model. Protocol correctness
must never depend on the model following this prompt.
