spec: task
name: "Activity navigator recent changes"
inherits: project
tags: [tui, activity, diff, navigator]
depends: [task-activity-navigator-overlay, task-inline-diff-livediff-polish]
estimate: 0.5d
---

## Intent

Borrow Livediff's "recent changes" scan pattern without adopting its file
watcher architecture. AppUI already emits file mutation progress rows; `/activity`
should surface those rows as first-class change entries instead of generic
progress lines.

## Decisions

- Use only existing `ActivityItem` truth (`file_mutation` progress rows and
  their details/status). Do not add filesystem watching.
- Add a `change` navigator row kind with compact path title and file type badge
  in the subtitle/detail.
- Add change count to the activity navigator toolbar.
- Preserve search/filter behavior: change rows should match path, operation,
  badge, and preview-ready text.
- Let direct typing in `/activity` start search. `/` still explicitly enters
  search mode, but users should not have to remember it.
- Search is case-insensitive. Empty results must name the query and filter so a
  user can tell whether search or filtering removed the rows.
- Include committed session messages as `message` rows. Users expect `/activity`
  search to find topics discussed in the current session, not only backend task
  activity.
- Do not include `system` messages in `/activity`; the navigator must preserve
  the same hidden-system-content invariant as the transcript renderer.
- Reset the activity query and filter to empty/`all` whenever `/activity` opens,
  so stale search state does not make a fresh overlay look broken.

## Completion Conditions

Scenario: File mutations become change rows
  Test: activity_navigator_file_mutations_render_as_recent_changes
  Given a file mutation activity item
  When the activity navigator model is built
  Then the mutation is represented as a `change` row with file badge,
       operation, path, and diff preview readiness.

Scenario: Toolbar counts change rows
  Test: activity_navigator_toolbar_counts_recent_changes
  Given a rendered activity navigator overlay with file mutation activity
  When the toolbar renders
  Then it includes a `changes N` metric.

Scenario: Direct typing searches
  Test: activity_navigator_typing_starts_search_and_filters
  Given an open activity navigator
  When the user types text without pressing `/`
  Then the query is populated and matching rows are filtered.

Scenario: Search is case-insensitive
  Test: activity_navigator_search_is_case_insensitive
  Given an activity navigator containing a `RUST` change row
  When the user searches for `RUST`
  Then the row still matches even though matching normalizes case.

Scenario: Empty search explains why
  Test: activity_navigator_empty_state_names_query_and_filter
  Given a query that matches no rows
  When the overlay renders
  Then the empty state includes both the query and active filter.

Scenario: Session messages are searchable
  Test: activity_navigator_search_matches_session_messages
  Given a session message that mentions `Rust`
  When the user searches for `rust` in `/activity`
  Then a `message` row is returned.

Scenario: System messages stay hidden
  Test: activity_navigator_search_omits_system_messages
  Given a session contains a system message
  When the user searches for text from that message in `/activity`
  Then no row exposes or indexes the system content.

Scenario: Opening resets stale search state
  Test: activity_navigator_open_resets_query_and_filter_to_all
  Given the prior activity navigator query was non-empty and filter was `done`
  When `/activity` opens again
  Then the query is empty and the filter is reset to `all`.

## Non-goals

- No file watcher or ignore engine.
- No line-count extraction beyond data already available in AppUI activity rows.
- No full-screen split-pane Livediff clone.
