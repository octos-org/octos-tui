# M9.31 Context Attachment UPCR

Status: proposed, not part of AppUi/UI Protocol 1.0.

## Problem

`octoscode` can render and select local context such as a diff hunk, file path,
selected text, or review comment. Today the only AppUi v1 turn input is text, so
the TUI must stage selected context as plain prompt text. That is safe for v1,
but it loses structured context semantics that a backend could use for token
budgeting, attribution, approval, and replay.

## Proposed Protocol Addition

Add optional structured attachments to `turn/start`:

```json
{
  "session_id": "coding:local:example",
  "turn_id": "turn_123",
  "input": [{ "type": "text", "text": "Review this hunk." }],
  "attachments": [
    {
      "type": "diff_hunk",
      "path": "src/lib.rs",
      "old_path": null,
      "file_status": "modified",
      "hunk_header": "@@ -10,7 +10,9 @@",
      "lines": [
        { "kind": "removed", "old_line": 12, "new_line": null, "content": "old" },
        { "kind": "added", "old_line": null, "new_line": 12, "content": "new" }
      ]
    }
  ]
}
```

Attachment types:

| Type | Required Fields | Purpose |
|---|---|---|
| `diff_hunk` | `path`, `hunk_header`, `lines` | Carry selected patch context. |
| `file_path` | `path` | Point the agent at a file without pasting content. |
| `text_selection` | `path`, `start_line`, `end_line`, `text` | Carry a selected range. |
| `review_comment` | `path`, `line`, `text` | Attach human review feedback. |

## Compatibility Rules

- Existing AppUi 1.0 clients and servers ignore this because it is not emitted
  yet.
- A future protocol version must advertise attachment support in the session
  snapshot or capability handshake.
- Servers must reject unsupported attachment types with a typed error, not
  silently reinterpret them.
- Attachments must be included in transcript persistence or explicitly marked
  ephemeral so replay behavior is deterministic.

## Current M9.30 Bridge

Until this UPCR is accepted, `octoscode` uses a v1-compatible bridge:

- `[` and `]` select the previous/next rendered diff hunk.
- `c` stages the selected hunk as plain text in the composer or pending-message
  queue.
- No AppUi/UI Protocol wire shape changes are made.
