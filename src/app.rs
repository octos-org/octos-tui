use ratatui::{
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, LineGauge, List, ListItem, ListState, Paragraph, StatefulWidget,
        Wrap,
    },
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use octos_core::{
    Message, SessionKey, TaskId, ui_protocol::TaskRuntimeState, ui_protocol::TurnId,
    ui_protocol::approval_kinds,
};

use crate::{
    menu::render as menu_render,
    model::{
        ActivityItem, ActivityKind, ActivityNavigatorFilter, AppState, ApprovalModalState,
        ArtifactDetailState, ComposerPresentation, DiffPreviewPaneState, FocusPane,
        GoalObjectiveFold, PlanStep as RenderedPlanStep, SessionAutonomyState, SessionRunState,
        SessionView, TaskOutputDetailState, TaskView, ThreadGraphDetailState, TurnActivityLog,
        TurnPromptAnchor, TurnStateDetailState, UserQuestionEntry, UserQuestionPickerState,
        extract_plan_steps, task_state_label,
    },
    theme::Palette,
    tui_terminal::FrameLike,
};

fn inspector_visible(app: &AppState) -> bool {
    matches!(
        app.focus,
        FocusPane::Sessions
            | FocusPane::Tasks
            | FocusPane::Artifacts
            | FocusPane::Workspace
            | FocusPane::Git
    )
}

/// Modal/overlay surfaces that must own the keyboard and the screen over a
/// sub-agent peek. Mirrors `event_loop::modal_owns_keyboard` (kept in sync): the
/// peek yields BOTH its rendering and its input while one of these is up, so an
/// approval / question / detail modal that arrives mid-peek renders visibly and
/// its keys aren't consumed behind an opaque overlay.
fn peek_yields_to_modal(app: &AppState) -> bool {
    app.activity_navigator.active
        || app
            .approval
            .as_ref()
            .is_some_and(|approval| approval.visible)
        || app
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.visible)
        || app.task_output.active
        || app.artifact_detail.active
        || app.thread_graph_detail.active
        || app.turn_state_detail.active
}

/// True when the main pane is peeking a still-present sub-agent AND no modal is
/// up — the state that swaps the inline chat for the full-screen agent-output
/// overlay and gives that overlay the keyboard. A selection pointing at a
/// vanished agent is NOT active (so the inline composer stays editable), and a
/// modal takes precedence (so it renders and receives keys). The event loop
/// gates the peek's keyboard ownership on this same predicate.
pub fn agent_view_active(app: &AppState) -> bool {
    !peek_yields_to_modal(app)
        && matches!(
            &app.chat_view,
            crate::model::ChatViewTarget::Agent(id) if app.active_agent_record(id).is_some()
        )
}

// ===========================================================================
// Inline-viewport rendering (codex-style scrollback model).
//
// The event loop keeps the live UI (live transcript tail + menus + indicators +
// composer + status) in a small ratatui inline viewport pinned to the bottom of
// the screen, and writes *finalized* transcript history into the terminal's
// normal scrollback (via `insert_history`). The terminal then owns that
// scrollback, so the user can natively mouse-select, wheel-scroll, and copy
// prior output (incl. through tmux) with no app mode key.
//
// `render_viewport` is the live-UI draw; `finalized_history_lines` produces the
// committed-only lines flushed to scrollback. Full-screen overlays (inspector,
// onboarding, modals) fall back to the legacy `render` path under alt-screen —
// see `wants_fullscreen_overlay`.
// ===========================================================================

/// True when the current state needs the legacy full-screen render (alt-screen),
/// rather than the inline-viewport + scrollback chat flow. Mirrors codex using
/// alt-screen only for transient overlays (transcript pager, resume picker).
pub fn wants_fullscreen_overlay(app: &AppState) -> bool {
    app.activity_navigator.active
        || agent_view_active(app)
        || inspector_visible(app)
        || onboarding_first_launch_active(app)
        || app.transcript_pager_active
        || app.task_output.active
        || app.artifact_detail.active
        || app.thread_graph_detail.active
        || app.turn_state_detail.active
}

/// The detail overlays that render full-screen (alt-screen, no native scrollback
/// behind them) and that `scroll_current_surface_*` routes the wheel to. Capture
/// must stay on while one is up so the wheel actually scrolls it: a detail modal
/// opening over a peek flips `agent_view_active` false, and without this the
/// capture would drop even though the modal is a full-screen wheel target.
fn scrollable_detail_modal_active(app: &AppState) -> bool {
    app.task_output.active
        || app.artifact_detail.active
        || app.thread_graph_detail.active
        || app.turn_state_detail.active
}

/// Mouse capture policy. In the default `native` scroll-mode, capture is on
/// ONLY while a full-screen overlay is up — the transcript pager, a sub-agent
/// peek, or a detail modal — so the wheel scrolls that overlay while the inline
/// chat flow keeps native terminal selection/copy untouched (these overlays are
/// alt-screen, with no native scrollback behind them to preserve). In `pinned`
/// scroll-mode the user explicitly trades native selection for a wheel that
/// always scrolls the app (composer pinned), so capture stays on.
pub fn wants_mouse_capture(app: &AppState) -> bool {
    app.transcript_pager_active
        || app.pinned_scroll
        || agent_view_active(app)
        || scrollable_detail_modal_active(app)
}

/// Watermarks for active-turn content that has already been written into native
/// scrollback while the turn is still running. The inline viewport uses this to
/// hide the same stable prefix so spinner ticks only repaint the live tail.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTurnFinalization {
    pub session_id: String,
    pub turn_id: String,
    pub reply_flushed_text: String,
    pub activity_flushed_items: usize,
    pub activity_flushed_keys: Vec<String>,
}

impl LiveTurnFinalization {
    fn new(session_id: &SessionKey, turn_id: &octos_core::ui_protocol::TurnId) -> Self {
        Self {
            session_id: session_id.0.clone(),
            turn_id: turn_id.0.to_string(),
            reply_flushed_text: String::new(),
            activity_flushed_items: 0,
            activity_flushed_keys: Vec::new(),
        }
    }

    pub(crate) fn matches_turn(
        &self,
        session_id: &SessionKey,
        turn_id: &octos_core::ui_protocol::TurnId,
    ) -> bool {
        self.session_id == session_id.0 && self.turn_id == turn_id.0.to_string()
    }

    pub(crate) fn has_flushed_content(&self) -> bool {
        !self.reply_flushed_text.is_empty()
            || self.activity_flushed_items > 0
            || !self.activity_flushed_keys.is_empty()
    }
}

/// Minimum rows of scrollback to keep visible above the inline viewport.
const LIVE_VIEWPORT_MIN_SCROLLBACK: u16 = 4;

/// Build the live-tail lines (everything that is NOT finalized committed
/// history): recent-user context pinned for the active turn, turn-flow
/// (approvals / questions / streaming reply / activity / diff preview), and
/// pending queued messages.
fn active_live_finalization<'a>(
    app: &AppState,
    live_finalization: Option<&'a LiveTurnFinalization>,
) -> Option<&'a LiveTurnFinalization> {
    let (session_id, turn_id) = app.active_turn()?;
    live_finalization.filter(|finalization| finalization.matches_turn(session_id, turn_id))
}

/// Drop blank lines from the end of a line set (a line is blank when every
/// span is whitespace). Interior blanks — paragraph separators — are kept.
fn trim_trailing_blank_lines(lines: &mut Vec<Line<'static>>) {
    while lines.last().is_some_and(|line| line_is_blank(Some(line))) {
        lines.pop();
    }
}

/// Collapse any run of two-or-more consecutive blank lines down to a single
/// blank, keeping the first of each run. The block builders
/// (`push_message_block`, `push_live_reply_block`, `push_formatted_body_marked`,
/// the activity-log/tool-call sections) each guard their *own* leading/trailing
/// separator, but a single flush concatenates several of them into one buffer
/// (committed history + live-turn deltas in `viewport.rs`), so a block that ends
/// in a blank followed by one that opens with a blank sums to a multi-line gap.
/// Applied once at the assembly endpoints, this guarantees at most one blank
/// between blocks regardless of how the pieces were produced. It never tightens
/// a single blank or fuses two non-blank blocks (a run of one stays one), so it
/// can only remove excess vertical space, never introduce it.
pub fn collapse_blank_runs(lines: &mut Vec<Line<'static>>) {
    collapse_blank_runs_seeded(lines, false);
}

/// [`collapse_blank_runs`] that also closes the seam against content already
/// emitted before this batch. `prev_ends_blank` is whether the line immediately
/// preceding these — e.g. the last line already in scrollback from an earlier
/// flush — was blank; when it was, a leading blank here is dropped. Reply text
/// streams to scrollback across many small flushes, so without this a chunk
/// ending on a blank and the next chunk opening on a blank stack into a 2-line
/// gap that per-batch collapse can't see. Returns whether the batch now ends on
/// a blank (feed back as the next call's `prev_ends_blank`).
pub fn collapse_blank_runs_seeded(lines: &mut Vec<Line<'static>>, prev_ends_blank: bool) -> bool {
    let mut prev_blank = prev_ends_blank;
    lines.retain(|line| {
        let blank = line_is_blank(Some(line));
        let keep = !(blank && prev_blank);
        prev_blank = blank;
        keep
    });
    match lines.last() {
        Some(line) => line_is_blank(Some(line)),
        // Batch contributed nothing (all dropped) → seam state is unchanged.
        None => prev_ends_blank,
    }
}

pub fn collapse_blank_runs_seeded_orphan_guard(
    lines: &mut Vec<Line<'static>>,
    prev_ends_blank: bool,
    drop_orphaned_leading_blank_run: bool,
) -> bool {
    if drop_orphaned_leading_blank_run {
        let leading_blank_run = lines
            .iter()
            .take_while(|line| line_is_blank(Some(line)))
            .count();
        if leading_blank_run > 1 {
            lines.drain(0..leading_blank_run);
        }
    }
    collapse_blank_runs_seeded(lines, prev_ends_blank)
}

/// A recorded segment boundary is "word-safe" when it does NOT fall inside a
/// word/token — i.e. not (the char before AND the char at the offset are both
/// word chars). `message/persisted` can sample the live buffer mid-word
/// ("anim|ate"); splitting or flushing there breaks words in immutable
/// scrollback. Boundaries adjacent to a delimiter (whitespace, punctuation, line
/// end, or buffer edge) pass — `ToolStarted` boundaries normally sit after
/// sentence punctuation and pass anyway.
fn boundary_is_word_safe(text: &str, boundary: usize) -> bool {
    if boundary > text.len() || !text.is_char_boundary(boundary) {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let before = text[..boundary].chars().next_back().is_some_and(is_word);
    let after = text[boundary..].chars().next().is_some_and(is_word);
    !(before && after)
}

/// Return the next active-turn watermark by extending the previous one with any
/// newly settled live reply lines and any non-running activity rows.
pub fn next_live_turn_finalization(
    app: &AppState,
    previous: Option<&LiveTurnFinalization>,
) -> Option<LiveTurnFinalization> {
    let (session_id, turn_id) = app.active_turn()?;
    let session = app.active_session()?;
    let mut next = previous
        .filter(|finalization| finalization.matches_turn(session_id, turn_id))
        .cloned()
        .unwrap_or_else(|| LiveTurnFinalization::new(session_id, turn_id));

    if let Some(live_reply) = session
        .live_reply
        .as_ref()
        .filter(|live_reply| &live_reply.turn_id == turn_id)
        && live_reply
            .text
            .starts_with(next.reply_flushed_text.as_str())
    {
        // A completed content segment (the text before a tool call) is stable and
        // flushable even without a trailing blank line. Without this, an agentic
        // turn whose narration segments are glued ("…step 1.step 2:") never
        // advances the blank-line watermark, so the whole growing reply stays in
        // the height-limited live tail and clips to its bottom — the user sees a
        // mid-reply fragment ("intermediate truncated") while the committed render
        // is correct. Flush through the last completed segment boundary so the
        // live tail holds only the in-progress segment.
        let last_completed_segment = app
            .live_reply_segment_boundaries
            .get(&(session_id.clone(), turn_id.clone()))
            .into_iter()
            .flatten()
            .copied()
            .filter(|b| {
                *b <= live_reply.text.len()
                    && live_reply.text.is_char_boundary(*b)
                    && boundary_is_word_safe(&live_reply.text, *b)
            })
            .max()
            .unwrap_or(0);
        // A completed segment is flushable UNLESS it ends inside an unclosed code
        // fence (a tool call mid-```block```), which stable_live_reply_prefix_len
        // deliberately pins behind — never flush an unbalanced fence into immutable
        // scrollback. Plain-text narration segments (the glued case this targets)
        // carry no fence and stay flushable.
        let segment_end = if last_completed_segment > 0
            && live_reply.text[..last_completed_segment]
                .lines()
                .filter(|line| line.trim_start().starts_with("```"))
                .count()
                % 2
                == 0
        {
            last_completed_segment
        } else {
            0
        };
        let stable_end = stable_live_reply_prefix_len(&live_reply.text).max(segment_end);
        if stable_end > next.reply_flushed_text.len() {
            next.reply_flushed_text = live_reply.text[..stable_end].to_string();
        }
    }

    let mut existing_activity = next
        .activity_flushed_keys
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    for (idx, item) in flow_activity_items(app).iter().enumerate() {
        let key = activity_finalization_key(item, idx);
        if !existing_activity.contains(&key) && !is_running_activity(item) {
            existing_activity.insert(key.clone());
            next.activity_flushed_keys.push(key);
        }
    }
    next.activity_flushed_items = next.activity_flushed_keys.len();

    Some(next)
}

/// Largest prefix of the streaming reply that is safe to flush into the
/// IMMUTABLE terminal scrollback (codex's markdown-stream model): the cut may
/// only land on a *completed block* boundary — a closed code fence, or a blank
/// line ending a paragraph/table/list run. Completed blocks are self-contained,
/// so rendering each flushed batch as an independent document stays correct;
/// an unclosed fence or a still-accumulating paragraph is held back (it keeps
/// re-rendering in the live tail) rather than written out half-parsed and
/// frozen wrong forever. The state machine is line-oriented: only lines
/// terminated by `\n` are considered at all.
fn stable_live_reply_prefix_len(text: &str) -> usize {
    let mut safe_end = 0;
    let mut offset = 0;
    let mut in_fence = false;
    let mut fence_start = 0;
    for segment in text.split_inclusive('\n') {
        if !segment.ends_with('\n') {
            // Trailing partial line: never flushable.
            break;
        }
        let line_start = offset;
        offset += segment.len();
        let trimmed = segment.trim();
        if trimmed.starts_with("```") {
            if in_fence {
                // Fence closed → the whole fenced block just completed.
                in_fence = false;
                safe_end = offset;
            } else {
                in_fence = true;
                fence_start = line_start;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.is_empty() {
            // Blank line ends any open paragraph / table / list run.
            safe_end = offset;
        }
    }
    if in_fence {
        // An unclosed fence pins the watermark before the fence opener, even
        // if blank lines were seen inside the fence body.
        safe_end = safe_end.min(fence_start);
    }
    safe_end
}

fn strip_lines_background(lines: &mut [Line<'static>]) {
    for line in lines {
        strip_line_background(line);
    }
}

/// Reset the background of a finalized scrollback line (and every span) to the
/// terminal default, so history written into real scrollback blends with the
/// terminal's native background instead of painting the theme surface. Only the
/// background is cleared; foreground colors and text attributes are preserved.
fn strip_line_background(line: &mut Line<'static>) {
    line.style.bg = None;
    for span in &mut line.spans {
        span.style.bg = None;
    }
}

/// Identity of the committed history flushed to scrollback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommittedFingerprint {
    pub session_id: String,
    pub message_count: usize,
    pub activity_log_count: usize,
    pub content_hash: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatLayoutAreas {
    /// #324: session strip across the top (0-height with a single session).
    pub session_strip: Rect,
    pub transcript: Rect,
    pub menu: Rect,
    pub autonomy: Rect,
    pub harness: Rect,
    /// Parked-decision watchdog banner, directly above the composer (0-height
    /// until a turn has been parked on a decision past the escalation threshold).
    pub decision: Rect,
    pub composer: Rect,
    /// Sub-agent selector strip, directly under the composer (0-height when the
    /// session has no sub-agents).
    pub agent_strip: Rect,
    pub status: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptScrollMetrics {
    pub visible_rows: usize,
    pub total_rows: usize,
    pub scroll_from_bottom: usize,
    pub max_scroll_from_bottom: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarThumb {
    pub top: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintBarMode {
    StatusbarKeys,
    Menu,
    Onboarding,
    Approval,
    UserQuestion,
    PagerKeys,
    PagerReviewing,
    ActivityNavigator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HintBarModel {
    pub mode: HintBarMode,
}

pub fn hint_bar_model(app: &AppState) -> HintBarModel {
    let mode = if app.activity_navigator.active {
        HintBarMode::ActivityNavigator
    } else if app
        .approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
    {
        HintBarMode::Approval
    } else if app
        .user_question
        .as_ref()
        .is_some_and(|question| question.visible)
    {
        HintBarMode::UserQuestion
    } else if onboarding_first_launch_active(app) {
        HintBarMode::Onboarding
    } else if app.menu_stack.is_active() {
        HintBarMode::Menu
    } else if app.transcript_pager_active && app.transcript_scroll > 0 {
        HintBarMode::PagerReviewing
    } else if app.transcript_pager_active {
        HintBarMode::PagerKeys
    } else {
        HintBarMode::StatusbarKeys
    };
    HintBarModel { mode }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityNavigatorRowKind {
    Session,
    Message,
    Orchestration,
    Task,
    FileChange,
    Activity,
    Approval,
}

impl ActivityNavigatorRowKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Message => "message",
            Self::Orchestration => "orchestration",
            Self::Task => "task",
            Self::FileChange => "change",
            Self::Activity => "activity",
            Self::Approval => "approval",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityNavigatorStatus {
    Running,
    Blocked,
    Failed,
    Done,
}

impl ActivityNavigatorStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityNavigatorCounts {
    pub all: usize,
    pub running: usize,
    pub blocked: usize,
    pub failed: usize,
    pub done: usize,
    pub changes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityNavigatorRow {
    pub kind: ActivityNavigatorRowKind,
    pub status: ActivityNavigatorStatus,
    pub title: String,
    pub subtitle: String,
    pub detail_lines: Vec<String>,
    pub session_id: Option<SessionKey>,
    pub task_id: Option<TaskId>,
    pub turn_id: Option<String>,
    search_text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ActivityNavigatorRowLinks {
    session_id: Option<SessionKey>,
    task_id: Option<TaskId>,
    turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityNavigatorModel {
    pub rows: Vec<ActivityNavigatorRow>,
    pub counts: ActivityNavigatorCounts,
    pub selected: usize,
    pub query: String,
    pub filter: ActivityNavigatorFilter,
    pub search_active: bool,
}

impl ActivityNavigatorModel {
    pub fn selected_row(&self) -> Option<&ActivityNavigatorRow> {
        self.rows.get(self.selected)
    }
}

pub fn selected_activity_navigator_session(app: &AppState) -> Option<SessionKey> {
    activity_navigator_model(app)
        .selected_row()
        .and_then(|row| row.session_id.clone())
}

pub fn chat_layout_areas(app: &AppState, area: Rect) -> ChatLayoutAreas {
    let active_menu = active_menu_surface(app);
    chat_layout_areas_for_menu(app, area, active_menu.as_ref())
}

fn chat_layout_areas_for_menu(
    app: &AppState,
    area: Rect,
    active_menu: Option<&menu_render::MenuSurface>,
) -> ChatLayoutAreas {
    let session_strip_height = session_strip_height(app);
    let composer_height = composer_height_for_size(app, area.width, area.height);
    let desired_menu_height = menu_height_hint(active_menu, area.width, area.height);
    let autonomy_height = autonomy_indicator_height(app, area.width);
    let harness_height = harness_status_height(app);
    let decision_height = decision_banner_height(app);
    let agent_strip_height = agent_strip_height(app, area.height);
    let surface_budget = area.height.saturating_sub(
        min_transcript_height(area.height)
            + session_strip_height
            + composer_height
            + autonomy_height
            + harness_height
            + decision_height
            + agent_strip_height
            + 1,
    );
    let menu_height = desired_menu_height.min(surface_budget);
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(session_strip_height),
            Constraint::Min(8),
            Constraint::Length(menu_height),
            Constraint::Length(autonomy_height),
            Constraint::Length(harness_height),
            Constraint::Length(decision_height),
            Constraint::Length(composer_height),
            Constraint::Length(agent_strip_height),
            Constraint::Length(1),
        ])
        .split(area);

    ChatLayoutAreas {
        session_strip: root[0],
        transcript: root[1],
        menu: root[2],
        autonomy: root[3],
        harness: root[4],
        decision: root[5],
        composer: root[6],
        agent_strip: root[7],
        status: root[8],
    }
}

/// #324: the session strip renders only when there is something to glance at
/// — two or more open sessions. Single-session users pay zero rows.
fn session_strip_height(app: &AppState) -> u16 {
    if app.sessions.len() >= 2 { 1 } else { 0 }
}

/// OCTOS figlet wordmark shown in the MAIN window on the first-launch
/// onboarding entry screen (it used to live in a right-side preview pane).
const ONBOARDING_LOGO_ART: &str = "\
 ██████╗  ██████╗████████╗ ██████╗ ███████╗
██╔═══██╗██╔════╝╚══██╔══╝██╔═══██╗██╔════╝
██║   ██║██║        ██║   ██║   ██║███████╗
██║   ██║██║        ██║   ██║   ██║╚════██║
╚██████╔╝╚██████╗   ██║   ╚██████╔╝███████║
 ╚═════╝  ╚═════╝   ╚═╝    ╚═════╝ ╚══════╝";

/// Display width of the figlet wordmark (max over its lines), measured with
/// `unicode-width` so the box-drawing glyphs are counted by display columns.
fn onboarding_logo_art_width() -> usize {
    ONBOARDING_LOGO_ART
        .lines()
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
}

/// UX2 A.1: rows to spend on the OCTOS banner HEADER across the top of every
/// onboarding step. Taken ONLY from the surplus above what the menu itself
/// needs (`menu_needed`) so the step list, its inputs, and the explanation pane
/// are never clipped on short terminals. Full bordered figlet box when there is
/// room, else a compact one-line bordered tagline box, else nothing.
///
/// Layout (full): top border + blank + 6 art rows + blank + tagline + bottom
/// border = 11 rows. Compact: top border + tagline + bottom border = 3 rows.
fn onboarding_header_height(area_height: u16, area_width: u16, menu_needed: u16) -> u16 {
    let art_width = onboarding_logo_art_width() as u16;
    let surplus = area_height.saturating_sub(menu_needed);
    if area_width >= art_width + 4 && surplus >= 11 {
        11
    } else if surplus >= 3 {
        3
    } else {
        0
    }
}

fn onboarding_first_launch_active(app: &AppState) -> bool {
    app.sessions.is_empty()
        && app.menu_stack.active().is_some_and(|frame| {
            matches!(
                frame.id.as_str(),
                crate::menu::registry::MENU_ONBOARD
                    | crate::menu::registry::MENU_PROFILE_PICKER
                    | crate::menu::registry::MENU_ONBOARD_LANGUAGE
                    | crate::menu::registry::MENU_ONBOARD_FAMILY
                    | crate::menu::registry::MENU_ONBOARD_MODEL
                    | crate::menu::registry::MENU_ONBOARD_ROUTE
                    | crate::menu::registry::MENU_ONBOARD_WORKSPACE
                    | crate::menu::registry::MENU_ONBOARD_DONE
            )
        })
}

fn min_transcript_height(terminal_height: u16) -> u16 {
    if terminal_height < 30 { 8 } else { 12 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityNavigatorAreas {
    pub toolbar: Rect,
    pub list: Rect,
    pub detail: Rect,
    pub hint: Rect,
}

/// Build the agent-peek body lines: an identity/status/task header, a blank
/// separator, then the streamed output (or a placeholder until any arrives).
/// The sub-agent has no turn-by-turn transcript — only this streamed log — so
/// the header supplies the context a chat transcript otherwise would.
fn agent_overlay_lines(app: &AppState, palette: Palette, agent_id: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    if let Some(agent) = app.active_agent_record(agent_id) {
        let name = if agent.nickname.trim().is_empty() {
            agent.role.clone()
        } else {
            agent.nickname.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} {name}", agent_status_glyph(&agent.status)),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  ·  {}", agent.status), palette.muted()),
        ]));
        let task = agent
            .last_task
            .as_deref()
            .or(agent.title.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if let Some(task) = task {
            lines.push(Line::from(Span::styled(
                t!("app.hint.agent_task_prefix", task = task).into_owned(),
                palette.muted(),
            )));
        }
        if let Some(cwd) = agent
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        {
            lines.push(Line::from(Span::styled(
                format!("cwd: {cwd}"),
                palette.muted(),
            )));
        }
        // #334 (Phase 2): surface the child's DELIVERABLES (the `*-review.md` /
        // analysis files it wrote) from the roster record's artifacts, so the
        // detail view shows what the sub-agent produced, not just its log.
        if !agent.artifacts.is_empty() {
            lines.push(Line::from(Span::styled(
                t!("app.hint.agent_deliverables").into_owned(),
                palette.title(),
            )));
            for artifact in &agent.artifacts {
                let title = artifact.title.trim();
                let title = if title.is_empty() {
                    artifact.id.as_str()
                } else {
                    title
                };
                lines.push(Line::from(vec![
                    Span::styled("  • ", palette.muted()),
                    Span::styled(title.to_string(), palette.text()),
                    Span::styled(format!("  [{}]", artifact.kind), palette.muted()),
                ]));
            }
        }
        lines.push(Line::from(String::new()));
    }
    match app.active_agent_output_or_tail(agent_id) {
        Some(text) if !text.trim().is_empty() => {
            for raw in text.lines() {
                lines.push(Line::from(raw.to_string()));
            }
        }
        _ => lines.push(Line::from(Span::styled(
            t!("app.hint.agent_no_output").into_owned(),
            palette.muted(),
        ))),
    }
    lines
}

fn status_style(status: ActivityNavigatorStatus, palette: Palette) -> Style {
    match status {
        ActivityNavigatorStatus::Running => palette.selected(),
        ActivityNavigatorStatus::Blocked => Style::default().fg(palette.highlight),
        ActivityNavigatorStatus::Failed => Style::default().fg(palette.danger),
        ActivityNavigatorStatus::Done => Style::default().fg(palette.success),
    }
}

/// Whether a slash/command menu surface is active this frame — i.e. the chat
/// layout is reserving a `menu_height` row block (see `render_chat_layout` /
/// `render_viewport_with_finalization`). The inline draw loop tracks the
/// open→closed transition of this predicate to repaint the rows the menu block
/// vacated (a shrinking reserved block otherwise strands the transcript above a
/// blank band).
pub fn menu_surface_active(app: &AppState) -> bool {
    active_menu_surface(app).is_some()
}

fn active_menu_surface(app: &AppState) -> Option<menu_render::MenuSurface> {
    let frame = app.menu_stack.active();
    let stack_path = app
        .menu_stack
        .frames()
        .iter()
        .map(|frame| frame.id.to_string())
        .collect::<Vec<_>>();
    match app.active_menu.as_ref()? {
        crate::menu::MenuBuildResult::Ready(spec) => {
            Some(menu_render::MenuSurface::from_spec(spec, frame, stack_path))
        }
        crate::menu::MenuBuildResult::Loading(status)
        | crate::menu::MenuBuildResult::Unavailable(status)
        | crate::menu::MenuBuildResult::Error(status) => {
            Some(menu_render::MenuSurface::from_status(status, stack_path))
        }
    }
}

fn menu_height_hint(
    menu: Option<&menu_render::MenuSurface>,
    terminal_width: u16,
    terminal_height: u16,
) -> u16 {
    let Some(menu) = menu else {
        return 0;
    };
    let max_height = terminal_height.saturating_sub(15);
    if max_height == 0 {
        return 0;
    }
    menu_render::height_hint(menu, terminal_width)
        .min(max_height)
        .max(4.min(max_height))
}

/// Menu height for the INLINE VIEWPORT render pass. `menu_height_hint` budgets
/// against the full TERMINAL height (its `-15` heuristic reserves scrollback
/// rows) and sizes the viewport accordingly; re-applying that heuristic to the
/// viewport's own (much smaller) height collapsed the menu to zero rows — the
/// slash popup's space was reserved but rendered blank once the activity
/// collapse made viewports short. Here the menu simply takes its desired
/// height, clamped to the room the viewport actually has.
fn menu_height_for_viewport(
    menu: Option<&menu_render::MenuSurface>,
    viewport_width: u16,
    available: u16,
) -> u16 {
    let Some(menu) = menu else {
        return 0;
    };
    if available == 0 {
        return 0;
    }
    menu_render::height_hint(menu, viewport_width)
        .min(available)
        .max(4.min(available))
}

const COMPOSER_CHROME_ROWS: u16 = 4;
const COMPOSER_MIN_HEIGHT: u16 = 5;
const COMPOSER_MAX_INPUT_ROWS: u16 = 12;
const COMPOSER_SIDE_COLUMNS: u16 = 6;

fn composer_height_for_size(app: &AppState, terminal_width: u16, terminal_height: u16) -> u16 {
    match app.composer_presentation() {
        ComposerPresentation::Inline(text) => {
            COMPOSER_CHROME_ROWS
                + composer_visible_input_rows(&text, terminal_width, terminal_height)
        }
        ComposerPresentation::Empty | ComposerPresentation::Collapsed(_) => COMPOSER_MIN_HEIGHT,
    }
}

fn composer_input_row_cap(terminal_height: u16) -> u16 {
    terminal_height
        .saturating_sub(12)
        .saturating_div(2)
        .clamp(3, COMPOSER_MAX_INPUT_ROWS)
}

fn composer_text_width(terminal_width: u16) -> usize {
    usize::from(terminal_width.saturating_sub(COMPOSER_SIDE_COLUMNS).max(1))
}

fn composer_visible_input_rows(text: &str, terminal_width: u16, terminal_height: u16) -> u16 {
    let width = composer_text_width(terminal_width);
    let rows = text
        .split('\n')
        .map(|line| visual_rows_for_text(line, width))
        .sum::<usize>()
        .max(1);
    rows.min(usize::from(composer_input_row_cap(terminal_height))) as u16
}

fn visual_rows_for_text(text: &str, width: usize) -> usize {
    // Derived from the wrap so the rows reserved here always equal the rows
    // actually drawn by render_composer (and the rows the cursor math counts).
    wrap_composer_line(text, width).len()
}

/// Split one logical composer line into the visual sub-lines it occupies, each
/// fitting within `width` display columns. The `Paragraph` that draws the
/// composer has no soft-wrap, so without this the overflow of a long line is
/// clipped at the pane edge and its reserved continuation row renders blank
/// ("dark/invisible").
///
/// Packing is by grapheme cluster measured with `UnicodeWidthStr::width` (the
/// same primitive `str::width()` uses), so a multi-codepoint glyph (CJK, emoji
/// ZWJ/modifier/variation sequences) is never split across a row boundary, and
/// the chunk count is the authoritative visual-row count (`visual_rows_for_text`
/// delegates here) — keeping reserved height, rendered rows, and cursor row in
/// agreement for every input. Always returns at least one (possibly empty)
/// chunk so an empty logical line still occupies a row.
fn wrap_composer_line(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for grapheme in text.graphemes(true) {
        let g_w = grapheme.width();
        if current_w + g_w > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_w = 0;
        }
        current.push_str(grapheme);
        current_w += g_w;
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(current);
    }
    chunks
}

const CODE_BLOCK_LINE_LIMIT: usize = 120;
const COLLAPSED_TOOL_PREVIEW_LINES: usize = 1;
const EXPANDED_TOOL_PREVIEW_LINES: usize = 24;

/// True while an activity is genuinely in-flight. Thin wrapper over the shared
/// [`crate::model::activity_status_is_running`] running-status set so the
/// renderer's chip "active" count and the orphan activity-chip self-heal in
/// [`crate::model::AppState::capture_completed_turn_activity`] stay in lockstep.
/// Sub-agent liveness is tracked separately via the task count
/// ([`running_subagent_titles_for_chip`]).
fn is_running_activity(item: &ActivityItem) -> bool {
    crate::model::activity_status_is_running(&item.status)
}

/// True for a fresh session that has no messages yet — where we show the launch
/// banner at the top of the transcript area (it scrolls away on the first turn).
fn launch_banner_active(app: &AppState) -> bool {
    app.pending_messages.is_empty()
        && app
            .active_session()
            .is_some_and(|session| session.messages.is_empty() && session.live_reply.is_none())
}

struct TranscriptRenderModel {
    paragraph: Paragraph<'static>,
    metrics: TranscriptScrollMetrics,
}

const PAGER_SCROLLBAR_TRACK: &str = "│";
const PAGER_SCROLLBAR_THUMB: &str = "█";

fn pager_scrollbar_track(area: Rect) -> Option<Rect> {
    if area.width < 2 || area.height == 0 {
        return None;
    }

    Some(Rect::new(
        area.x + area.width.saturating_sub(1),
        area.y,
        1,
        area.height,
    ))
}

fn latest_user_message(session: &SessionView) -> Option<&str> {
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role.as_str() == "user")
        .map(|message| message.content.as_str())
        .filter(|content| !content.trim().is_empty())
}

fn pending_messages_contains(pending: &[String], content: &str) -> bool {
    pending.iter().any(|pending| pending == content)
}

fn anchored_turn_activity_logs<'a>(
    app: &'a AppState,
    session: &'a SessionView,
) -> Vec<(usize, &'a TurnActivityLog)> {
    app.turn_activity_logs
        .iter()
        .filter(|log| log.session_id == session.id)
        .filter_map(|log| {
            let anchor_index = log
                .anchor_index
                .filter(|idx| user_message_at(session, *idx))
                .or_else(|| {
                    log.request.as_ref().and_then(|request| {
                        session.messages.iter().rposition(|message| {
                            message.role.as_str() == "user" && message.content == *request
                        })
                    })
                })?;
            Some((activity_log_render_index(session, anchor_index), log))
        })
        .collect()
}

fn user_message_at(session: &SessionView, idx: usize) -> bool {
    session
        .messages
        .get(idx)
        .is_some_and(|message| message.role.as_str() == "user")
}

fn resolve_turn_prompt_anchor_for_render(
    session: &SessionView,
    anchor: &TurnPromptAnchor,
) -> Option<usize> {
    if session
        .messages
        .get(anchor.anchor_index)
        .is_some_and(|message| message.role.as_str() == "user" && message.content == anchor.content)
    {
        return Some(anchor.anchor_index);
    }

    session
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role.as_str() == "user" && message.content == anchor.content)
        .nth(anchor.prior_matching_user_count)
        .map(|(idx, _)| idx)
}

fn should_pin_recent_user_context(app: &AppState, session: &SessionView) -> bool {
    session.live_reply.is_some()
        || live_turn_diff_preview_visible(app)
        || app.active_turn().is_some()
        || app.run_state.is_active()
}

fn should_show_turn_flow(app: &AppState, session: &SessionView) -> bool {
    app.approval
        .as_ref()
        .is_some_and(|approval| approval.visible)
        || app
            .user_question
            .as_ref()
            .is_some_and(|picker| picker.visible)
        // NB: a `/btw` aside no longer forces the turn flow — it renders as a
        // floating top overlay (`render_btw_overlay`) so it doesn't mingle.
        || should_pin_recent_user_context(app, session)
}

/// Whether the ACTIVE session's turn is in its "thinking" phase: the model
/// has started reasoning (`live_reasoning` non-empty) and no answer has
/// streamed yet (`live_reply.text` empty). This is EXACTLY the swimming-octopus
/// condition, which the status-bar "Thinking" label tracks verbatim (the user
/// asked for "Thinking when the octopus swimming"); it flips to "Working" the
/// moment the answer begins streaming.
fn active_turn_is_thinking(app: &AppState) -> bool {
    let Some((session_id, turn_id)) = app.active_turn() else {
        return false;
    };
    let reasoning_started = app
        .live_reasoning
        .get(&(session_id.clone(), turn_id.clone()))
        .is_some_and(|reasoning| !reasoning.trim().is_empty());
    let answer_not_started = app
        .active_session()
        .and_then(|session| session.live_reply.as_ref())
        .is_none_or(|live_reply| live_reply.text.trim().is_empty());
    // Not thinking while parked on an operator decision FOR THIS session: an
    // approval-gated tool sets run_state Blocked and the status bar shows
    // "Waiting", so the octopus must stop too (codex round 3). The
    // approval/question slots are global, so scope them to the active session
    // — a background session's pending decision must not suppress the octopus
    // here (codex round 4). Durable state, not transient activity rows.
    let decision_for_active = app
        .approval
        .as_ref()
        .is_some_and(|approval| &approval.session_id == session_id)
        || app
            .user_question
            .as_ref()
            .is_some_and(|question| &question.session_id == session_id);
    let awaiting_operator =
        decision_for_active || matches!(app.run_state, SessionRunState::Blocked { .. });
    // Deliberately NOT gated on tool activity: this predicate IS the swimming
    // octopus, which the user asked the label to track ("Thinking when the
    // octopus swimming"). The octopus swims from the first reasoning delta
    // until the answer streams — including while tools run — so the label
    // matches it exactly.
    reasoning_started && answer_not_started && !awaiting_operator
}

/// A horizontal ASCII octopus that "swims" across the thinking line: a `[⇔]`
/// head flanked by the tilted-line glyphs `彡`/`ミ` (one arm per side). The two
/// frames are alternating paddle *strokes* — the arms flip mirror-image every
/// column step while the octopus ping-pongs left↔right (see [`octopus_swim`]),
/// so it visibly paddles the whole way instead of holding one pose per leg.
///
///   `彡[⇔]ミ` ⇄ `ミ[⇔]彡`
const OCTOPUS_SWIM_FRAMES: [&str; 2] = ["彡[⇔]ミ", "ミ[⇔]彡"];

/// One-way sweep duration: the octopus crosses edge-to-edge in this time
/// REGARDLESS of terminal width. The previous fixed ms-per-column pace made
/// the sweep ~21s one-way on a 146-column pane, so typical thinking phases
/// ended with the octopus visibly stuck around mid-screen ("only went half
/// of the page"). 4s matches the pace the capped sweep used to have.
const OCTOPUS_SWEEP_ONE_WAY_MS: u128 = 4_000;

/// Paddle-stroke cadence — the arms flip mirror-image at this interval,
/// independent of travel position (~3 strokes/sec reads as swimming, not a
/// strobe).
const OCTOPUS_STROKE_MS: u128 = 150;

/// How long the octopus rests at each edge before turning around. A pure
/// triangle wave touches its peak for a single millisecond, but the event
/// loop repaints only every ~120ms — sampled at 0, 120, …, 3960, 4080 the
/// edge column is never painted (and on a `MAX == 1` pane the octopus
/// appears frozen). Resting ≥ one repaint interval guarantees the far edge
/// is visibly reached every sweep.
const OCTOPUS_EDGE_DWELL_MS: u128 = 250;

/// Pure elapsed→(leading-space offset, frame) mapping for the swimming octopus.
///
/// The octopus travels horizontally as a trapezoid wave: the leading-space
/// offset climbs `0 → MAX` in [`OCTOPUS_SWEEP_ONE_WAY_MS`], RESTS at the far
/// edge for [`OCTOPUS_EDGE_DWELL_MS`], falls back, rests at the origin, and
/// repeats — sweeping the FULL `wrap_width`. `MAX` keeps the octopus plus a
/// one-column right margin inside it, measured in display *columns* via
/// `unicode-width` (the CJK arm glyphs are double-width). Position is
/// time-proportional, so it reaches the far edge every sweep on any width,
/// and the edge rest is at least one repaint interval so that frame is
/// actually painted. The paddle frame alternates every [`OCTOPUS_STROKE_MS`]
/// independent of travel. On a terminal too narrow to travel, `MAX` is 0 and
/// the octopus paddles in place at the left margin rather than panicking.
/// All arithmetic is overflow-safe: `offset` is bounded by `MAX`, so the
/// caller's `" ".repeat(offset)` can never run away.
fn octopus_swim(elapsed_ms: u128, wrap_width: usize) -> (usize, &'static str) {
    let octopus_width = UnicodeWidthStr::width(OCTOPUS_SWIM_FRAMES[0]);
    let frame = OCTOPUS_SWIM_FRAMES[((elapsed_ms / OCTOPUS_STROKE_MS) % 2) as usize];
    let max = wrap_width.saturating_sub(octopus_width + 1);
    if max == 0 {
        return (0, frame);
    }
    // Trapezoid wave in TIME (u128 end-to-end so a huge uptime can't
    // truncate): rise, dwell at MAX, fall, dwell at 0.
    let leg_ms = OCTOPUS_SWEEP_ONE_WAY_MS + OCTOPUS_EDGE_DWELL_MS;
    let phase = elapsed_ms % (2 * leg_ms);
    let one_way = if phase < leg_ms {
        // Rising for SWEEP ms, then resting at the far edge for DWELL ms.
        phase.min(OCTOPUS_SWEEP_ONE_WAY_MS)
    } else {
        // Falling for SWEEP ms, then resting at the origin for DWELL ms
        // (phase ≥ leg ⇒ the subtraction is ≤ SWEEP; saturation covers the
        // origin rest where it would go negative).
        (leg_ms + OCTOPUS_SWEEP_ONE_WAY_MS).saturating_sub(phase)
    };
    let offset = ((one_way * max as u128) / OCTOPUS_SWEEP_ONE_WAY_MS) as usize;
    (offset, frame)
}

/// `▰▰▰▰▱▱▱▱` fixed-width fraction bar for the compaction/context UX.
pub(crate) fn progress_bar(frac: f64, width: usize) -> String {
    let filled = ((frac.clamp(0.0, 1.0)) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(width - filled))
}

/// The in-progress compaction block (UPCR-2026-026):
/// ```text
/// ✶ Compacting conversation… (12s · 87.4k tokens)
///   ▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▰▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱▱ 49%
/// ```
/// The percentage is honest: pre-compaction tokens over the session's
/// context window (threshold as the fallback denominator).
/// How long the settled "context compacted" block dwells after completion.
/// The server pass is synchronous — started/completed land in one drain batch
/// and draws only follow the batch — so without this dwell the block would
/// paint zero frames, ever.
const LIVE_COMPACTION_SETTLED_DISPLAY_SECS: u64 = 4;

/// Push the committed `reasoning_content` as a capped "· reasoning" block,
/// gated on the active session's `/thinking` display toggle. Off by default
/// (codex-style quiet). Capped to the first `REASONING_BLOCK_CAP` lines unless
/// `expanded` (Ctrl+O), with a "+N more" affordance — the same convention as
/// tool output. A no-op when display is off or there is no reasoning.
const REASONING_BLOCK_CAP: usize = 6;

/// Hanging indent for assistant message bodies: the `• ` marker (2 display
/// columns) sits on the first visual line only, and every other physical line
/// of the same message hangs under it by this prefix, so the body reads as one
/// contiguous block (the Claude Code reference shape).
const ASSISTANT_BODY_INDENT: &str = "  ";

/// A localized status string in every bundled locale, so a synthesized card
/// stored in one language still matches after a `/lang` switch changes the
/// locale `t!` resolves against (codex P2 on #292).
fn localized_in_all_locales(key: &str) -> Vec<String> {
    ["en", "zh"]
        .into_iter()
        .map(|locale| rust_i18n::t!(key, locale = locale).into_owned())
        .collect()
}

/// Byte offset where a Session Summary block begins in `content`, if any. The
/// block is either the whole message (failure / no-answer card) or a suffix
/// appended after a partial live reply (`{prose}\n\n{summary}` — see
/// `finalize_live_reply_text`). Locale-independent: the title is matched
/// against every bundled locale so a stored card highlights regardless of the
/// current UI language.
fn session_summary_block_start(content: &str) -> Option<usize> {
    let titles = localized_in_all_locales("status.summary_title");
    let mut offset = 0usize;
    let mut iter = content.lines().peekable();
    while let Some(line) = iter.next() {
        let is_title = titles.iter().any(|title| title == line);
        let next_is_bullet = iter
            .peek()
            .is_some_and(|next| next.trim_start().starts_with("- "));
        if is_title && next_is_bullet {
            return Some(offset);
        }
        // `lines()` strips the `\n`; add it back. The final line has none, but
        // a match returns before we reach past it, so the +1 is never used out
        // of bounds.
        offset += line.len() + 1;
    }
    None
}

fn seeded_live_reply_content_can_emit(
    content: &str,
    previous_reply_has_output: bool,
    previous_reply_ends_blank: bool,
) -> bool {
    !content.trim().is_empty()
        || (previous_reply_has_output
            && !previous_reply_ends_blank
            && content.contains('\n')
            && content.lines().any(|line| line.trim().is_empty()))
}

fn code_block_is_unified_diff(language: &str, body: &[String]) -> bool {
    let language = language.trim().to_ascii_lowercase();
    if matches!(
        language.as_str(),
        "diff" | "patch" | "udiff" | "unidiff" | "gitdiff"
    ) {
        return true;
    }

    if !language.is_empty() && language != "code" {
        return false;
    }

    let mut has_hunk_or_file_header = false;
    let mut has_added = false;
    let mut has_removed = false;

    for line in body {
        let trimmed = line.trim_start();
        if trimmed.starts_with("@@")
            || trimmed.starts_with("diff --git")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("--- ")
            || trimmed.starts_with("+++ ")
        {
            has_hunk_or_file_header = true;
        }
        if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            has_added = true;
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            has_removed = true;
        }
    }

    has_hunk_or_file_header && (has_added || has_removed)
}

fn retitle_last_code_block_header_as_diff(lines: &mut [Line<'static>]) {
    let Some(line) = lines.last_mut() else {
        return;
    };
    let Some(label) = line.spans.last_mut() else {
        return;
    };
    if label.content.as_ref() == "code" {
        label.content = "diff".into();
    }
}

fn chat_message_bg(palette: Palette, role: &str) -> Color {
    match role {
        "user" => palette.diff_context_bg,
        "assistant" => palette.surface,
        "reasoning" => palette.surface,
        "btw" => palette.surface,
        "tool" => palette.surface,
        _ => palette.surface,
    }
}

/// Post-pass for hanging-indent bodies (assistant messages, whose `indent` is
/// the all-whitespace [`ASSISTANT_BODY_INDENT`]): swap the first non-blank
/// row's leading indent span for the `• ` prose marker, then pre-wrap any
/// over-width row so its wrapped continuations keep the hang. Both downstream
/// wrap paths (ratatui's `Wrap { trim: false }` in the live tail and
/// `insert_history::wrap_line` for native scrollback) restart wrapped rows at
/// column 0, so the body must never hand them an over-width line. Glyph-gutter
/// bodies (`$ `, `· `, `› `) and unindented bodies are left exactly as before.
fn finish_hanging_body(
    lines: &mut Vec<Line<'static>>,
    body_start: usize,
    palette: Palette,
    indent: &'static str,
    prose_marker: Option<&'static str>,
    bg: Option<Color>,
    width: usize,
) {
    if indent.is_empty() || !indent.trim().is_empty() {
        return;
    }

    // Sanitize BEFORE measuring — the same order `insert_history` uses. Tabs
    // render as four columns once scrollback sanitizes them, so measuring the
    // raw `\t` (0 columns here, 1 in `str::width`) under-counted the row: it
    // passed the pre-wrap check, then insert-time wrapping split it back to a
    // column-zero continuation, losing the hang (codex r2 P2). Stripping the
    // other control chars here also keeps the pre-wrap cutter's budget honest
    // (codex r2 P1) and renders deterministically in the live lane.
    for line in lines[body_start..].iter_mut() {
        crate::insert_history::sanitize_line_in_place(line);
    }

    if let Some(marker) = prose_marker
        && let Some(first_line) = lines[body_start..]
            .iter_mut()
            .find(|line| !line_is_blank(Some(line)))
    {
        let marker_span = Span::styled(marker, style_bg(palette.selected(), bg));
        match first_line.spans.first_mut() {
            // Every body emitter leads with the indent span; the marker
            // replaces it 1:1 (same display width), keeping the row width
            // unchanged.
            Some(lead) if lead.content.as_ref() == indent => *lead = marker_span,
            _ => first_line.spans.insert(0, marker_span),
        }
    }

    let line_width = |line: &Line<'static>| -> usize {
        line.spans
            .iter()
            .map(|span| span.content.as_ref().width())
            .sum()
    };
    if lines[body_start..]
        .iter()
        .all(|line| line_width(line) <= width)
    {
        return;
    }

    let content_width = width.saturating_sub(indent.width()).max(1);
    let body = lines.split_off(body_start);
    for mut line in body {
        if line_width(&line) <= width {
            lines.push(line);
            continue;
        }
        // Detach the leading indent/marker span, wrap the remainder to the
        // hang-reduced width, then re-attach: row 0 keeps its own lead,
        // continuation rows get the hang.
        let lead = match line.spans.first() {
            Some(span)
                if span.content.as_ref() == indent
                    || prose_marker.is_some_and(|marker| span.content.as_ref() == marker) =>
            {
                Some(line.spans.remove(0))
            }
            _ => None,
        };
        let hang_style = lead
            .as_ref()
            .map(|span| span.style)
            .unwrap_or_else(|| style_bg(palette.border(), bg));
        let style = line.style;
        let rest = Line::from(std::mem::take(&mut line.spans)).style(style);
        for (row_idx, row) in crate::insert_history::wrap_line(&rest, content_width)
            .into_iter()
            .enumerate()
        {
            let mut spans = Vec::with_capacity(row.spans.len() + 1);
            match (&lead, row_idx) {
                (Some(lead), 0) => spans.push(lead.clone()),
                _ => spans.push(Span::styled(indent, hang_style)),
            }
            spans.extend(row.spans);
            lines.push(Line::from(spans).style(style));
        }
    }
}

/// Minimum width a table column is allowed to shrink to (just an `…`). Columns
/// shrink this far before the per-line clip (below) becomes the last resort, so
/// even many-column tables fit the pane whenever the column count allows.
const MIN_TABLE_COL: usize = 1;

#[allow(clippy::too_many_arguments)]
fn table_border_line(
    indent: &'static str,
    widths: &[usize],
    left: char,
    mid: char,
    right: char,
    border: Style,
    bg: Option<Color>,
    width: usize,
) -> Line<'static> {
    let mut s = String::new();
    s.push(left);
    for (idx, w) in widths.iter().enumerate() {
        if idx > 0 {
            s.push(mid);
        }
        for _ in 0..(w + 2) {
            s.push('─');
        }
    }
    s.push(right);
    let spans = vec![Span::styled(indent, border), Span::styled(s, border)];
    chat_line(clip_line_spans(spans, width), bg)
}

/// Hard-cut a fully-built line's spans to `max_width` display columns (no
/// ellipsis) so an over-wide table row is clipped rather than wrapped.
fn clip_line_spans(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= max_width {
            break;
        }
        let span_w = span.content.as_ref().width();
        if used + span_w <= max_width {
            used += span_w;
            out.push(span);
        } else {
            let mut clipped = String::new();
            for ch in span.content.chars() {
                let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + ch_w > max_width {
                    break;
                }
                clipped.push(ch);
                used += ch_w;
            }
            if !clipped.is_empty() {
                out.push(Span::styled(clipped, span.style));
            }
            break;
        }
    }
    out
}

/// Render an inline-markdown table cell as styled spans, truncating to `max_w`
/// display columns (with an `…`) so the bordered grid stays aligned. Returns the
/// spans and the display width they occupy (`<= max_w`).
fn fit_cell_spans(
    cell: &str,
    max_w: usize,
    normal: Style,
    bold: Style,
    code: Style,
) -> (Vec<Span<'static>>, usize) {
    let spans = inline_markdown_spans(cell, normal, bold, code);
    let total: usize = spans.iter().map(|span| span.content.as_ref().width()).sum();
    if total <= max_w {
        return (spans, total);
    }
    if max_w == 0 {
        return (Vec::new(), 0);
    }
    let budget = max_w - 1; // leave room for the ellipsis
    let mut out = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let span_w = span.content.as_ref().width();
        if used + span_w <= budget {
            used += span_w;
            out.push(span);
        } else {
            let mut clipped = String::new();
            for ch in span.content.chars() {
                let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + ch_w > budget {
                    break;
                }
                clipped.push(ch);
                used += ch_w;
            }
            if !clipped.is_empty() {
                out.push(Span::styled(clipped, span.style));
            }
            break;
        }
    }
    out.push(Span::styled("…", normal));
    (out, used + 1)
}

fn table_cell_width(cell: &str) -> usize {
    // Column padding must match the terminal's *display* width, not the char
    // count — emoji/CJK render at width 2 but are a single char, so
    // chars().count() under-pads their columns and misaligns the table.
    restore_streamed_sentence_spacing(&plain_inline_markdown(cell))
        .as_str()
        .width()
}

fn chat_line(spans: Vec<Span<'static>>, bg: Option<Color>) -> Line<'static> {
    let line = Line::from(spans);
    match bg {
        Some(bg) => line.style(Style::default().bg(bg)),
        None => line,
    }
}

fn style_bg(style: Style, bg: Option<Color>) -> Style {
    match bg {
        Some(bg) => style.bg(bg),
        None => style,
    }
}

fn truncate_terminal_line(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        return text.to_string();
    }

    let keep = max_chars.saturating_sub(4);
    let mut preview = text.chars().take(keep).collect::<String>();
    preview.push_str(" ...");
    preview
}

/// Truncate `text` to at most `max_cols` terminal *display* columns
/// (unicode-width aware), appending a `…` when it overflows. Unlike
/// [`truncate_terminal_line`] this counts double-width CJK/emoji glyphs as 2
/// columns, so a row built from the result can never exceed its column budget
/// and wrap. Never splits a char and never byte-slices, so it cannot panic on
/// a multibyte boundary. The returned string's display width is `<= max_cols`.
fn truncate_to_display_width(text: &str, max_cols: usize) -> String {
    if text.width() <= max_cols {
        return text.to_string();
    }
    if max_cols == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis marker.
    let budget = max_cols - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_w > budget {
            break;
        }
        out.push(ch);
        used += ch_w;
    }
    out.push('…');
    out
}

fn line_is_blank(line: Option<&Line<'static>>) -> bool {
    line.map(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
        .unwrap_or(false)
}

/// True when a line is a thematic break (`---`, `***`, `___`): ≥3 of a single
/// marker char once spaces are removed. Table separators (which contain `|`)
/// are handled earlier and never reach here.
fn markdown_hr(line: &str) -> bool {
    let stripped: String = line
        .trim()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect();
    if stripped.len() < 3 {
        return false;
    }
    let mut chars = stripped.chars();
    let first = chars.next().unwrap();
    matches!(first, '-' | '*' | '_') && chars.all(|ch| ch == first)
}

fn markdown_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hash_count = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hash_count) {
        return None;
    }
    let heading = trimmed.get(hash_count..)?.strip_prefix(' ')?;
    (!heading.trim().is_empty()).then_some(heading.trim())
}

fn markdown_checkbox(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    if let Some(text) = trimmed
        .strip_prefix("- [x] ")
        .or_else(|| trimmed.strip_prefix("- [X] "))
    {
        return Some((true, text.trim()));
    }
    trimmed
        .strip_prefix("- [ ] ")
        .map(|text| (false, text.trim()))
}

fn markdown_emphasis_segment(rest: &str) -> Option<(&str, usize)> {
    let delimiter = rest.chars().next()?;
    if !matches!(delimiter, '*' | '_') {
        return None;
    }
    let after_open = &rest[delimiter.len_utf8()..];
    if after_open.starts_with(delimiter) {
        return None;
    }
    let close = after_open.find(delimiter)?;
    let emphasized = &after_open[..close];
    if emphasized.is_empty() || emphasized.chars().all(char::is_whitespace) {
        return None;
    }
    Some((
        emphasized,
        delimiter.len_utf8() + close + delimiter.len_utf8(),
    ))
}

fn markdown_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .filter(|text| !text.trim().is_empty())
        .map(str::trim)
}

fn markdown_blockquote(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("> ")
        .or_else(|| trimmed.strip_prefix('>'))
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn markdown_numbered(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let dot = trimmed.find(". ")?;
    let number = &trimmed[..dot];
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let text = trimmed[dot + 2..].trim();
    (!text.is_empty()).then_some((number, text))
}

fn markdown_table_cells(line: &str) -> Option<Vec<&str>> {
    let trimmed = line.trim();
    if !(trimmed.starts_with('|') && trimmed.ends_with('|')) {
        return None;
    }
    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    (cells.len() >= 2 && cells.iter().any(|cell| !cell.is_empty())).then_some(cells)
}

fn markdown_table_separator(line: &str) -> bool {
    markdown_table_cells(line).is_some_and(|cells| {
        cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|ch| matches!(ch, '-' | ':' | ' ')))
    })
}

/// Parse a markdown link `[text](url)` at the start of `s`, requiring
/// non-empty text AND url. Returns `(link_text, url, consumed_bytes)`, or `None`
/// to fall through to the plain-text path. Shared by the span renderer and the
/// width-measurement path (`plain_inline_markdown`) so the two cannot drift —
/// a link in a table cell measures exactly what it renders.
fn parse_markdown_link(s: &str) -> Option<(&str, &str, usize)> {
    let after_lb = s.strip_prefix('[')?;
    let mid = after_lb.find("](")?;
    let rel_close = after_lb[mid + 2..].find(')')?;
    let link_text = &after_lb[..mid];
    let url = &after_lb[mid + 2..mid + 2 + rel_close];
    if link_text.is_empty() || url.is_empty() {
        return None;
    }
    // '[' + link_text + "](" + url + ')'
    Some((link_text, url, 1 + mid + 2 + rel_close + 1))
}

/// Parse `~~text~~` at the start of `s`, requiring NON-WHITESPACE content
/// between the markers. Returns `(struck_text, consumed_bytes)`, or `None` for
/// degenerate forms (`~~~~`, `~~ ~~`) so they fall through to the plain-text
/// path and the literal tildes survive instead of being silently eaten. Shared
/// by the span renderer and `plain_inline_markdown` so width matches render.
fn parse_markdown_strikethrough(s: &str) -> Option<(&str, usize)> {
    let after_open = s.strip_prefix("~~")?;
    let close = after_open.find("~~")?;
    let struck = &after_open[..close];
    if struck.trim().is_empty() {
        return None;
    }
    // "~~" + struck + "~~"
    Some((struck, 2 + close + 2))
}

fn inline_markdown_spans(
    text: &str,
    normal_style: Style,
    bold_style: Style,
    code_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;

    while !rest.is_empty() {
        // Link `[text](url)`: text in the highlight (code) style, url appended
        // dimmed. NOT a real OSC 8 hyperlink — ratatui renders cell-by-cell, so
        // a raw escape would be counted as width and corrupt the layout.
        // The url is rendered IN FULL and unbroken (no truncation) so the
        // terminal's native URL detector can linkify it for cmd/ctrl+click in
        // the native-scrollback flow. (When the link text already IS the url,
        // we show it once instead of duplicating.)
        if let Some((link_text, url, consumed)) = parse_markdown_link(rest) {
            if link_text == url {
                spans.push(Span::styled(url.to_string(), code_style));
            } else {
                spans.push(Span::styled(link_text.to_string(), code_style));
                spans.push(Span::styled(
                    format!(" ({url})"),
                    normal_style.add_modifier(Modifier::DIM),
                ));
            }
            rest = &rest[consumed..];
            continue;
        }

        if let Some((struck, consumed)) = parse_markdown_strikethrough(rest) {
            spans.push(Span::styled(
                struck.to_string(),
                normal_style.add_modifier(Modifier::CROSSED_OUT),
            ));
            rest = &rest[consumed..];
            continue;
        }

        if let Some(after_open) = rest.strip_prefix("**")
            && let Some(close) = after_open.find("**")
        {
            let bold = &after_open[..close];
            if !bold.is_empty() {
                spans.push(Span::styled(bold.to_string(), bold_style));
            }
            rest = &after_open[close + 2..];
            continue;
        }

        if let Some(after_open) = rest.strip_prefix('`')
            && let Some(close) = after_open.find('`')
        {
            let code = &after_open[..close];
            if !code.is_empty() {
                spans.push(Span::styled(code.to_string(), code_style));
            }
            rest = &after_open[close + 1..];
            continue;
        }

        if let Some((emphasis, consumed)) = markdown_emphasis_segment(rest) {
            spans.push(Span::styled(
                emphasis.to_string(),
                bold_style.add_modifier(Modifier::ITALIC),
            ));
            rest = &rest[consumed..];
            continue;
        }

        let next_bold = rest.find("**");
        let next_code = rest.find('`');
        // Stop a plain-text run before a link/strike opener so the next loop
        // iteration can parse it (otherwise the run would swallow `[` / `~~`).
        let next_link = rest.find('[');
        let next_strike = rest.find("~~");
        let next_emphasis = rest
            .char_indices()
            .skip(1)
            .find(|(_, ch)| matches!(ch, '*' | '_'))
            .map(|(idx, _)| idx);
        let next = [next_bold, next_code, next_link, next_strike, next_emphasis]
            .into_iter()
            .flatten()
            .min();
        let take = next.unwrap_or(rest.len());
        if take == 0 {
            let mut chars = rest.chars();
            if let Some(ch) = chars.next() {
                spans.push(Span::styled(ch.to_string(), normal_style));
                rest = chars.as_str();
            } else {
                break;
            }
        } else {
            spans.push(Span::styled(
                restore_streamed_sentence_spacing(&rest[..take]),
                normal_style,
            ));
            rest = &rest[take..];
        }
    }

    spans
}

fn restore_streamed_sentence_spacing(text: &str) -> String {
    let mut repaired = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        repaired.push(ch);
        let needs_sentence_space = matches!(ch, '.' | '!' | '?')
            && chars.peek().is_some_and(|next| next.is_ascii_uppercase())
            && repaired
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|prev| prev.is_ascii_lowercase() || prev == ')');
        let needs_colon_space = ch == ':'
            && chars
                .peek()
                .is_some_and(|next| next.is_ascii_uppercase() || !next.is_ascii())
            && repaired
                .chars()
                .rev()
                .nth(1)
                .is_some_and(|prev| prev.is_ascii_alphanumeric() || prev == ')');
        if needs_sentence_space || needs_colon_space {
            repaired.push(' ');
        }
    }

    repaired
}

struct FileMutationActivity {
    operation: String,
    path: String,
    preview_ready: bool,
}

impl FileMutationActivity {
    fn from_item(item: &ActivityItem) -> Option<Self> {
        if item.kind != ActivityKind::Progress {
            return None;
        }
        if item.title != "file_mutation" && !item.status.starts_with("File mutation: ") {
            return None;
        }

        let source = item
            .detail
            .as_deref()
            .or_else(|| item.status.strip_prefix("File mutation: "))
            .filter(|source| !source.is_empty())?;
        let preview_ready = source.contains("diff preview ready");
        let source = source
            .replace(" | diff preview ready", "")
            .replace("diff preview ready", "");
        let (operation, path) = source.trim().split_once(' ')?;
        if path.trim().is_empty() {
            return None;
        }

        Some(Self {
            operation: operation.to_string(),
            path: path.trim().to_string(),
            preview_ready,
        })
    }
}

fn file_mutation_action_label(operation: &str) -> String {
    match operation {
        "add" | "added" | "create" | "created" => t!("app.tool.added"),
        "delete" | "deleted" | "remove" | "removed" => t!("app.tool.deleted"),
        "write" | "wrote" => t!("app.tool.wrote"),
        "modify" | "modified" | "update" | "updated" => t!("app.tool.changed"),
        _ => t!("app.tool.changed"),
    }
    .into_owned()
}

fn compact_file_path(path: &str) -> String {
    let components = path
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    let keep = 4;
    if components.len() <= keep {
        return path.to_string();
    }
    format!(".../{}", components[components.len() - keep..].join("/"))
}

/// octos exposes several shell-family tools that all run a command string:
/// `shell`/`sh`/`exec`/`exec_command` (field `command`) and the
/// codex-compatible `bash` (field `cmd`, falling back to `command`). They all
/// render as a real command line, never the raw JSON arguments blob. Kept in
/// sync with the projection-side extraction in
/// [`crate::store::tool_invocation_detail`].
pub(crate) fn is_shell_family_tool(title: &str) -> bool {
    matches!(
        title.to_ascii_lowercase().as_str(),
        "shell" | "sh" | "exec" | "exec_command" | "bash"
    )
}

/// Longest raw-JSON fallback (display columns) `tool_invocation_text` will emit
/// when it has no better human rendering — a hard cap so a pathological args
/// blob can never be handed to the row builder unbounded. The per-row width
/// budget truncates further; this only bounds the worst case.
const RAW_ARG_FALLBACK_COLS: usize = 512;

/// A human-readable one-line invocation for a tool activity, preferring a real
/// command string over the raw serialized arguments (which used to leak into
/// the card as `{"cmd":…}`). Order: an explicit `detail` (run through the
/// args-echo humanizer — the server path fills it with the protocol #1606
/// `arguments_preview` JSON echo), then a shell-like tool's command string,
/// then a compact `key=value` of the first meaningful object field, then a
/// bounded raw-JSON fallback.
///
/// DISPLAY-ONLY: `ActivityItem.detail` itself is never rewritten so the
/// underlying protocol-provided invocation echo remains available to other
/// activity consumers.
fn tool_invocation_text(item: &ActivityItem) -> Option<String> {
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        return Some(humanize_args_echo(detail, &item.title));
    }
    let arguments = item.arguments.as_ref()?;
    // The projection lane can carry a serialized args echo in `arguments` as
    // a JSON String: treat the inner text exactly like a detail echo —
    // re-serializing it would render `"{\"cmd\":…`.
    if let Some(echo) = arguments.as_str() {
        let echo = echo.trim();
        if !echo.is_empty() {
            return Some(humanize_args_echo(echo, &item.title));
        }
    }
    // Shell-like tools carry their command under `command`/`cmd`; surface that
    // (untruncated — callers like `shell_action_label` match on the full text,
    // and the row builder applies the display-width budget) instead of the JSON
    // envelope.
    if is_shell_like_tool(&item.title) {
        if let Some(command) = shell_command_from_args(arguments) {
            return Some(command);
        }
    }
    // Other tools with an object payload: show a compact `key=value` of the
    // first meaningful string/number field rather than the whole JSON blob.
    if let Some(map) = arguments.as_object() {
        if let Some(rendered) = first_meaningful_arg(map) {
            return Some(single_line_invocation(&rendered));
        }
    }
    // Last resort: bounded raw JSON (never an unbounded dump).
    serde_json::to_string(arguments)
        .ok()
        .map(|json| truncate_to_display_width(&json, RAW_ARG_FALLBACK_COLS))
}

/// The `command`/`cmd` string of a shell-like tool's args object, flattened to
/// one line. `None` when the payload has no non-empty command string.
fn shell_command_from_args(arguments: &serde_json::Value) -> Option<String> {
    arguments
        .get("command")
        .or_else(|| arguments.get("cmd"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .map(single_line_invocation)
}

/// Humanize a serialized arguments echo for the one-line tool row. The server
/// caps the echo (~700 bytes, protocol #1606), so a JSON object echo often
/// arrives CUT mid-string — strict parsing gets the well-formed case, a
/// lenient scan covers the truncated one, and a cleanup pass guarantees the
/// floor: no raw `{"key":` prefix, no literal `\n`/`\t` escape leaking into
/// the row.
///
/// `detail` ALSO carries already-decoded REAL invocation text (the `!`-bang
/// echo, the live-lane command summaries, progress prose, thread markers), so
/// the transforms are gated on the two serialized-echo shapes and everything
/// else renders verbatim (one-lined only): a brace-group command `{ echo ok; }`
/// is NOT a JSON echo (that requires `{"`), and `printf '\n'` keeps its
/// intentional two-char escape (escape decoding requires the `key: value`
/// preview opener).
fn humanize_args_echo(echo: &str, title: &str) -> String {
    let trimmed = echo.trim();
    if looks_like_json_object_echo(trimmed) {
        // Complete echo: strict parse, then the same rendering the
        // object-arguments path uses (command string / first `key=value`).
        if let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(trimmed)
        {
            let value = serde_json::Value::Object(map);
            if is_shell_like_tool(title) {
                if let Some(command) = shell_command_from_args(&value) {
                    return command;
                }
            }
            if let Some(map) = value.as_object() {
                if let Some(rendered) = first_meaningful_arg(map) {
                    return single_line_invocation(&rendered);
                }
            }
        } else if is_shell_like_tool(title) {
            // Truncated echo (strict parse fails): scan for the command key
            // and decode the string value up to the cut.
            if let Some(command) = lenient_echo_command(trimmed) {
                return command;
            }
        }
        // Floor for anything else `{`-shaped (truncated non-shell echo, or an
        // object with no scalar field): strip the JSON framing and decode the
        // common escapes so the row never shows `{"key":` or a literal `\n`.
        return single_line_invocation(&scrub_json_echo_fragment(trimmed));
    }
    // The producer's `key: value` preview format JSON-encodes string values,
    // so decode the common escapes there; rows are one-line, so an escaped
    // newline becomes a space.
    if has_key_value_echo_opener(trimmed) {
        return single_line_invocation(&decode_json_string_escapes(trimmed));
    }
    // Plain already-decoded text (bang commands, live-lane invocation
    // summaries, progress prose, thread markers): verbatim, one-lined.
    single_line_invocation(trimmed)
}

/// A serialized JSON object echo starts `{"` (optionally with whitespace
/// between — pretty printing), because the first thing inside a JSON object is
/// a quoted key. A brace-group shell command (`{ echo ok; }`) does not, so it
/// is never mistaken for an echo.
fn looks_like_json_object_echo(text: &str) -> bool {
    text.strip_prefix('{')
        .is_some_and(|rest| rest.trim_start().starts_with('"'))
}

/// The `key: value` preview opener the #1606 producer emits for object args
/// (`cmd: "grep …", timeout: 300`): a bare identifier-ish key, then `: `. Real
/// commands/prose almost never start this way (`printf '\n'` has no colon; an
/// `echo "note: x"` command's first token contains spaces/quotes and fails the
/// key charset).
fn has_key_value_echo_opener(text: &str) -> bool {
    let Some((key, _)) = text.split_once(": ") else {
        return false;
    };
    !key.is_empty()
        && key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

/// Lenient `command`/`cmd` extraction from a truncated JSON object echo that
/// `serde_json` cannot parse (the ~700-byte cap cuts mid-string): find the
/// key, then decode its string value up to the closing unescaped quote or the
/// end of the input. Char-boundary safe (operates on `char`s, and the marker
/// find can only land on ASCII boundaries).
fn lenient_echo_command(echo: &str) -> Option<String> {
    for key in ["\"command\"", "\"cmd\""] {
        let Some(pos) = echo.find(key) else {
            continue;
        };
        let rest = echo[pos + key.len()..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let Some(body) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let command = single_line_invocation(&decode_json_string_body(body, true));
        if !command.is_empty() {
            return Some(command);
        }
    }
    None
}

/// Floor rendering for a truncated JSON echo with no better extraction: drop
/// the leading `{`/`"` framing and decode the common escapes. The result is
/// not pretty, but it never shows a raw `{"key":` prefix or a literal `\n`.
fn scrub_json_echo_fragment(echo: &str) -> String {
    let body = echo.strip_prefix('{').unwrap_or(echo).trim_start();
    let body = body.strip_prefix('"').unwrap_or(body);
    decode_json_string_escapes(body)
}

/// Decode the common JSON string escapes for one-line display: `\"`→`"`,
/// `\\`→`\`, `\n`/`\t`/`\r`→space. Unknown escapes pass through verbatim and a
/// dangling trailing backslash (left by the echo's byte cap) is dropped.
fn decode_json_string_escapes(text: &str) -> String {
    decode_json_string_body(text, false)
}

/// Shared escape decoder. With `stop_at_quote`, decoding ends at the first
/// unescaped `"` (the value's closing quote in a JSON echo — trailing sibling
/// keys are dropped); otherwise the whole input is decoded.
fn decode_json_string_body(text: &str, stop_at_quote: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if stop_at_quote => break,
            '\\' => match chars.next() {
                Some('n' | 't' | 'r') => out.push(' '),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                // Dangling backslash at the truncation cut — drop it.
                None => {}
            },
            other => out.push(other),
        }
    }
    out
}

/// Rows are one-line: flatten real newlines/tabs in an invocation to spaces
/// (the row is width-truncated by the builder; multi-line content belongs to
/// the `│` output-preview lines, which are NOT run through this).
fn single_line_invocation(text: &str) -> String {
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '\t')) {
        text.chars()
            .map(|ch| match ch {
                '\n' | '\r' | '\t' => ' ',
                other => other,
            })
            .collect::<String>()
            .trim()
            .to_string()
    } else {
        text.trim().to_string()
    }
}

/// Case-insensitive check for the shell family whose invocation is a command
/// string (`shell`/`bash`/`sh`). Kept in one place so the command-extraction in
/// [`tool_invocation_text`] and the `$ ` prompt in the row builder agree.
fn is_shell_like_tool(title: &str) -> bool {
    matches!(title.to_ascii_lowercase().as_str(), "shell" | "bash" | "sh")
}

/// Render the first meaningful field of an args object as a compact
/// `key=value`, bounded so a huge value can't blow up the row. Returns `None`
/// when no scalar (string/number/bool) field is present.
fn first_meaningful_arg(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for (key, value) in map {
        let rendered = match value {
            serde_json::Value::String(s) if !s.trim().is_empty() => s.trim().to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => continue,
        };
        let value = truncate_to_display_width(&rendered, RAW_ARG_FALLBACK_COLS);
        return Some(format!("{key}={value}"));
    }
    None
}
fn meaningful_output_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect()
}

fn format_duration_ms(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        return format!("{duration_ms}ms");
    }
    let seconds = duration_ms as f64 / 1_000.0;
    if seconds < 10.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{seconds:.0}s")
    }
}

fn format_elapsed_secs(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn approval_action_labels(_approval: &ApprovalModalState) -> [String; 3] {
    [
        t!("app.approval.action_once").to_string(),
        t!("app.approval.action_session").to_string(),
        t!("app.approval.action_deny").to_string(),
    ]
}

/// Render the `/btw` aside as a floating BORDERED pane pinned to the TOP of
/// the live viewport. It draws over the top rows of the live tail each frame
/// (never flushed to scrollback) and vanishes on the next prompt submit. The
/// border + title are load-bearing: a borderless overlay reads as embedded
/// transcript text whenever the tail is short — the box is what makes it a
/// visibly distinct window over the session instead of part of the flow.
/// Rows the `/btw` overlay pane wants (card lines sans leading blanks, plus
/// the two border rows); `0` when the active session has no aside. The aside
/// contributes NO lines to the turn flow, so [`live_tail_height_with_finalization`]
/// must reserve these rows explicitly — a settled session's tail otherwise
/// collapses to 1-2 rows, under [`render_btw_overlay`]'s 3-row minimum, and
/// the pane silently stops drawing while the aside is still answering
/// (codex P1). Kept in sync with `render_btw_overlay`'s layout math.
/// Build the `/btw` overlay's inner lines, WRAPPED to `inner_width`, with the
/// card's leading spacer dropped (the border already separates it). Wrapping
/// here — mirroring every other transcript pane, which the overlay's own
/// `Paragraph` historically did NOT — is what makes the physical-row count exact
/// so the pane can size to fit and scroll precisely. Shared by the height hint
/// and the renderer so the two never drift.
fn btw_overlay_wrapped_lines(
    palette: Palette,
    aside: &crate::model::BtwAside,
    inner_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    push_btw_aside_card(&mut lines, palette, aside, inner_width);
    while line_is_blank(lines.first()) {
        lines.remove(0);
    }
    lines
        .iter()
        .flat_map(|line| crate::insert_history::wrap_line(line, inner_width))
        .collect()
}

fn btw_overlay_height_hint(app: &AppState, area_width: u16) -> u16 {
    if area_width < 4 {
        return 0;
    }
    let Some(session) = app.active_session() else {
        return 0;
    };
    let Some(aside) = app.btw_asides.get(&session.id) else {
        return 0;
    };
    let wrapped = btw_overlay_wrapped_lines(
        Palette::for_theme(app.theme),
        aside,
        area_width as usize - 2,
    );
    if wrapped.is_empty() {
        return 0;
    }
    // Ask for the full wrapped content + borders; the caller
    // (`live_tail_height_with_finalization`) caps the tail at half the viewport,
    // and the renderer scrolls whatever still doesn't fit.
    (wrapped.len() as u16).saturating_add(2)
}

fn user_question_action_labels(picker: &UserQuestionPickerState) -> Vec<String> {
    // Garbled / 0-question event: nothing is answerable, so offer only a dismiss
    // hint — never a submit affordance that would form an invalid respond
    // (DO-NOT-SHIP #2). Ctrl+R/Alt+a re-opens it if dismissed (DO-NOT-SHIP #1).
    if picker.questions.is_empty() {
        return vec![t!("app.question.action_dismiss").to_string()];
    }
    let mut labels = vec![t!("app.question.action_toggle").to_string()];
    if picker.is_last_question() {
        labels.push(t!("app.question.action_submit").to_string());
    } else {
        labels.push(t!("app.question.action_next").to_string());
    }
    labels
}

#[cfg(test)]
fn fit_card_text(text: &str, width: usize) -> String {
    // Reserve the 4-space prefix added by the caller. The budget is DISPLAY
    // COLUMNS (unicode-width), not chars — CJK glyphs are double-width, so a
    // char-count budget let CJK options overflow the card (mirror of
    // `clip_line_spans`).
    let budget = width.saturating_sub(4).max(1);
    if text.width() <= budget {
        return text.to_string();
    }
    let cut = budget.saturating_sub(1); // leave a column for the ellipsis
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_w > cut {
            break;
        }
        out.push(ch);
        used += ch_w;
    }
    out.push('…');
    out
}

/// Pending master re-entries the server has queued for the active session
/// (from the `session/orchestration` mirror). Drives the "re-entering" chip
/// title so a settled-but-continuing turn doesn't read as completed.
fn active_session_pending_continuations(app: &AppState) -> u32 {
    app.active_session()
        .and_then(|session| app.orchestration.get(&session.id))
        .filter(|status| status.active)
        .map(|status| status.pending_continuations)
        .unwrap_or(0)
}

/// Whether the agent-task group identified by `group_turn` is the CURRENT/active
/// turn's group (vs an ARCHIVED past-turn group).
///
/// Blocking bug 1: `active_session_pending_continuations` is a per-SESSION
/// fact (the server's queued re-entry count), so feeding it to every group
/// retitled archived completed/failed groups as "Re-entering". Only the active
/// group may flip to "Re-entering"; this predicate scopes that.
///
/// A group is active when:
/// - its `turn_id` equals the active session's live turn (`active_turn`), OR
/// - it is the turn-less fold (`None`) AND no turn is live but the session is
///   orchestrating — the turn-less sub-agent fold of the live orchestration is
///   the current group (see `flow_activity_items` / `is_subagent_progress`).
fn is_active_group(app: &AppState, group_turn: Option<&octos_core::ui_protocol::TurnId>) -> bool {
    match (group_turn, app.active_turn()) {
        (Some(group_turn), Some((_, active_turn))) => group_turn == active_turn,
        (Some(_), None) => false,
        // Turn-less fold: only the live orchestration's sub-agent fold (no live
        // turn) is the current group. With a live turn present, the turn-less
        // fold is not the active group.
        (None, None) => app
            .active_session()
            .and_then(|session| app.orchestration.get(&session.id))
            .is_some_and(|status| status.active),
        (None, Some(_)) => false,
    }
}

fn flow_activity_items(app: &AppState) -> Vec<&ActivityItem> {
    let active_turn_id = app.active_turn().map(|(_, turn_id)| turn_id);
    app.activity
        .iter()
        .filter(|item| match active_turn_id {
            Some(turn_id) => item.turn_id.as_ref() == Some(turn_id),
            // When no turn is active, turn-less running sub-agent progress is
            // folded into the orchestrating turn's chip (as children) — don't
            // also render it here as a separate turn-less "Orchestrating" chip.
            None => item.turn_id.is_none() && !is_subagent_progress(app, item),
        })
        .collect()
}

/// A turn-less running sub-agent progress row (an `AgentUpdated` / spawn-complete
/// `Progress` item with no originating turn) that is ALSO represented by a
/// running sub-agent task. Such rows are surfaced under the orchestrating turn's
/// chip via `running_subagent_titles_for_chip`, so they must not also form their
/// own phantom turn-less "Orchestrating" chip (mini5 soak: the "two Orchestrating
/// chips" for one parallel-spawn turn).
///
/// codex P2: we only suppress when a matching running TASK exists. A turn-less
/// progress row with no matching task has nothing to fold into, so we keep it
/// visible in the flow rather than hiding it entirely (orphaned-from-view).
fn is_subagent_progress(app: &AppState, item: &ActivityItem) -> bool {
    if item.turn_id.is_some() || item.kind != ActivityKind::Progress || !is_running_activity(item) {
        return false;
    }
    app.active_session().is_some_and(|session| {
        session.tasks.iter().any(|task| {
            matches!(task_state_label(task.state), "pending" | "running")
                && task.title == item.title
        })
    })
}

/// The committed per-turn status report line, e.g.
/// `✻ Ran for 5m 19s · 2 background task(s) still running`. The `✻` glyph and
/// duration mirror the live working indicator; the trailing clause is dropped
/// when nothing was left running.
fn turn_summary_text(summary: &crate::model::TurnActivitySummary) -> String {
    let ran_for = t!(
        "app.turn_summary.ran_for",
        duration = format_elapsed_secs(summary.elapsed_secs)
    );
    if summary.background_tasks > 0 {
        let still_running = t!(
            "app.turn_summary.tasks_still_running",
            count = summary.background_tasks
        );
        format!("✻ {ran_for} · {still_running}")
    } else {
        format!("✻ {ran_for}")
    }
}

/// "Swirling galaxy" spinner frames: a spiral arm sweeps one full clockwise
/// revolution (6 arc frames), then the core glints (bright ✦ → fading ✧) —
/// at the 120ms tick in [`spinner_frame`] that is a 720ms swirl + a 240ms
/// sparkle per 960ms cycle. Every frame is exactly one terminal cell wide
/// (ambiguous-width-but-1 glyphs; same shipped precedent as ✻ / ⚠), which the
/// fixed marker layout math depends on.
const SPINNER_FRAMES: [&str; 8] = ["◜", "◠", "◝", "◞", "◡", "◟", "✦", "✧"];

/// Current spinner frame, advancing ~every 120ms off a process-lifetime clock
/// (independent of any turn timer, so it keeps animating while background
/// sub-agents run after the parent turn has finished). The event loop redraws
/// every ~25ms, so this reads as smooth motion.
fn spinner_frame() -> &'static str {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    SPINNER_FRAMES[(elapsed / 120) as usize % SPINNER_FRAMES.len()]
}

/// Seconds since process start — the same process clock `spinner_frame` rides,
/// so a wave keyed off it advances on every ~25ms animation redraw without
/// threading a phase counter through `AppState`.
fn anim_time_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Extract an RGB triple from a ratatui `Color`. Truecolor themes store
/// `Color::Rgb`; named/`Reset` colors (the Terminal theme) fall back to neutral
/// grey so the wave degrades to a subtle ripple rather than panicking.
fn rgb_of(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (170, 170, 170),
    }
}

/// Linear RGB lerp across gradient `stops`; `t` clamped to 0..=1.
fn gradient_sample(stops: &[(u8, u8, u8)], t: f32) -> (u8, u8, u8) {
    match stops {
        [] => (255, 255, 255),
        [only] => *only,
        _ => {
            let f = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
            let lo = f.floor() as usize;
            let hi = (lo + 1).min(stops.len() - 1);
            let frac = f - lo as f32;
            let (a, b) = (stops[lo], stops[hi]);
            let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * frac).round() as u8;
            (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
        }
    }
}

/// One `Span` per grapheme, each colored from a sine-driven sample point that
/// slides with `phase`, so a bright crest travels along `text` like a ripple.
/// Advances by DISPLAY columns (CJK/emoji are double-width) so the wave stays
/// even across multi-width glyphs; `bg` preserves the row's surface background.
fn wave_gradient_spans(
    text: &str,
    phase: f32,
    stops: &[(u8, u8, u8)],
    bg: Color,
) -> Vec<Span<'static>> {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    const K: f32 = 0.45; // radians per display column — ripple tightness
    let mut spans = Vec::new();
    let mut col = 0.0f32;
    for g in text.graphemes(true) {
        let wave = 0.5 + 0.5 * (col * K - phase).sin();
        let (r, gg, b) = gradient_sample(stops, wave);
        spans.push(Span::styled(
            g.to_string(),
            Style::default().fg(Color::Rgb(r, gg, b)).bg(bg),
        ));
        col += g.width().max(1) as f32;
    }
    spans
}

/// Title for an agent-task group chip. Pure so it can be unit-tested
/// directly (Gap 2 fix #2). The order of precedence is deliberate:
///
/// 1. `in_progress` (live tool calls or running sub-agents) → "Orchestrating".
/// 2. `pending_continuations > 0` AND `is_active_group` → "re-entering". The
///    parent's tool calls can all be settled while the server has a master
///    re-entry queued; the CURRENT turn's chip must NOT read "Agent task
///    completed" in that gap (the "looks done" lie).
///
///    Blocking bug 1: `pending_continuations` is the active SESSION's queued
///    count, not a per-group fact. It must only retitle the CURRENT/active
///    turn's group — never an ARCHIVED past-turn group (whose work is over and
///    is not the thing being continued). `is_active_group` gates this. For the
///    active group the continuation is the live truth, so it even wins over a
///    `failed` parent (the failure is what is being retried/continued).
/// 3. `failed > 0` → finished with errors (the only re-entry-beating outcome
///    for ARCHIVED groups; pending never applies there).
/// 4. otherwise → completed.
fn agent_task_group_title(
    in_progress: bool,
    failed: usize,
    pending_continuations: u32,
    is_active_group: bool,
) -> String {
    if in_progress {
        t!("app.activity.orchestrating").to_string()
    } else if is_active_group && pending_continuations > 0 {
        t!("app.activity.re_entering").to_string()
    } else if failed > 0 {
        t!("app.activity.finished_errors").to_string()
    } else {
        t!("app.activity.completed").to_string()
    }
}

/// Claude-Code-style display name for a tool (`bash` → `Bash`, `read_file` →
/// `Read`, …). Unknown tools get their first letter capitalized.
fn tool_display_name(title: &str) -> String {
    match title {
        "shell" | "exec" | "exec_command" | "bash" => "Bash".into(),
        "read_file" => "Read".into(),
        "write_file" => "Write".into(),
        "edit_file" | "diff_edit" => "Edit".into(),
        "list_dir" => "List".into(),
        "grep" | "grep_tool" => "Grep".into(),
        "glob" | "glob_tool" => "Glob".into(),
        "web_search" | "deep_search" => "Search".into(),
        "web_fetch" => "Fetch".into(),
        "spawn" => "Spawn".into(),
        "browser" => "Browser".into(),
        "message" => "Message".into(),
        "send_file" => "Send".into(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        }
    }
}

/// The `⏺` card bullet, colored by status: green when the tool succeeded, red
/// when it failed, and the animated spinner while it is still running.
fn tool_card_bullet(item: &ActivityItem, palette: Palette) -> (String, Style) {
    if is_running_activity(item) {
        (spinner_frame().to_string(), palette.selected())
    } else if activity_is_failed(item) {
        // Failures keep a distinct glyph (not just red) so they stay legible
        // without color; success drops the checkmark for the calmer `⏺`.
        ("✗".to_string(), Style::default().fg(palette.danger))
    } else if activity_is_completed(item) {
        ("⏺".to_string(), Style::default().fg(palette.success))
    } else {
        // interrupted / skipped / pending — neutral, never a false green success.
        ("⏺".to_string(), palette.muted())
    }
}

/// Leading indent for a tool card rendered as an agent-task-group CHILD:
/// the card is always emitted under a group header (`◠ Orchestrating…`), so
/// its bullet must nest instead of sitting flush at column 0 where it reads
/// as a sibling of the header. Two columns puts the `⏺`/spinner bullet at the
/// same tree level as the `⎿` connector of non-tool children.
const TOOL_CARD_CHILD_INDENT: &str = "  ";

fn compact_activity_spans(
    item: &ActivityItem,
    palette: Palette,
    content_budget: usize,
) -> Vec<Span<'static>> {
    if let Some(mutation) = FileMutationActivity::from_item(item) {
        // Activity rows render uniformly muted, no bold: the runtime log must
        // never outweigh the reply prose or the user's own words.
        // "preview ready" was dropped: the TUI exposes no action to open the
        // preview here, so the label was a dead affordance.
        return vec![
            Span::styled(
                file_mutation_action_label(&mutation.operation),
                palette.muted(),
            ),
            Span::styled(" ", palette.muted()),
            Span::styled(compact_file_path(&mutation.path), palette.muted()),
            Span::styled(format!("  {}", mutation.operation), palette.muted()),
        ];
    }

    // Tool activities render as Claude-Code cards via `push_tool_card_header`;
    // this path only handles non-tool rows (progress, generic).

    // A context-compaction notice is an infrequent, notable event — render it
    // prominently (accent + ✦) so it stands out from the muted activity stream
    // instead of scrolling by unseen in a busy multi-agent session.
    let compacted_title = t!("status.activity_context_compacted");
    if item.kind == ActivityKind::Progress && item.title.as_str() == compacted_title.as_ref() {
        let mut spans = vec![
            Span::styled("✦ ", Style::default().fg(palette.accent)),
            Span::styled(
                item.title.clone(),
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {}", item.status), palette.muted()),
        ];
        // Reserve the trailing metadata (duration) width up front so the
        // detail is truncated to fit BEFORE it, keeping the duration visible.
        let mut meta = Vec::new();
        push_compact_metadata_spans(&mut meta, palette, item);
        if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
            spans.push(Span::styled("  ", palette.muted()));
            let detail_budget = remaining_content_budget(content_budget, &spans, &meta);
            spans.push(Span::styled(
                truncate_to_display_width(detail, detail_budget),
                palette.muted(),
            ));
        }
        spans.extend(meta);
        return spans;
    }

    let mut spans = vec![
        Span::styled(item.title.clone(), palette.muted()),
        Span::styled(format!("  {}", item.status), palette.muted()),
    ];
    let mut meta = Vec::new();
    push_compact_metadata_spans(&mut meta, palette, item);
    if let Some(detail) = item.detail.as_deref().filter(|detail| !detail.is_empty()) {
        spans.push(Span::styled("  ", palette.muted()));
        let detail_budget = remaining_content_budget(content_budget, &spans, &meta);
        spans.push(Span::styled(
            truncate_to_display_width(detail, detail_budget),
            palette.muted(),
        ));
    }
    spans.extend(meta);
    spans
}

/// Display columns still available for a row's variable part, given the total
/// `content_budget`, the fixed leading spans already built, and the trailing
/// metadata spans reserved after it. Saturating so an over-tight budget yields
/// 0 (the variable part vanishes) rather than underflowing.
fn remaining_content_budget(
    content_budget: usize,
    leading: &[Span<'static>],
    trailing: &[Span<'static>],
) -> usize {
    let used: usize = leading
        .iter()
        .chain(trailing.iter())
        .map(|span| span.content.as_ref().width())
        .sum();
    content_budget.saturating_sub(used)
}

/// Tally the agent-task-group counts over a slice of activity items.
///
/// Returns `(total, completed, active, failed)` using the SAME predicates the
/// chip header and footer already use ([`activity_is_completed`],
/// [`is_running_activity`], [`activity_is_failed`]).
///
/// The chip header MUST tally over the FULL turn activity set, not the
/// display-capped slice of children that's actually rendered — otherwise a
/// 66-action turn showing the last 3 rows reads "3 action(s) · 3 completed"
/// while its own "... +63 older action(s)" footer proves the real total is 66.
/// Both the header and the footer call this single helper so their numbers
/// cannot diverge.
fn task_group_counts(full_items: &[&ActivityItem]) -> (usize, usize, usize, usize) {
    let total = full_items.len();
    let completed = full_items
        .iter()
        .filter(|item| activity_is_completed(item))
        .count();
    let active = full_items
        .iter()
        .filter(|item| is_running_activity(item))
        .count();
    let failed = full_items
        .iter()
        .filter(|item| activity_is_failed(item))
        .count();
    (total, completed, active, failed)
}

/// The single-variant diff-preview status the server always sends today
/// (`DiffPreviewGetStatus::Ready`). It carries no information, so it is
/// suppressed from the header; any other value is surfaced.
fn is_default_diff_status(status: &str) -> bool {
    status == "ready"
}

/// The single-variant diff-preview source the server always sends today
/// (`DiffPreviewSource::PendingStore`) — an internal implementation detail.
/// Suppressed from the header; any other value is surfaced.
fn is_default_diff_source(source: &str) -> bool {
    source == "pending_store"
}

/// One aligned side-by-side row: old file's line on the left, new file's on
/// the right. `None` = blank half (a removed line with no added counterpart,
/// or vice versa).
type SideBySideRow<'a> = (
    Option<&'a crate::model::DiffPreviewLine>,
    Option<&'a crate::model::DiffPreviewLine>,
);

/// Pair already-parsed unified hunk lines into aligned side-by-side rows:
/// context appears on both sides, a removed run pairs row-by-row with the
/// added run it abuts, and surplus removed/added lines keep a blank opposite
/// column. Reuses `DiffPreviewLine` — no re-parsing.
fn side_by_side_rows(lines: &[crate::model::DiffPreviewLine]) -> Vec<SideBySideRow<'_>> {
    fn flush_changes<'a>(
        rows: &mut Vec<SideBySideRow<'a>>,
        removed: &mut Vec<&'a crate::model::DiffPreviewLine>,
        added: &mut Vec<&'a crate::model::DiffPreviewLine>,
    ) {
        for idx in 0..removed.len().max(added.len()) {
            rows.push((removed.get(idx).copied(), added.get(idx).copied()));
        }
        removed.clear();
        added.clear();
    }

    let mut rows = Vec::with_capacity(lines.len());
    let mut removed: Vec<&crate::model::DiffPreviewLine> = Vec::new();
    let mut added: Vec<&crate::model::DiffPreviewLine> = Vec::new();
    for line in lines {
        // Kind aliases mirror `diff_preview_line_is_change` (model.rs).
        match line.kind.as_str() {
            "removed" | "delete" | "deleted" => removed.push(line),
            "added" | "insert" | "inserted" => added.push(line),
            _ => {
                flush_changes(&mut rows, &mut removed, &mut added);
                rows.push((Some(line), Some(line)));
            }
        }
    }
    flush_changes(&mut rows, &mut removed, &mut added);
    rows
}

/// Minimum line-number gutter width in a side-by-side half (matches the
/// unified view's `{n:>4}` gutter).
const SIDE_BY_SIDE_MIN_GUTTER: usize = 4;

/// Line-number gutter width for one hunk's side-by-side rows: wide enough
/// for the LARGEST line number either side shows, never below
/// [`SIDE_BY_SIDE_MIN_GUTTER`]. Computed per hunk — a fixed `{n:>4}` gutter
/// silently widened only the rows with numbers >= 10000, shifting that
/// row's separator out of column and breaking the hunk's shared-row
/// alignment (#362 review).
fn side_by_side_gutter_width(lines: &[crate::model::DiffPreviewLine]) -> usize {
    lines
        .iter()
        .filter_map(|line| line.old_line.max(line.new_line))
        .max()
        .map(|max| max.to_string().len())
        .unwrap_or(SIDE_BY_SIDE_MIN_GUTTER)
        .max(SIDE_BY_SIDE_MIN_GUTTER)
}

/// Fixed columns in a side-by-side row besides the two content cells, for a
/// given line-number gutter width: 4 indent + (gutter + 1 space + sign + 1
/// space) per half + 3 separator.
fn side_by_side_chrome_cols(gutter: usize) -> usize {
    4 + 2 * (gutter + 3) + 3
}

/// Fit `content` into exactly `cell` display columns: truncate with a
/// trailing `…` when too wide (no horizontal scroll in v1), pad with spaces
/// when narrower so the column separator stays aligned. UTF-8/display-width
/// safe (a wide char never straddles the cell boundary).
///
/// Sanitizes (tabs -> 4 spaces, other control chars stripped) BEFORE
/// measuring — the same order `insert_history` and `finish_hanging_body`
/// use. The finalized-scrollback flush runs `sanitize_line_in_place` AFTER
/// this cell has been padded to exact width, so measuring a raw `\t` would
/// let the row grow four columns per tab at insert time, hard-wrap in native
/// scrollback, and permanently misalign the old|new separator.
fn fit_diff_cell(content: &str, cell: usize) -> String {
    let content: std::borrow::Cow<'_, str> = if content.chars().any(char::is_control) {
        std::borrow::Cow::Owned(crate::insert_history::sanitize_span_content(content))
    } else {
        std::borrow::Cow::Borrowed(content)
    };
    let content = content.as_ref();
    let width = UnicodeWidthStr::width(content);
    if width <= cell {
        let mut out = String::with_capacity(content.len() + (cell - width));
        out.push_str(content);
        out.push_str(&" ".repeat(cell - width));
        return out;
    }
    let budget = cell.saturating_sub(1); // room for the ellipsis marker
    let mut out = String::new();
    let mut used = 0usize;
    for ch in content.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > budget {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    // A wide char stopping short of the boundary leaves the cell a column
    // narrow; pad so the separator stays aligned.
    out.push_str(&" ".repeat(cell.saturating_sub(used + 1)));
    out
}

fn diff_file_line_counts(file: &crate::model::DiffPreviewFile) -> (usize, usize) {
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .fold((0, 0), |(added, removed), line| match line.kind.as_str() {
            "added" | "insert" | "inserted" => (added + 1, removed),
            "removed" | "delete" | "deleted" => (added, removed + 1),
            _ => (added, removed),
        })
}

fn diff_file_type_badge(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "rs" => "RUST",
        "toml" => "TOML",
        "json" => "JSON",
        "yaml" | "yml" => "YAML",
        "md" | "markdown" => "MD",
        "js" | "jsx" => "JS",
        "ts" | "tsx" => "TS",
        "css" | "scss" | "sass" => "CSS",
        "html" | "htm" => "HTML",
        "sh" | "bash" | "zsh" => "SH",
        "py" => "PY",
        _ => "FILE",
    }
}

fn diff_file_badge_style(badge: &str, palette: Palette) -> Style {
    let fg = match badge {
        "RUST" => palette.danger,
        "TOML" | "JSON" | "YAML" => palette.highlight,
        "MD" => palette.text,
        "JS" | "TS" => palette.accent,
        "CSS" | "HTML" => palette.accent,
        "SH" | "PY" => palette.success,
        _ => palette.muted,
    };
    Style::default()
        .fg(fg)
        .bg(palette.diff_context_bg)
        .add_modifier(Modifier::BOLD)
}

fn shell_command_from_line(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("$ ")
        .or_else(|| trimmed.strip_prefix("command: "))
        .filter(|command| !command.trim().is_empty())
}

fn active_background_tasks(app: &AppState) -> usize {
    app.active_session()
        .map(|session| {
            session
                .tasks
                .iter()
                .filter(|task| matches!(task_state_label(task.state), "pending" | "running"))
                .count()
        })
        .unwrap_or(0)
}

/// Titles of the running sub-agents attributed to an agent-task chip. Each
/// running task is attributed to the chip for its OWN originating turn
/// (`task.turn_id`, stamped by the server per C1 step 4), so two turns can no
/// longer both claim the same global sub-agent count — the "two Orchestrating
/// chips" bug (C5). Background sub-agents outlive the parent turn (it shows
/// "done" while they keep running), and that still works: their `turn_id` keeps
/// pointing at the turn that spawned them, so that — and only that — chip stays
/// "Orchestrating", and lists those agents as its children (so their live
/// progress no longer forms a *second*, turn-less "Orchestrating" chip).
///
/// Tasks the server couldn't stamp with a turn (legacy daemons, `session/open`
/// replay, synthetic emitters → `turn_id == None`) fall back to a SINGLE current
/// chip — the active (live) turn if one exists, else the latest activity-log
/// turn — so they still surface without being double-counted across chips.
fn running_subagent_titles_for_chip(
    app: &AppState,
    turn_id: Option<&octos_core::ui_protocol::TurnId>,
) -> Vec<String> {
    let Some(chip_turn) = turn_id else {
        return Vec::new();
    };
    let Some(session) = app.active_session() else {
        return Vec::new();
    };
    // The one chip that owns turn-less tasks: prefer the active (live) turn; if
    // the turn already finished, this session's latest activity-log turn. At most
    // one chip is ever "current", so unattributed tasks are counted exactly once.
    // Scope the log lookup to the active session (codex P2): `turn_activity_logs`
    // is cross-session, and the tasks we count belong to `session`, so a newer
    // log in a *different* session must not steal this session's fallback chip.
    let current_turn = app.active_turn().map(|(_, t)| t).or_else(|| {
        app.turn_activity_logs
            .iter()
            .rev()
            .find(|log| log.session_id == session.id)
            .map(|log| &log.turn_id)
    });
    let owns_unattributed = current_turn == Some(chip_turn);
    session
        .tasks
        .iter()
        .filter(|task| matches!(task_state_label(task.state), "pending" | "running"))
        .filter(|task| match task.turn_id.as_ref() {
            Some(task_turn) => task_turn == chip_turn,
            None => owns_unattributed,
        })
        .map(|task| task.title.clone())
        .collect()
}

fn plan_step_text_spans(text: &str, palette: Palette) -> Vec<Span<'static>> {
    inline_markdown_spans(
        text,
        palette.text(),
        palette.title().add_modifier(Modifier::BOLD),
        palette.selected(),
    )
}

fn plain_inline_markdown(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        // Mirror the link/strikethrough rendering exactly so the measured width
        // equals what `inline_markdown_spans` draws — otherwise a link in a
        // table cell sizes the column by the raw `[text](url)` markup and can
        // shrink/ellipsize unrelated columns (issue #207).
        if let Some((link_text, url, consumed)) = parse_markdown_link(rest) {
            if link_text == url {
                output.push_str(url);
            } else {
                output.push_str(link_text);
                output.push_str(&format!(" ({url})"));
            }
            rest = &rest[consumed..];
            continue;
        }
        if let Some((struck, consumed)) = parse_markdown_strikethrough(rest) {
            output.push_str(struck);
            rest = &rest[consumed..];
            continue;
        }
        if let Some(after_open) = rest.strip_prefix("**")
            && let Some(close) = after_open.find("**")
        {
            output.push_str(&after_open[..close]);
            rest = &after_open[close + 2..];
            continue;
        }
        if let Some(after_open) = rest.strip_prefix('`')
            && let Some(close) = after_open.find('`')
        {
            output.push_str(&after_open[..close]);
            rest = &after_open[close + 1..];
            continue;
        }
        if let Some((emphasis, consumed)) = markdown_emphasis_segment(rest) {
            output.push_str(emphasis);
            rest = &rest[consumed..];
            continue;
        }
        if let Some(ch) = rest.chars().next() {
            output.push(ch);
            rest = &rest[ch.len_utf8()..];
        } else {
            break;
        }
    }
    output
}

fn extract_plan_lines(app: &AppState) -> Vec<RenderedPlanStep> {
    let mut plan = extract_plan_steps(app);
    normalize_rendered_plan_steps(&mut plan);
    apply_completed_plan_steps_from_history(app, &mut plan);
    plan
}

fn normalize_rendered_plan_steps(plan: &mut [RenderedPlanStep]) {
    for step in plan {
        while let Some((completed, rest)) = strip_leading_plan_checkbox(&step.text) {
            step.completed |= completed;
            step.text = rest.to_string();
        }
    }
}

fn apply_completed_plan_steps_from_history(app: &AppState, plan: &mut [RenderedPlanStep]) {
    if plan.iter().all(|step| step.completed) {
        return;
    }
    let Some(session) = app.active_session() else {
        return;
    };

    let completed_steps = session
        .messages
        .iter()
        .rev()
        .filter(|message| message.role.as_str() == "assistant")
        .flat_map(|message| completed_plan_texts(message.content.as_str()))
        .collect::<Vec<_>>();

    for step in plan.iter_mut().filter(|step| !step.completed) {
        if completed_steps
            .iter()
            .any(|completed| normalize_plan_text(completed) == normalize_plan_text(&step.text))
        {
            step.completed = true;
        }
    }
}

fn completed_plan_texts(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(completed_plan_text_from_line)
        .collect()
}

fn completed_plan_text_from_line(line: &str) -> Option<String> {
    let mut rest = line.trim();
    let mut completed = false;
    let mut saw_marker = false;

    for _ in 0..6 {
        rest = rest.trim_start();
        if let Some((checked, next)) = strip_leading_plan_checkbox(rest) {
            completed |= checked;
            saw_marker = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_leading_plan_bullet(rest) {
            saw_marker = true;
            rest = next;
            continue;
        }
        if let Some(next) = strip_leading_plan_number(rest) {
            saw_marker = true;
            rest = next;
            continue;
        }
        break;
    }

    let text = rest.trim_start_matches(['.', ')', ' ']).trim();
    (completed && saw_marker && !text.is_empty()).then(|| text.to_string())
}

fn strip_leading_plan_checkbox(line: &str) -> Option<(bool, &str)> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (marker, rest) = rest.split_once(']')?;
    let completed = match marker.trim() {
        "x" | "X" => true,
        "" => false,
        _ => return None,
    };
    Some((completed, rest.trim_start()))
}

fn strip_leading_plan_bullet(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
}

fn strip_leading_plan_number(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let split = trimmed.find(['.', ')'])?;
    let (number, rest) = trimmed.split_at(split);
    if number.is_empty() || number.len() > 3 || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = rest[1..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

fn normalize_plan_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

struct ComposerInputView {
    lines: Vec<String>,
    hidden_lines: usize,
    hidden_prefix: bool,
    cursor_row: u16,
    cursor_width: usize,
}

/// Max width of a per-loop chip label before truncation. Keeps the
/// indicator row compact when several loops are running concurrently.
const AUTONOMY_LOOP_LABEL_MAX: usize = 20;

/// Returns the active session's autonomy mirror, or `None` if either no
/// session is selected or the backend has not yet populated the mirror.
fn active_session_autonomy(app: &AppState) -> Option<&SessionAutonomyState> {
    let session = app.active_session()?;
    app.session_autonomy_for(&session.id)
}

/// Whether the active session currently has a goal in its autonomy mirror.
/// Gates the Ctrl+P fold toggle so the key is only claimed when the ◆ Goal
/// banner is actually showing (otherwise it falls through, unswallowed).
pub(crate) fn active_session_has_goal(app: &AppState) -> bool {
    active_session_autonomy(app).is_some_and(|state| state.goal.is_some())
}

/// Number of rows the sticky autonomy indicator needs: 0 when both goal
/// and loops are absent, 1 when only one is present, 2 when both are.
/// Max plan items rendered in the sticky panel before collapsing to a
/// `… +N more` summary line, so a long checklist can't dominate the screen.
const PLAN_PANEL_MAX_ITEMS: usize = 8;

/// Rows the plan checklist adds to the sticky panel: a header line plus one
/// row per shown item (capped), plus a `+N more` line when truncated.
fn plan_panel_rows(plan: &octos_core::ui_protocol::UiPlanRecord) -> u16 {
    if plan.items.is_empty() {
        return 0;
    }
    let shown = plan.items.len().min(PLAN_PANEL_MAX_ITEMS);
    let overflow = usize::from(plan.items.len() > PLAN_PANEL_MAX_ITEMS);
    (1 + shown + overflow) as u16
}

/// Wrap a goal objective into up to [`GOAL_OBJECTIVE_MAX_ROWS`] display chunks so
/// the banner shows the WHOLE goal (not a single clipped line — the user's raw
/// `/goal` text can be hundreds of chars). Char-chunked at a nominal width (exact
/// column wrapping needs the render width, which the height reservation can't
/// see); a trailing "…" marks an objective longer than the cap. Shared by the
/// height reservation and the render so they always agree on row count.
///
/// The cap is generous (≈ 20 rows × 56 chars ≈ 1.1k chars) so a realistic
/// extensive `/goal` prompt renders in FULL — a 3-row cap (the first pass) still
/// clipped long objectives with a "…", which users reported. The ceiling exists
/// only so a pathological multi-KB objective can't shove the composer off screen
/// (the overall live-UI height clamp bounds it further).
const GOAL_OBJECTIVE_MAX_ROWS: usize = 20;
/// Wrapping floor: even on a very narrow terminal the objective wraps at a sane
/// minimum rather than collapsing toward one char per row.
const GOAL_OBJECTIVE_MIN_WIDTH: usize = 24;

/// Width available for objective text: the render area width minus the banner's
/// glyph/prefix gutter (`{glyph} ` on row 1) or the matching continuation indent
/// — both are `goal_prefix + 2` columns. Threading the real width in (rather than
/// the old fixed 56) lets the objective use the FULL terminal width; the height
/// reservation and the render call this with the same width so their row counts
/// stay in lock-step.
fn goal_objective_body_width(width: u16) -> usize {
    let indent = t!("app.autonomy.goal_prefix").chars().count() + 2;
    (width as usize)
        .saturating_sub(indent)
        .max(GOAL_OBJECTIVE_MIN_WIDTH)
}

/// The status/budget parenthetical trailing the objective (e.g.
/// "(active · 0K/2000K tokens)"). Built in ONE place so the height reservation
/// and the render agree on its width when deciding whether it fits the last row.
fn goal_meta_parenthetical(goal: &octos_core::ui_protocol::UiGoalRecord) -> String {
    let (_, status_label) = goal_status_display(&goal.status);
    t!(
        "app.autonomy.goal_meta",
        status = status_label,
        used = format_tokens_k(goal.tokens_used),
        budget = format_tokens_k(goal.token_budget)
    )
    .into_owned()
}

/// Wrap a goal objective into up to [`GOAL_OBJECTIVE_MAX_ROWS`] display chunks at
/// the given render `width`. `tail_len` is the trailing parenthetical's column
/// count: when the objective fits within the cap but the parenthetical wouldn't
/// fit after the final row, an empty trailing chunk is appended so the
/// parenthetical renders on its own indented line instead of being clipped off
/// the right edge. Shared by the height reservation and the render so they always
/// agree on the row count.
fn goal_objective_chunks(objective: &str, width: u16, tail_len: usize) -> Vec<String> {
    let objective = objective.trim();
    if objective.is_empty() {
        return Vec::new();
    }
    let body = goal_objective_body_width(width);
    let chars: Vec<char> = objective.chars().collect();
    let mut chunks: Vec<String> = chars
        .chunks(body)
        .take(GOAL_OBJECTIVE_MAX_ROWS)
        .map(|c| c.iter().collect())
        .collect();
    if chars.len() > GOAL_OBJECTIVE_MAX_ROWS * body {
        // Objective longer than the cap: mark the clip. The parenthetical rides
        // the (full) last row; the cap already bounds height.
        if let Some(last) = chunks.last_mut() {
            last.push('…');
        }
    } else if tail_len > 0 {
        // Objective fits: keep the status/budget parenthetical fully on-screen —
        // if it won't fit after the final objective row, give it its own indented
        // line (only while row budget remains).
        let last_len = chunks.last().map(|c| c.chars().count()).unwrap_or(0);
        if last_len + 1 + tail_len > body && chunks.len() < GOAL_OBJECTIVE_MAX_ROWS {
            chunks.push(String::new());
        }
    }
    chunks
}

/// Auto-fold threshold: a goal whose objective wraps to MORE than this many rows
/// at the render width is folded to one compact row by DEFAULT (Ctrl+P expands),
/// so a huge pasted objective can't dominate the banner. A 1–3 row goal shows in
/// full — short goals never look truncated. Only consulted while the fold
/// preference is [`GoalObjectiveFold::Auto`]; an explicit Ctrl+P choice wins.
const GOAL_FOLD_AUTO_MAX_ROWS: usize = 3;

/// Minimum columns the folded preview keeps even on a narrow terminal, so a
/// sliver of the objective is always legible before the `…`.
const GOAL_FOLD_PREVIEW_MIN: usize = 8;

/// Resolve the EFFECTIVE fold for the goal objective and record it on `app` so
/// Ctrl+P ([`AppState::toggle_goal_objective_fold`]) can flip whatever is on
/// screen. `Auto` folds a long objective (wraps beyond
/// [`GOAL_FOLD_AUTO_MAX_ROWS`] rows at `width`) and shows a short one in full; an
/// explicit fold choice always wins. Both the height reservation and the render
/// call this with the SAME width, so their fold decision — hence their row count
/// — always agree (the banner's reserve==render discipline).
fn goal_objective_folded(app: &AppState, objective: &str, width: u16) -> bool {
    let folded = match app.goal_objective_fold {
        GoalObjectiveFold::Folded => true,
        GoalObjectiveFold::Unfolded => false,
        GoalObjectiveFold::Auto => {
            goal_objective_chunks(objective, width, 0).len() > GOAL_FOLD_AUTO_MAX_ROWS
        }
    };
    app.goal_objective_folded_effective.set(folded);
    folded
}

fn autonomy_indicator_height(app: &AppState, width: u16) -> u16 {
    match active_session_autonomy(app) {
        Some(state) => {
            let mut rows = 0u16;
            if let Some(goal) = state.goal.as_ref() {
                // Folded: exactly ONE compact row (glyph + preview + parenthetical
                // + hint). Unfolded: at least one row (glyph + status even when the
                // objective is empty), otherwise the wrapped-objective row count.
                // MUST use the same fold decision + width + parenthetical length as
                // the render so the reserved height matches the rendered rows
                // exactly (reserve==render).
                let obj_rows = if goal_objective_folded(app, &goal.objective, width) {
                    1
                } else {
                    let tail = goal_meta_parenthetical(goal).chars().count();
                    goal_objective_chunks(&goal.objective, width, tail)
                        .len()
                        .max(1)
                };
                rows += obj_rows as u16;
            }
            if state.loops.iter().any(autonomy_loop_is_active) {
                rows += 1;
            }
            if let Some(plan) = state.plan.as_ref() {
                rows += plan_panel_rows(plan);
            }
            rows
        }
        None => 0,
    }
}

/// Trim a loop's prompt down to a chip-sized label. Prefers the first
/// line for legibility; falls back to a UTF-8 safe char-boundary cut at
/// [`AUTONOMY_LOOP_LABEL_MAX`].
fn autonomy_loop_label(record: &octos_core::ui_protocol::UiLoopRecord) -> String {
    let prompt = record.prompt.trim();
    if prompt.is_empty() {
        return record
            .loop_id
            .chars()
            .take(AUTONOMY_LOOP_LABEL_MAX)
            .collect();
    }
    let first_line = prompt.lines().next().unwrap_or(prompt).trim();
    if first_line.chars().count() <= AUTONOMY_LOOP_LABEL_MAX {
        first_line.to_string()
    } else {
        let mut truncated: String = first_line
            .chars()
            .take(AUTONOMY_LOOP_LABEL_MAX.saturating_sub(1))
            .collect();
        truncated.push('…');
        truncated
    }
}

/// Format the cadence prefix for a loop chip (e.g. `5m`, `2h`,
/// `self-paced`, `maintenance`). Unknown modes pass through verbatim.
fn autonomy_loop_cadence(record: &octos_core::ui_protocol::UiLoopRecord) -> String {
    match record.mode.as_str() {
        "fixed_interval" => match record.interval_seconds {
            Some(secs) if secs >= 3600 && secs % 3600 == 0 => format!("{}h", secs / 3600),
            Some(secs) if secs >= 60 && secs % 60 == 0 => format!("{}m", secs / 60),
            Some(secs) => format!("{secs}s"),
            None => "interval".to_string(),
        },
        "self_paced" => "self-paced".to_string(),
        "maintenance" => "maintenance".to_string(),
        other => other.to_string(),
    }
}

/// True when a loop is in the runnable `"active"` state. Paused / deleted
/// loops still appear in the chip row but are dimmed.
fn autonomy_loop_is_active(record: &octos_core::ui_protocol::UiLoopRecord) -> bool {
    record.status == "active"
}

/// Build the line set for the sticky autonomy indicator. Returns 0, 1,
/// or 2 lines (goal first, then loops).
/// Render a raw token count in K units for the goal chip: 174_763 →
/// "175K", 2_000_000 → "2000K", 0 → "0K". Rounded to the nearest thousand
/// so the goal budget reads at a glance instead of as a raw 6–9 digit
/// number (user request: "tui should display in K unit"). Rounds without
/// the overflow that `saturating_add(500)` would hit near `u64::MAX`.
fn format_tokens_k(tokens: u64) -> String {
    let k = tokens / 1_000 + u64::from(tokens % 1_000 >= 500);
    format!("{k}K")
}

/// Human-readable token count for context-window display: `128K`, `256K`,
/// `1M`, `1.5M`. Reuses [`format_tokens_k`] below 1M; switches to `M` above so
/// a 1,000,000-token window renders `1M` rather than `1000K`.
pub(crate) fn format_tokens_human(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens as f64 / 1_000_000.0;
        let rendered = format!("{millions:.1}");
        let rendered = rendered
            .strip_suffix(".0")
            .map(str::to_owned)
            .unwrap_or(rendered);
        format!("{rendered}M")
    } else {
        format_tokens_k(tokens)
    }
}

/// Per-status glyph + localized label for the goal chip: every status the
/// server can report renders distinctly (#329) — active ◆, paused ⏸,
/// budget-limited ⚠, blocked ⛔ (the #1693 circuit breaker), complete ✔.
/// Unknown statuses fall back to the raw string so a newer server never
/// renders blank.
fn goal_status_display(status: &str) -> (&'static str, String) {
    match status {
        "active" => ("◆", t!("app.autonomy.status_active").into_owned()),
        "paused" => ("⏸", t!("app.autonomy.status_paused").into_owned()),
        "budget_limited" => ("⚠", t!("app.autonomy.status_budget_limited").into_owned()),
        "blocked" => ("⛔", t!("app.autonomy.status_blocked").into_owned()),
        "complete" => ("✔", t!("app.autonomy.status_complete").into_owned()),
        other => ("◆", other.to_owned()),
    }
}

fn autonomy_indicator_lines(app: &AppState, palette: Palette, width: u16) -> Vec<Line<'static>> {
    let Some(state) = active_session_autonomy(app) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(goal) = state.goal.as_ref() {
        let (glyph, _status_label) = goal_status_display(&goal.status);
        let parenthetical = goal_meta_parenthetical(goal);
        // Folded (default for a long objective, or after Ctrl+P): ONE compact
        // row. The fold decision MUST match `autonomy_indicator_height` — both
        // call `goal_objective_folded` with the same width (reserve==render).
        // Loops/plan rows still render below, exactly as in the unfolded case.
        if goal_objective_folded(app, &goal.objective, width) {
            lines.push(goal_folded_line(
                goal,
                glyph,
                &parenthetical,
                palette,
                width,
            ));
        } else {
            // The objective wraps across up to GOAL_OBJECTIVE_MAX_ROWS lines at
            // the FULL render width so the whole goal is visible (a raw `/goal`
            // request can be hundreds of chars). Row count here MUST match
            // `autonomy_indicator_height`'s reservation — both derive from
            // `goal_objective_chunks` with the same width + parenthetical length.
            let mut chunks =
                goal_objective_chunks(&goal.objective, width, parenthetical.chars().count());
            if chunks.is_empty() {
                chunks.push(goal.goal_id.clone());
            }
            let last = chunks.len() - 1;
            let indent = " ".repeat(t!("app.autonomy.goal_prefix").chars().count() + 2);
            for (idx, chunk) in chunks.into_iter().enumerate() {
                let mut spans = Vec::new();
                if idx == 0 {
                    spans.push(Span::styled(
                        format!("{glyph} "),
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD)
                            .bg(palette.surface),
                    ));
                    spans.push(Span::styled(
                        t!("app.autonomy.goal_prefix").to_string(),
                        palette.title().bg(palette.surface),
                    ));
                } else {
                    spans.push(Span::styled(
                        indent.clone(),
                        palette.text().bg(palette.surface),
                    ));
                }
                spans.push(Span::styled(chunk, palette.text().bg(palette.surface)));
                // The status/budget parenthetical rides the FINAL objective line.
                if idx == last {
                    spans.push(Span::styled(
                        parenthetical.clone(),
                        palette.muted().bg(palette.surface),
                    ));
                }
                lines.push(Line::from(spans));
            }
        }
    }
    // The loops row shows only while something is actually FIRING: a
    // paused-only session must not pin a permanent banner above the composer
    // (user report: long-parked test loops kept a "0 active · 3 paused" row
    // forever). Paused loops stay discoverable via the status-bar chip and
    // `/loop`; once at least one loop is active, paused siblings still render
    // here (muted chips + the paused suffix) so the header reconciles.
    if state.loops.iter().any(autonomy_loop_is_active) {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let running = state
            .loops
            .iter()
            .filter(|l| autonomy_loop_is_active(l))
            .count();
        let paused = state.loops.iter().filter(|l| l.status == "paused").count();
        spans.push(Span::styled(
            "↻ ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ));
        let mut loops_label = t!("app.autonomy.loops_running", count = running).to_string();
        if paused > 0 {
            loops_label.push_str(&t!("app.autonomy.loops_paused_suffix", count = paused));
        }
        spans.push(Span::styled(
            loops_label,
            palette.title().bg(palette.surface),
        ));
        spans.push(Span::styled("   ", palette.text().bg(palette.surface)));
        for record in &state.loops {
            let label = autonomy_loop_label(record);
            let cadence = autonomy_loop_cadence(record);
            let chip = format!("[{cadence} {label}]");
            let chip_style = if autonomy_loop_is_active(record) {
                palette.text().bg(palette.surface)
            } else {
                palette.muted().bg(palette.surface)
            };
            spans.push(Span::styled(chip, chip_style));
            spans.push(Span::styled(" ", palette.text().bg(palette.surface)));
        }
        // Drop the trailing space for tidiness.
        if matches!(spans.last(), Some(s) if s.content == " ") {
            spans.pop();
        }
        lines.push(Line::from(spans));
    }
    if let Some(plan) = state.plan.as_ref() {
        lines.extend(plan_indicator_lines(plan, palette));
    }
    lines
}

/// Render the ◆ Goal banner folded to ONE compact row:
/// `{glyph} Goal: {preview}… {(status · used/budget tokens)} · Ctrl+P expand`.
/// Used when the objective is folded (default for a long objective, or after
/// Ctrl+P). Always exactly one line, matching `autonomy_indicator_height`'s
/// folded reservation of a single row (reserve==render). The banner Paragraph
/// CLIPS rather than wraps, so the preview is budgeted to leave room for the
/// parenthetical and the hint — a long objective is truncated, its status/budget
/// and the expand hint stay on-screen.
fn goal_folded_line(
    goal: &octos_core::ui_protocol::UiGoalRecord,
    glyph: &str,
    parenthetical: &str,
    palette: Palette,
    width: u16,
) -> Line<'static> {
    let prefix = t!("app.autonomy.goal_prefix");
    let hint = t!("app.autonomy.goal_fold_hint");
    // Reserve the fixed columns (glyph+space, prefix, `…`, parenthetical, hint)
    // so the objective preview — not the trailing status/hint — is what gets
    // truncated when the goal is long.
    let reserved = prefix.chars().count()
        + 2 // "{glyph} "
        + 1 // the trailing "…"
        + parenthetical.chars().count()
        + hint.chars().count();
    let budget = (width as usize)
        .saturating_sub(reserved)
        .max(GOAL_FOLD_PREVIEW_MIN);
    let first_line = goal.objective.trim().lines().next().unwrap_or("").trim();
    let mut preview: String = first_line.chars().take(budget).collect();
    // Ellipsis when the preview doesn't show the whole objective (truncated
    // first line, or there is more than one line).
    let truncated = preview.chars().count() < first_line.chars().count()
        || goal.objective.trim().lines().nth(1).is_some();
    // Drop trailing whitespace so `word …` reads cleanly.
    while preview.ends_with(char::is_whitespace) {
        preview.pop();
    }
    if preview.is_empty() {
        // Objective empty (or all whitespace): fall back to the goal id so the
        // row is never a bare glyph, mirroring the unfolded empty-objective case.
        preview = goal.goal_id.clone();
    }
    let mut spans = vec![
        Span::styled(
            format!("{glyph} "),
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ),
        Span::styled(prefix.to_string(), palette.title().bg(palette.surface)),
        Span::styled(preview, palette.text().bg(palette.surface)),
    ];
    if truncated {
        spans.push(Span::styled("…", palette.text().bg(palette.surface)));
    }
    // `parenthetical` already carries a leading space (`" (…)"`); the hint carries
    // its own ` · ` separator — so they read `… (active · …) · Ctrl+P expand`.
    spans.push(Span::styled(
        parenthetical.to_string(),
        palette.muted().bg(palette.surface),
    ));
    spans.push(Span::styled(
        hint.to_string(),
        palette.muted().bg(palette.surface),
    ));
    Line::from(spans)
}

/// Render the model-authored plan/todo checklist as a header line
/// (`✶ <activity> (done/total)`) plus a `⎿`-anchored tree of items with a
/// per-status glyph. Mirrors the sub-agent task-group tree visual.
fn plan_indicator_lines(
    plan: &octos_core::ui_protocol::UiPlanRecord,
    palette: Palette,
) -> Vec<Line<'static>> {
    use octos_core::ui_protocol::PlanItemStatus;
    if plan.items.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let total = plan.items.len();
    let done = plan
        .items
        .iter()
        .filter(|item| item.status == PlanItemStatus::Completed)
        .count();
    // Header: prefer the model's activity label, else the in-progress item,
    // else a generic fallback.
    let title = plan
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            plan.items
                .iter()
                .find(|item| item.status == PlanItemStatus::InProgress)
                .map(|item| item.title.clone())
        })
        .unwrap_or_else(|| "Plan".to_string());
    lines.push(Line::from(vec![
        Span::styled(
            "✶ ",
            Style::default()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD)
                .bg(palette.surface),
        ),
        Span::styled(title, palette.title().bg(palette.surface)),
        Span::styled(
            format!("  ({done}/{total})"),
            palette.muted().bg(palette.surface),
        ),
    ]));
    for (idx, item) in plan.items.iter().take(PLAN_PANEL_MAX_ITEMS).enumerate() {
        let (glyph, glyph_style) = match item.status {
            PlanItemStatus::Completed => (
                "✔",
                Style::default().fg(palette.success).bg(palette.surface),
            ),
            PlanItemStatus::InProgress => (
                "▸",
                Style::default()
                    .fg(palette.accent)
                    .add_modifier(Modifier::BOLD)
                    .bg(palette.surface),
            ),
            PlanItemStatus::Pending => ("◼", palette.muted().bg(palette.surface)),
        };
        // `⎿` anchors the first child; the rest align under the glyph.
        let prefix = if idx == 0 { "  ⎿  " } else { "     " };
        let mut spans = vec![
            Span::styled(prefix, palette.muted().bg(palette.surface)),
            Span::styled(format!("{glyph} "), glyph_style),
        ];
        if let Some(priority) = item.priority.as_ref().filter(|p| !p.trim().is_empty()) {
            spans.push(Span::styled(
                format!("{priority} "),
                palette.muted().bg(palette.surface),
            ));
        }
        let item_style = if item.status == PlanItemStatus::Completed {
            palette.muted().bg(palette.surface)
        } else {
            palette.text().bg(palette.surface)
        };
        spans.push(Span::styled(item.title.clone(), item_style));
        lines.push(Line::from(spans));
    }
    if plan.items.len() > PLAN_PANEL_MAX_ITEMS {
        let more = plan.items.len() - PLAN_PANEL_MAX_ITEMS;
        lines.push(Line::from(Span::styled(
            format!("     … +{more} more"),
            palette.muted().bg(palette.surface),
        )));
    }
    lines
}

/// Status glyph for a sub-agent chip in the agent strip.
pub(crate) fn agent_status_glyph(status: &str) -> &'static str {
    match status.to_ascii_lowercase().as_str() {
        "running" | "spawned" | "in_progress" => "⏵",
        "completed" | "complete" | "done" | "ready" => "✔",
        "failed" | "error" => "✖",
        "cancelled" | "canceled" | "interrupted" => "⊘",
        _ => "•",
    }
}

/// Minimum terminal rows before the selector strip claims its row. Below this a
/// full composer + status + the `Min(1)` tail + the reserved scrollback already
/// fill the screen, so adding the strip would force Ratatui to collapse a fixed
/// row (clipping the composer or status). The Tab switcher still works without
/// the strip — it is a visual aid, not the control surface — so on a tiny
/// terminal we drop it rather than corrupt the layout.
const AGENT_STRIP_MIN_TERMINAL_ROWS: u16 = 12;

/// Maximum sub-agent rows the vertical strip may claim below its title row.
/// Larger rosters stay fully reachable via Tab; the title row carries a `+N`
/// overflow marker and the visible window shifts to keep the selection shown.
const AGENT_STRIP_MAX_AGENT_ROWS: u16 = 4;

/// Sub-agents shown in the under-composer selector strip: the active session's
/// roster minus any that have reached a terminal state. A completed / failed /
/// interrupted sub-agent leaves the strip the instant its terminal
/// `agent/updated` lands — no linger, no waiting for the next Tab-cycle or
/// submit. The ROSTER itself keeps the terminal record (the tick sweep still
/// ages it out for `/ps`), so the peek, the `/ps` dock, and the scrollback card
/// continue to show completed agents; only this live selector drops them.
fn strip_live_agents(app: &AppState) -> Vec<&octos_core::ui_protocol::UiAgentRecord> {
    app.active_session_agents()
        .iter()
        .filter(|agent| !crate::model::agent_status_is_terminal(&agent.status))
        .collect()
}

/// Rows the agent strip occupies under the composer: a title row (with the
/// `main` chip) plus ONE ROW PER SUB-AGENT — vertical so each agent gets a
/// full line of status/task visibility instead of an abbreviated chip. Agent
/// rows are capped by [`AGENT_STRIP_MAX_AGENT_ROWS`] and by what the terminal
/// can spare beyond the minimum layout, so a constrained terminal never
/// oversubscribes the live layout. Both the height reservation
/// (`live_ui_height`) and the render pass call this with the same terminal
/// height, so they always agree.
///
/// Also hidden while the transcript pager is up: the strip switches views via
/// Tab, but Tab is disabled in the pager (it never enters a peek), so the strip
/// is non-interactive there — and the pager's `Min(8)` transcript floor makes
/// its extra rows overcommit sooner than the inline flow's `Min(1)` tail.
fn agent_strip_height(app: &AppState, terminal_height: u16) -> u16 {
    if app.transcript_pager_active
        || terminal_height < AGENT_STRIP_MIN_TERMINAL_ROWS
        || strip_live_agents(app).is_empty()
    {
        0
    } else if app.agent_dock_collapsed {
        // Agent Dock (#323): collapsed mode is a one-line summary pill.
        1
    } else {
        1 + agent_strip_agent_rows(app, terminal_height)
    }
}

/// Sub-agent rows shown below the strip's title row: one line per agent,
/// capped by [`AGENT_STRIP_MAX_AGENT_ROWS`] and by the rows the terminal has
/// to spare beyond [`AGENT_STRIP_MIN_TERMINAL_ROWS`] (at exactly the minimum
/// height the strip degrades to the title row alone — the `+N` marker and Tab
/// keep every agent reachable).
fn agent_strip_agent_rows(app: &AppState, terminal_height: u16) -> u16 {
    let roster = strip_live_agents(app).len().min(u16::MAX as usize) as u16;
    roster
        .min(AGENT_STRIP_MAX_AGENT_ROWS)
        .min(terminal_height.saturating_sub(AGENT_STRIP_MIN_TERMINAL_ROWS))
}

/// Visible window of the agent roster for the vertical strip: the range of
/// indices into `active_session_agents()` to render, plus how many agents are
/// left out. The window starts at the top of the roster and shifts down just
/// enough to keep the selected agent visible.
fn agent_strip_window(app: &AppState, rows: usize) -> (std::ops::Range<usize>, usize) {
    let agents = strip_live_agents(app);
    let len = agents.len();
    if rows == 0 || len == 0 {
        return (0..0, len);
    }
    let rows = rows.min(len);
    let selected = match &app.chat_view {
        crate::model::ChatViewTarget::Agent(id) => agents
            .iter()
            .position(|agent| &agent.agent_id == id)
            .unwrap_or(0),
        _ => 0,
    };
    let start = selected.saturating_sub(rows - 1).min(len - rows);
    (start..start + rows, len - rows)
}

/// One-line task/status detail for an agent row: the last task if the server
/// reported one, else the summary, else the tail of its streamed output —
/// flattened to a single line (the row must never wrap).
fn agent_strip_detail(agent: &octos_core::ui_protocol::UiAgentRecord) -> Option<String> {
    [
        agent.last_task.as_deref(),
        agent.summary.as_deref(),
        agent.output_tail.as_deref(),
    ]
    .into_iter()
    .flatten()
    .flat_map(|text| text.lines())
    .map(str::trim)
    .find(|line| !line.is_empty())
    .map(str::to_owned)
}

/// `(total, running, unread)` roster counts for the Agent Dock pill and the
/// `/agents` menu subtitle. `running` = every non-terminal status (spawned/
/// pending included — they occupy a concurrency slot either way).
pub(crate) fn agent_dock_counts(app: &AppState) -> (usize, usize, usize) {
    let agents = app.active_session_agents();
    let running = agents
        .iter()
        .filter(|agent| !crate::model::agent_status_is_terminal(&agent.status))
        .count();
    let unseen = app.active_session_unseen_agents().len();
    (agents.len(), running, unseen)
}

/// Spawn depth of `agent` within the visible roster, by walking
/// `parent_agent_id` links. Bounded so a malformed cycle can't loop; agents
/// whose parent is not in the roster (or absent) render at depth 0.
fn agent_depth(agents: &[octos_core::ui_protocol::UiAgentRecord], agent_id: &str) -> usize {
    let mut depth = 0;
    let mut current = agent_id;
    while depth < 4 {
        let Some(parent) = agents
            .iter()
            .find(|a| a.agent_id == current)
            .and_then(|a| a.parent_agent_id.as_deref())
        else {
            break;
        };
        if parent == current || !agents.iter().any(|a| a.agent_id == parent) {
            break;
        }
        depth += 1;
        current = parent;
    }
    depth
}

/// Compact `41s` / `2m14s` / `1h02m` duration label for an agent row.
fn format_short_duration(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Elapsed label for an agent row: run duration so far for a live agent
/// (local wall clock vs the server's `created_at_ms` — minor skew is
/// acceptable for a glanceable label, floored at 0), and the final
/// `updated - created` span (same clock on both ends) once terminal.
fn agent_elapsed_label(agent: &octos_core::ui_protocol::UiAgentRecord) -> Option<String> {
    if agent.created_at_ms <= 0 {
        return None;
    }
    let end_ms = if crate::model::agent_status_is_terminal(&agent.status) {
        agent.updated_at_ms
    } else {
        chrono::Utc::now().timestamp_millis()
    };
    (end_ms > agent.created_at_ms).then(|| format_short_duration(end_ms - agent.created_at_ms))
}

/// The collapsed Agent Dock pill (#323): one glanceable line —
/// `🐙 3 agents · 2 running · 1● unread — Alt+D` — in place of the per-agent
/// rows. The unread segment only appears when something finished unseen.
fn agent_dock_pill_line(app: &AppState, palette: Palette) -> Line<'static> {
    let (total, running, unseen) = agent_dock_counts(app);
    let mut spans = vec![Span::styled(
        t!(
            "app.hint.agent_dock_pill",
            count = total.to_string(),
            running = running.to_string()
        )
        .into_owned(),
        palette.text().bg(palette.surface),
    )];
    if unseen > 0 {
        spans.push(Span::styled(
            t!(
                "app.hint.agent_dock_pill_unread",
                count = unseen.to_string()
            )
            .into_owned(),
            Style::default()
                .fg(palette.highlight)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", t!("app.hint.agent_dock_toggle_hint")),
        palette.muted().bg(palette.surface),
    ));
    Line::from(spans)
}

/// Logical lines for the vertical agent strip. Row 0 is the title row: strip
/// title + the `main` chip + a muted `+N` marker when the roster overflows the
/// visible window. Each following row is one sub-agent — glyph, name, raw
/// status, and a muted task/output detail — with the selected target
/// highlighted. Split from rendering so the layout logic is unit-testable
/// without a frame; `agent_rows` must be the value the height reservation was
/// computed with (`agent_strip_height` - 1).
fn agent_strip_lines(app: &AppState, palette: Palette, agent_rows: u16) -> Vec<Line<'static>> {
    if app.agent_dock_collapsed {
        return vec![agent_dock_pill_line(app, palette)];
    }
    // Full roster for tree-depth (a child's parent may itself be terminal and
    // hidden from the rows) — but only LIVE agents become rows.
    let roster = app.active_session_agents();
    let agents = strip_live_agents(app);
    let (window, hidden) = agent_strip_window(app, agent_rows as usize);
    let selected_style = Style::default()
        .fg(palette.surface)
        .bg(palette.accent)
        .add_modifier(Modifier::BOLD);

    let mut title_spans: Vec<Span<'static>> = vec![Span::styled(
        t!("app.hint.agent_strip_title").into_owned(),
        palette.muted().bg(palette.surface),
    )];
    let main_selected = matches!(app.chat_view, crate::model::ChatViewTarget::Main);
    title_spans.push(Span::styled(
        format!(" ⌂ {} ", t!("app.hint.agent_strip_main")),
        if main_selected {
            selected_style
        } else {
            palette.text().bg(palette.surface)
        },
    ));
    if hidden > 0 {
        title_spans.push(Span::styled(
            format!(
                "  {}",
                t!("app.hint.agent_strip_more", count = hidden.to_string())
            ),
            palette.muted().bg(palette.surface),
        ));
    }
    // Unread summary on the title row so overflow-hidden completions still
    // register at a glance (#323).
    let unseen_total = app.active_session_unseen_agents().len();
    if unseen_total > 0 {
        title_spans.push(Span::styled(
            t!(
                "app.hint.agent_dock_pill_unread",
                count = unseen_total.to_string()
            )
            .into_owned(),
            Style::default()
                .fg(palette.highlight)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let mut lines = vec![Line::from(title_spans)];

    for &agent in &agents[window] {
        let selected = matches!(
            &app.chat_view,
            crate::model::ChatViewTarget::Agent(id) if id == &agent.agent_id
        );
        let label = if agent.nickname.trim().is_empty() {
            agent.role.clone()
        } else {
            agent.nickname.clone()
        };
        // Depth-indent children under their parent (#323) — nested spawns
        // read as a tree instead of a flat list.
        let indent = "  ".repeat(agent_depth(roster, &agent.agent_id));
        // Only LIVE agents are rows now (a terminal agent leaves the strip the
        // instant it finishes), and the unread badge only ever marks terminal
        // agents — so a per-row unread dot can never fire here. The unread
        // outcome still surfaces on the title-row summary and the collapsed
        // pill, and the full result stays in `/ps`, the peek, and scrollback.
        let mut spans = Vec::new();
        let elapsed = agent_elapsed_label(agent)
            .map(|label| format!(" · {label}"))
            .unwrap_or_default();
        spans.push(Span::styled(
            format!(
                " {indent}{} {label} · {}{elapsed} ",
                agent_status_glyph(&agent.status),
                agent.status
            ),
            if selected {
                selected_style
            } else {
                palette.text().bg(palette.surface)
            },
        ));
        if let Some(detail) = agent_strip_detail(agent) {
            spans.push(Span::styled(
                format!(" — {detail}"),
                palette.muted().bg(palette.surface),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Fallback context-window denominator for `ctx N%`, used only until a cost
/// update carries the real per-model window (`token_cost.context_window`, stored
/// in `AppState::session_context_window`). Surfaces the inspector-only
/// `token_estimate` as a glanceable budget bar in the harness status row.
const DEFAULT_CONTEXT_WINDOW_TOKENS: usize = 128_000;

/// Compact token count for the harness row: `34211` -> `34.2k`.
fn humanize_token_count(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}

/// True when the harness has live state worth surfacing in the dedicated
/// status row: the active session is orchestrating (server `active`) OR a turn
/// is in progress locally. Idle → the row collapses to height 0 so it can
/// never collide with the composer's top-border chrome (the prior revert,
/// 249fe652, drew the indicator ON the composer border).
fn harness_status_active(app: &AppState) -> bool {
    let orchestrating = app
        .active_session()
        .and_then(|session| app.orchestration.get(&session.id))
        .is_some_and(|status| status.active);
    orchestrating || matches!(app.run_state, SessionRunState::InProgress)
}

/// Rows the harness status indicator needs: 1 when active, 0 when idle.
fn harness_status_height(app: &AppState) -> u16 {
    if harness_status_active(app) { 1 } else { 0 }
}

/// `(used_tokens, window_tokens)` for `session_id`, for the `/context` menu's
/// live usage line. `None` until a token estimate is known for the session.
/// Window resolution mirrors [`harness_context_ratio`]: the real per-model
/// window (`session_context_window`, from `metadata.token_cost.context_window`)
/// when known, else the fixed default until the first cost update arrives.
pub(crate) fn context_window_usage(app: &AppState, session_id: &SessionKey) -> Option<(u64, u64)> {
    let used = app
        .context_lifecycle_for(session_id)?
        .state
        .as_ref()?
        .token_estimate as u64;
    let window = app
        .session_context_window
        .get(session_id)
        .copied()
        .filter(|w| *w > 0)
        .unwrap_or_else(|| model_context_window_hint(app, session_id));
    Some((used, window))
}

/// Context-window fill ratio (0.0..=1.0) for the harness row `LineGauge`, or
/// `None` when no `token_estimate` is known for the active session yet.
fn harness_context_ratio(app: &AppState) -> Option<f64> {
    let session = app.active_session()?;
    let token_estimate = app
        .context_lifecycle_for(&session.id)?
        .state
        .as_ref()?
        .token_estimate;
    // Prefer the real per-model context window carried on the wire
    // (`metadata.token_cost.context_window`); fall back to the fixed default
    // only until the first cost update arrives for this session.
    let window = app
        .session_context_window
        .get(&session.id)
        .copied()
        .filter(|w| *w > 0)
        .unwrap_or_else(|| model_context_window_hint(app, &session.id)) as usize;
    if window == 0 {
        return None;
    }
    Some((token_estimate as f64 / window as f64).clamp(0.0, 1.0))
}

/// Integer context-window percent (0..=100) for the `ctx N%` label.
fn harness_context_percent(app: &AppState) -> Option<u16> {
    harness_context_ratio(app).map(|ratio| (ratio * 100.0).round() as u16)
}

/// Full context-window label for the harness gauge/row: `ctx 128K/1M ~13%`.
/// Pairs the used/max token counts (see [`context_window_usage`]) with the
/// estimate percent so the always-on row shows the raw numbers, not just a
/// bare percentage. The `~` marks it an estimate: the numerator is the harness
/// `token_estimate` and the denominator falls back to the fixed default until a
/// real per-model window arrives. `None` until an estimate is known.
fn harness_context_label(app: &AppState) -> Option<String> {
    let session = app.active_session()?;
    let (used, window) = context_window_usage(app, &session.id)?;
    let percent = harness_context_percent(app)?;
    Some(format!(
        "ctx {}/{} ~{percent}%",
        format_tokens_human(used),
        format_tokens_human(window),
    ))
}

/// Build the harness status line(s): spinner + phase + agent count +
/// re-entering + token in/out + cost + retry + ctx %. Empty when idle.
fn harness_status_lines(
    app: &AppState,
    palette: Palette,
    include_ctx_text: bool,
) -> Vec<Line<'static>> {
    if !harness_status_active(app) {
        return Vec::new();
    }
    let Some(session) = app.active_session() else {
        return Vec::new();
    };
    let session_id = session.id.clone();
    let status = app.orchestration.get(&session_id);

    // The whimsical persona status word (server `progress/updated{kind:
    // "status_word"}`, rotated ~every 8s — e.g. "Conjuring", "正在炼丹") wins
    // over the flat "Working" phase so the gradient line reads `◠ Conjuring…`
    // like the web ThinkingIndicator. It replaces ONLY the generic working
    // phase; a real "orchestrating" / "re-entering" phase (sub-agents running,
    // master re-entry) still shows, since that is information the operator
    // should see rather than a decorative word. The `…` reads as an ongoing
    // action.
    // Only the ACTIVE turn's word shows — a word keyed to a settled/prior turn
    // (or a server-started continuation before its own first rotation) is
    // ignored, so a stale word never lingers (codex P2 on #294).
    let active_turn_id = app.active_turn().map(|(_, turn_id)| turn_id);
    let persona_word = app
        .session_status_word
        .get(&session_id)
        .filter(|(word_turn, _)| active_turn_id == Some(word_turn))
        .map(|(_, word)| word.trim())
        .filter(|word| !word.is_empty())
        .map(|word| format!("{word}…"));
    let phase = match status.and_then(|s| s.phase.as_deref()) {
        Some("orchestrating") => t!("app.harness.orchestrating").to_string(),
        Some("re-entering") => t!("app.harness.re_entering").to_string(),
        Some("working") => persona_word
            .clone()
            .unwrap_or_else(|| t!("app.harness.working").to_string()),
        Some(other) if !other.is_empty() => other.to_string(),
        _ => persona_word
            .clone()
            .unwrap_or_else(|| t!("app.harness.working").to_string()),
    };

    let mut spans: Vec<Span<'static>> = Vec::new();
    // Water-wave gradient on "spinner + phase" (e.g. "◠ Working"): a bright crest
    // ripples across the label, advanced by the ~25ms animation redraw via the
    // shared process clock. Uses Color::Rgb like the rest of octos-tui's themes
    // (truecolor-assuming, so it works over SSH where COLORTERM isn't forwarded);
    // the non-RGB Terminal theme degrades to a neutral-grey ripple via rgb_of.
    let label = format!("{} {}", spinner_frame(), phase);
    let stops = [
        rgb_of(palette.muted),
        rgb_of(palette.accent),
        rgb_of(palette.highlight),
    ];
    spans.extend(wave_gradient_spans(
        &label,
        anim_time_secs() * 3.0,
        &stops,
        palette.surface,
    ));

    if let Some(status) = status {
        if status.running_agents > 0 {
            spans.push(Span::styled(
                format!(
                    " · {}",
                    t!("app.statusbar.agents", count = status.running_agents)
                ),
                palette.text().bg(palette.surface),
            ));
        }
        // The re-entry gap (sub-agents settled, a continuation queued) is the
        // whole reason for this row: it must NOT read as done.
        if status.pending_continuations > 0 {
            spans.push(Span::styled(
                format!(" · {}", t!("app.statusbar.re_entering")),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    // Token in/out + cumulative session cost (from token_cost progress).
    if let Some((input, output, cost)) = app.session_usage.get(&session_id) {
        if input.is_some() || output.is_some() {
            spans.push(Span::styled(
                format!(
                    " · ↑{} ↓{}",
                    humanize_token_count(input.unwrap_or(0)),
                    humanize_token_count(output.unwrap_or(0)),
                ),
                palette.text().bg(palette.surface),
            ));
        }
        if let Some(cost) = cost.filter(|c| *c > 0.0) {
            spans.push(Span::styled(
                format!(" · ${cost:.4}"),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    // Retry/backoff (metadata.retry — previously ignored on the wire).
    if let Some(retry) = app.session_retry.get(&session_id) {
        let attempt = match (retry.attempt, retry.max_attempts) {
            (Some(a), Some(max)) => format!(
                " · {}",
                t!("app.statusbar.retrying_attempt_max", attempt = a, max = max)
            ),
            (Some(a), None) => format!(" · {}", t!("app.statusbar.retrying_attempt", attempt = a)),
            _ => format!(" · {}", t!("app.statusbar.retrying")),
        };
        spans.push(Span::styled(
            attempt,
            palette
                .muted()
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // Context window %. This textual label is the NARROW-terminal fallback:
    // when `render_harness_status_row` draws the LineGauge (wide terminal) it
    // passes `include_ctx_text = false` so the percent does not render twice —
    // once as this text on the left and once as the gauge's own label on the
    // right (the duplicate-`ctx ~N%` bug). Kept (and unit-tested) for narrow
    // terminals where the gauge column is dropped.
    if include_ctx_text {
        // `ctx {used}/{max} ~{pct}%` — the raw token counts plus the estimate
        // percent. `~` marks it an estimate: the numerator is the harness
        // `token_estimate`. The denominator is the real per-model context
        // window once a cost update carries it (`token_cost.context_window`),
        // falling back to `DEFAULT_CONTEXT_WINDOW_TOKENS` until then.
        if let Some(label) = harness_context_label(app) {
            spans.push(Span::styled(
                format!(" · {label}"),
                palette.muted().bg(palette.surface),
            ));
        }
    }

    vec![Line::from(spans)]
}

/// The current model id for the active session, drawn on the composer's bottom
/// border. Prefers the runtime status's reported model, then its runtime policy
/// stamp, then the model catalog's selected entry — so the footer reflects the
/// current model whether it arrived via `session/status/read`, a model
/// selection, or just the `/model` catalog. `None` until any of those is known
/// (the footer then shows only the cwd).
fn composer_footer_model(app: &AppState) -> Option<String> {
    let session_id = &app.active_session()?.id;
    session_model_id(app, session_id)
}

/// The active model id for a session — from the runtime status, else the
/// selected model in the catalog. Shared by the footer and the model-aware
/// context-window fallback ([`model_context_window_hint`]).
fn session_model_id(app: &AppState, session_id: &SessionKey) -> Option<String> {
    let from_status = app.runtime_status_for(session_id).and_then(|status| {
        status
            .model
            .as_ref()
            .map(|model| model.model.clone())
            .or_else(|| {
                status
                    .runtime_policy_stamp
                    .as_ref()
                    .and_then(|stamp| stamp.model.clone())
            })
    });
    from_status
        .or_else(|| {
            app.model_catalog_for(session_id).and_then(|catalog| {
                catalog
                    .models
                    .iter()
                    .find(|model| model.selected)
                    .map(|model| model.model.clone())
            })
        })
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
}

/// A model-aware context-window fallback denominator for the `ctx N%` gauge,
/// used ONLY until the first `token_cost` update carries the real per-model
/// window (`session_context_window`). Mirrors the octos server's
/// `context::context_window_tokens` heuristic for the well-known long-context
/// models, so a fresh MiniMax-M3 / DeepSeek-V4 / Kimi-K3 / GLM session shows its
/// real ~1M window instead of the generic 128K placeholder. The authoritative
/// server value still takes over on the first turn; this only fixes the
/// pre-first-turn display. Unknown models keep the conservative 128K default.
fn model_context_window_hint(app: &AppState, session_id: &SessionKey) -> u64 {
    let Some(model) = session_model_id(app, session_id) else {
        return DEFAULT_CONTEXT_WINDOW_TOKENS as u64;
    };
    let m = model.to_ascii_lowercase();
    // Bare `k3` / `kimi-for-coding*` are the Kimi coding plan's K3 ids — 1M, like
    // `kimi-k3` (which they don't contain). Mirrors the server heuristic.
    if m.contains("deepseek-v4")
        || m.contains("minimax-m3")
        || m.contains("kimi-k3")
        || m == "k3"
        || m.starts_with("kimi-for-coding")
    {
        1_048_576
    } else if m.contains("glm") || m.contains("minimax") {
        1_000_000
    } else {
        DEFAULT_CONTEXT_WINDOW_TOKENS as u64
    }
}

fn set_composer_cursor(frame: &mut impl FrameLike, app: &AppState, area: Rect) {
    if app.focus != FocusPane::Composer {
        return;
    }
    if let Some(position) = composer_cursor_position(app, area) {
        frame.set_cursor_position(position);
    }
}

fn composer_cursor_position(app: &AppState, area: Rect) -> Option<Position> {
    if area.width <= 2 || area.height <= 2 {
        return None;
    }

    let (row_offset, text_width) = composer_cursor_row_and_width(
        &app.composer_presentation(),
        app.composer_cursor_index(),
        area,
    );
    let input_y = area.y + 2 + row_offset;
    if input_y >= area.y + area.height.saturating_sub(1) {
        return None;
    }

    let text_width = text_width as u16;
    let inner_right = area.x + area.width.saturating_sub(2);
    let input_x = area.x + 4 + text_width;
    Some(Position::new(input_x.min(inner_right), input_y))
}

fn composer_cursor_row_and_width(
    composer: &ComposerPresentation,
    cursor: usize,
    area: Rect,
) -> (u16, usize) {
    match composer {
        ComposerPresentation::Empty => (0, 0),
        ComposerPresentation::Inline(text) => {
            let view = composer_input_view(
                text,
                cursor,
                area.width,
                area.height.saturating_sub(COMPOSER_CHROME_ROWS),
            );
            (view.cursor_row, view.cursor_width)
        }
        ComposerPresentation::Collapsed(collapse) => {
            (0, "[paste] ".width() + collapse.summary.width())
        }
    }
}

fn composer_input_view(
    text: &str,
    cursor: usize,
    terminal_width: u16,
    max_rows: u16,
) -> ComposerInputView {
    let width = composer_text_width(terminal_width);
    let max_rows = usize::from(max_rows.max(1));
    let logical_lines = composer_logical_lines(text);
    let cursor = cursor.min(text.len());
    let cursor_line_index = logical_lines
        .iter()
        .position(|line| cursor <= line.end)
        .unwrap_or_else(|| logical_lines.len().saturating_sub(1));
    let line_window_end = if cursor == text.len() {
        logical_lines.len().saturating_sub(1)
    } else {
        cursor_line_index
    };
    let mut selected = Vec::new();
    let mut used_rows = 0usize;
    let mut hidden_prefix = false;
    let mut selected_cursor_line = 0usize;
    let mut cursor_width = 0usize;
    let mut cursor_row = 0usize;

    for index in (0..=line_window_end).rev() {
        let line = &logical_lines[index];
        let rows = visual_rows_for_text(line.text, width);
        if used_rows == 0 && rows > max_rows {
            let line_cursor = cursor.saturating_sub(line.start).min(line.text.len());
            let visible = tail_around_cursor(line.text, line_cursor, width, max_rows);
            cursor_row = cursor_row_for_text(&visible.before_cursor, width);
            cursor_width = cursor_width_for_text(&visible.before_cursor, width);
            selected_cursor_line = 0;
            selected.push(visible.text);
            hidden_prefix = true;
            break;
        }
        if used_rows + rows > max_rows {
            break;
        }
        if index == cursor_line_index {
            let before_cursor =
                &line.text[..cursor.saturating_sub(line.start).min(line.text.len())];
            cursor_row = cursor_row_for_text(before_cursor, width);
            cursor_width = cursor_width_for_text(before_cursor, width);
            selected_cursor_line = selected.len();
        }
        selected.push(line.text.to_string());
        used_rows += rows;
    }

    selected.reverse();
    selected_cursor_line = selected
        .len()
        .saturating_sub(1)
        .saturating_sub(selected_cursor_line);
    if selected.is_empty() {
        selected.push(String::new());
    }

    let hidden_lines = logical_lines.len().saturating_sub(selected.len());
    let rows_before_cursor = selected
        .iter()
        .take(selected_cursor_line)
        .map(|line| visual_rows_for_text(line, width))
        .sum::<usize>();

    ComposerInputView {
        lines: selected,
        hidden_lines,
        hidden_prefix,
        cursor_row: rows_before_cursor.saturating_add(cursor_row) as u16,
        cursor_width,
    }
}

struct ComposerLogicalLine<'a> {
    text: &'a str,
    start: usize,
    end: usize,
}

fn composer_logical_lines(text: &str) -> Vec<ComposerLogicalLine<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for line in text.split('\n') {
        let end = start + line.len();
        lines.push(ComposerLogicalLine {
            text: line,
            start,
            end,
        });
        start = end.saturating_add(1);
    }
    if lines.is_empty() {
        lines.push(ComposerLogicalLine {
            text: "",
            start: 0,
            end: 0,
        });
    }
    lines
}

struct VisibleCursorLine {
    text: String,
    before_cursor: String,
}

fn tail_around_cursor(
    text: &str,
    cursor: usize,
    width: usize,
    max_rows: usize,
) -> VisibleCursorLine {
    let prefix = &text[..cursor.min(text.len())];
    // Whole line fits the budget: show it unchanged. Measured via the same
    // grapheme wrapping render uses, so this can't disagree with what is drawn.
    if visual_rows_for_text(text, width) <= max_rows {
        return VisibleCursorLine {
            text: text.to_string(),
            before_cursor: prefix.to_string(),
        };
    }
    // Line is taller than the budget. If the cursor is still within the first
    // `max_rows` rows, show the HEAD window (the first `max_rows` wrapped rows)
    // — the cursor is already inside it — so render never emits more rows than
    // the composer reserved (which would clip the footer).
    let cursor_chunks = wrap_composer_line(prefix, width);
    if cursor_chunks.len() <= max_rows {
        let chunks = wrap_composer_line(text, width);
        let head: String = chunks[..max_rows.min(chunks.len())].concat();
        return VisibleCursorLine {
            text: head,
            before_cursor: prefix.to_string(),
        };
    }
    // Cursor is past the budget: show the tail of `prefix` ending at the cursor.
    // Keep the last `max_rows - 1` wrapped rows and reserve the first row for the
    // "..." marker, so the window never exceeds `max_rows` rows even when
    // double-width graphemes leave spare columns at a row boundary.
    let keep = max_rows.saturating_sub(1).max(1);
    let start = cursor_chunks.len().saturating_sub(keep);
    let tail: String = cursor_chunks[start..].concat();
    let text = format!("...{tail}");
    VisibleCursorLine {
        text: text.clone(),
        before_cursor: text,
    }
}

fn cursor_row_for_text(text: &str, width: usize) -> usize {
    // Row index of the cursor within its logical line, derived from the same
    // grapheme wrapping render uses (wrap_composer_line) so the cursor sits on
    // the row the text is actually drawn on.
    wrap_composer_line(text, width).len().saturating_sub(1)
}

fn cursor_width_for_text(text: &str, width: usize) -> usize {
    // Display column of the cursor within its row: the width of the last wrapped
    // chunk (0 for empty input).
    wrap_composer_line(text, width)
        .last()
        .map(|chunk| chunk.width())
        .unwrap_or(0)
}

fn hint_bar_text(model: HintBarModel) -> String {
    match model.mode {
        HintBarMode::StatusbarKeys => t!("app.hint.statusbar_keys").into_owned(),
        HintBarMode::Menu => t!("app.hint.menu").into_owned(),
        HintBarMode::Onboarding => t!("app.hint.onboarding").into_owned(),
        HintBarMode::Approval => t!("app.hint.approval").into_owned(),
        HintBarMode::UserQuestion => t!("app.hint.user_question").into_owned(),
        HintBarMode::PagerKeys => t!("app.hint.pager_keys").into_owned(),
        HintBarMode::PagerReviewing => t!("app.hint.pager_reviewing").into_owned(),
        HintBarMode::ActivityNavigator => t!("app.hint.activity_navigator").into_owned(),
    }
}

/// The `(session, turn)` of the operator decision the active session's turn is
/// parked on — a pending tool approval or an `AskUserQuestion` picker — if any.
/// This is authoritative for interrupting a parked turn: the decision carries its
/// own `turn_id`, so it works even when `active_turn()` is `None` (a decision can
/// park a turn before any reply streams, so there is no `live_reply` for
/// `active_turn` to key off).
pub(crate) fn active_session_pending_decision_turn(app: &AppState) -> Option<(SessionKey, TurnId)> {
    let session_id = app.active_session().map(|session| session.id.clone())?;
    if let Some(approval) = app
        .approval
        .as_ref()
        .filter(|approval| approval.session_id == session_id)
    {
        return Some((approval.session_id.clone(), approval.turn_id.clone()));
    }
    app.user_question
        .as_ref()
        .filter(|question| question.session_id == session_id)
        .map(|question| (question.session_id.clone(), question.turn_id.clone()))
}

/// True when the active session's turn is parked on an operator decision — a
/// pending tool approval or an `AskUserQuestion` picker. While this holds the
/// decision modal owns the keyboard (y/s/n) so the composer is locked; the modal
/// can also scroll out of the height-clipped live tail, leaving the user with a
/// bare "Waiting" and no visible prompt — so the status bar must advertise the
/// recovery keys (Ctrl+R/Alt+A to bring the prompt back, Ctrl+C to interrupt).
pub(crate) fn active_session_has_pending_decision(app: &AppState) -> bool {
    active_session_pending_decision_turn(app).is_some()
}

/// Seconds a turn may sit parked on an operator decision before the watchdog
/// escalates. The escalation re-shows a hidden modal and paints a prominent
/// banner above the composer; it NEVER auto-answers or auto-interrupts — a
/// human-approval gate must wait for the human.
pub(crate) const PARKED_DECISION_ESCALATE_SECS: u64 = 60;

/// `Some(elapsed_secs)` once the active session has been parked on a decision for
/// at least [`PARKED_DECISION_ESCALATE_SECS`]. Elapsed is derived from the SAME
/// source as the status bar's "11m 12s" (`run_state_elapsed_secs`, a monotonic
/// `Instant`), so the banner and the status agree and the threshold check stays
/// deterministic in tests.
pub(crate) fn parked_decision_escalation_secs(app: &AppState) -> Option<u64> {
    if !active_session_has_pending_decision(app) {
        return None;
    }
    app.run_state_elapsed_secs()
        .filter(|elapsed| *elapsed >= PARKED_DECISION_ESCALATE_SECS)
}

/// Rows reserved for the parked-decision escalation banner (one line, styled as a
/// solid attention band above the composer). Zero until the escalation fires.
/// Reserved height equals the rendered rows — one — so the layout reservation and
/// [`render_decision_banner`] agree (same discipline as the autonomy indicator).
fn decision_banner_height(app: &AppState) -> u16 {
    u16::from(
        parked_decision_escalation_secs(app).is_some()
            || pending_question_for_banner(app).is_some(),
    )
}

/// A pending, keyboard-owning question renders its submit/toggle affordance in
/// the reserved decision-banner chrome, so the SUBMIT control can never scroll
/// off the height-capped live tail. The options list can (and does) scroll; the
/// submit hint must not — before this, the only submit affordance lived at the
/// bottom of the scrollable picker card (clips vertically) and in the unwrapped
/// status line (clips horizontally), so a taller-than-half-screen question left
/// the user staring at options with no visible way to submit.
fn pending_question_for_banner(app: &AppState) -> Option<&UserQuestionPickerState> {
    app.user_question
        .as_ref()
        .filter(|picker| picker.visible && !picker.questions.is_empty())
}

fn status_bar_work_text(app: &AppState) -> String {
    let mut parts = Vec::new();
    match &app.run_state {
        SessionRunState::Blocked { message } | SessionRunState::Error { message }
            if !message.trim().is_empty() =>
        {
            parts.push(truncate_terminal_line(message, 80));
        }
        _ => {}
    }
    if let Some(seconds) = app.run_state_elapsed_secs() {
        parts.push(format_elapsed_secs(seconds));
    }
    let background_tasks = active_background_tasks(app);
    if background_tasks > 0 {
        parts.push(t!("app.statusbar.background_tasks", count = background_tasks).into_owned());
        parts.push(t!("app.statusbar.ps_to_view").into_owned());
    }
    if active_session_has_pending_decision(app) {
        // Turn parked on YOUR decision; the approval/question card may have
        // scrolled out of the clipped live tail, so a bare "Esc interrupt" (a
        // two-step while a modal is up) is a dead end. Advertise the real
        // recovery keys instead — shown whenever a decision is pending, not just
        // when an active turn is reported.
        parts.push(t!("app.statusbar.pending_decision_help").into_owned());
    } else if app.active_turn().is_some() {
        parts.push(t!("app.statusbar.esc_interrupt").into_owned());
        parts.push(t!("app.statusbar.stop_to_close").into_owned());
    }
    if app.expanded_tool_outputs {
        parts.push(t!("app.statusbar.tool_output_expanded").into_owned());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("({})", parts.join(" | "))
    }
}

fn run_state_status_label(state: &SessionRunState) -> String {
    match state {
        SessionRunState::Idle => t!("app.status.idle").to_string(),
        SessionRunState::InProgress => t!("app.status.working").to_string(),
        SessionRunState::Blocked { .. } => t!("app.status.blocked").to_string(),
        SessionRunState::Success => t!("app.status.done").to_string(),
        SessionRunState::Error { .. } => t!("app.status.error").to_string(),
    }
}

fn run_state_style(state: &SessionRunState, palette: Palette) -> Style {
    match state {
        SessionRunState::Idle => palette.muted(),
        SessionRunState::InProgress => palette.selected().add_modifier(Modifier::BOLD),
        SessionRunState::Blocked { .. } => Style::default()
            .fg(palette.highlight)
            .add_modifier(Modifier::BOLD),
        SessionRunState::Success => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD),
        SessionRunState::Error { .. } => Style::default()
            .fg(palette.danger)
            .add_modifier(Modifier::BOLD),
    }
}

fn run_state_marker(state: &SessionRunState) -> &'static str {
    match state {
        // Pin the swirling galaxy to the always-visible status bar: on a big
        // turn the transcript's "Orchestrating" chip scrolls above the fold, so
        // this is the reliable "still working" signal that never scrolls away.
        // Time-based like the transcript spinner; the status bar redraws every
        // frame so it animates smoothly.
        SessionRunState::InProgress => spinner_frame(),
        SessionRunState::Blocked { .. } => "!",
        SessionRunState::Success => "✓",
        SessionRunState::Error { .. } => "x",
        SessionRunState::Idle => "·",
    }
}

fn short_id(id: &str) -> String {
    const MAX_ID_LEN: usize = 8;
    if id.len() <= MAX_ID_LEN {
        id.to_string()
    } else {
        id[..MAX_ID_LEN].to_string()
    }
}

/// Resolve the current user's home directory from `HOME`, falling back to
/// `USERPROFILE` (Windows normally sets only the latter), if set and non-empty.
fn home_dir_str() -> Option<String> {
    ["HOME", "USERPROFILE"].into_iter().find_map(|var| {
        std::env::var_os(var)
            .filter(|home| !home.is_empty())
            .and_then(|home| home.into_string().ok())
    })
}

/// Collapse a leading home-directory prefix to `~` the way a shell does
/// (`/Users/me/proj` → `~/proj`, `/Users/me` → `~`). A no-op when `home` is
/// absent/empty or is not a path-boundary prefix of `path` (so `/Users/mentor`
/// is never mangled by a `/Users/me` home). Both `/` and `\` count as the
/// boundary so native Windows paths collapse too. Pure over `home` so it is
/// testable without touching the process environment.
fn collapse_home_prefix(path: &str, home: Option<&str>) -> String {
    let Some(home) = home
        .map(|home| home.trim_end_matches(['/', '\\']))
        .filter(|home| !home.is_empty())
    else {
        return path.to_string();
    };
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

fn short_path(path: &str) -> String {
    const MAX_PATH_LEN: usize = 28;
    let path = collapse_home_prefix(path, home_dir_str().as_deref());
    if path.chars().count() <= MAX_PATH_LEN {
        return path;
    }
    let suffix = path
        .chars()
        .rev()
        .take(MAX_PATH_LEN.saturating_sub(3))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("...{suffix}")
}

fn approval_modal_lines(approval: &ApprovalModalState, palette: Palette) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(approval.title.clone(), palette.title())),
        Line::from(vec![
            Span::styled(format!("{} ", t!("app.field.tool")), palette.muted()),
            Span::styled(approval.tool_name.clone(), palette.text()),
        ]),
    ];

    if let Some(kind) = approval.approval_kind.as_ref() {
        let risk = approval
            .risk
            .as_ref()
            .map(|risk| format!("  {} {risk}", t!("app.field.risk")))
            .unwrap_or_default();
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", t!("app.field.kind")), palette.muted()),
            Span::styled(kind.clone(), palette.text()),
            Span::styled(risk, palette.muted()),
        ]));
    }

    lines.push(Line::from(""));

    if let Some(details) = approval.typed_details.as_ref() {
        match details.kind.as_str() {
            approval_kinds::COMMAND => {
                if let Some(command) = details.command.as_ref() {
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.command").into_owned(),
                        command.command_line.as_deref(),
                    );
                    push_optional_field(&mut lines, palette, "cwd", command.cwd.as_deref());
                    if !command.argv.is_empty() {
                        push_field(&mut lines, palette, "argv", command.argv.join(" "));
                    }
                    if !command.env_keys.is_empty() {
                        push_field(&mut lines, palette, "env", command.env_keys.join(", "));
                    }
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.tool_call").into_owned(),
                        command.tool_call_id.as_deref(),
                    );
                }
                if let Some(sandbox) = details.sandbox.as_ref() {
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.sandbox").into_owned(),
                        sandbox.mode.as_deref(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.filesystem").into_owned(),
                        sandbox.filesystem_access.as_deref(),
                    );
                    if let Some(network_access) = sandbox.network_access {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.network").into_owned(),
                            network_access.to_string(),
                        );
                    }
                    if !sandbox.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.writable").into_owned(),
                            sandbox.writable_roots.join(", "),
                        );
                    }
                }
            }
            approval_kinds::DIFF => {
                if let Some(diff) = details.diff.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.preview").into_owned(),
                        diff.preview_id.0.to_string(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        diff.operation.as_deref(),
                    );
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.summary").into_owned(),
                        diff.summary.as_deref(),
                    );
                    let stats = [
                        diff.file_count
                            .map(|value| t!("app.field.files_count", count = value).into_owned()),
                        diff.additions.map(|value| format!("+{value}")),
                        diff.deletions.map(|value| format!("-{value}")),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    if !stats.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.stats").into_owned(),
                            stats,
                        );
                    }
                }
            }
            approval_kinds::FILESYSTEM => {
                if let Some(filesystem) = details.filesystem.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        filesystem.operation.clone(),
                    );
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.outside_workspace").into_owned(),
                        filesystem.outside_workspace.to_string(),
                    );
                    for path in &filesystem.paths {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.path").into_owned(),
                            path.clone(),
                        );
                    }
                    if !filesystem.writable_roots.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.writable").into_owned(),
                            filesystem.writable_roots.join(", "),
                        );
                    }
                }
            }
            approval_kinds::NETWORK => {
                if let Some(network) = details.network.as_ref() {
                    push_field(
                        &mut lines,
                        palette,
                        t!("app.field.operation").into_owned(),
                        network.operation.clone(),
                    );
                    if !network.hosts.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.hosts").into_owned(),
                            network.hosts.join(", "),
                        );
                    }
                    if !network.ports.is_empty() {
                        let ports = network
                            .ports
                            .iter()
                            .map(|port| port.to_string())
                            .collect::<Vec<_>>()
                            .join(", ");
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.ports").into_owned(),
                            ports,
                        );
                    }
                    for url in &network.urls {
                        push_field(&mut lines, palette, "url", url.clone());
                    }
                }
            }
            approval_kinds::SANDBOX_ESCALATION => {
                if let Some(escalation) = details.sandbox_escalation.as_ref() {
                    if let Some(from) = escalation.from.as_ref() {
                        push_optional_field(
                            &mut lines,
                            palette,
                            t!("app.field.from").into_owned(),
                            from.mode.as_deref(),
                        );
                    }
                    if let Some(to) = escalation.to.as_ref() {
                        push_optional_field(
                            &mut lines,
                            palette,
                            t!("app.field.to").into_owned(),
                            to.mode.as_deref(),
                        );
                    }
                    if !escalation.requested_permissions.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            t!("app.field.permissions").into_owned(),
                            escalation.requested_permissions.join(", "),
                        );
                    }
                    push_optional_field(
                        &mut lines,
                        palette,
                        t!("app.field.justification").into_owned(),
                        escalation.justification.as_deref(),
                    );
                    if !escalation.suggested_prefix_rule.is_empty() {
                        push_field(
                            &mut lines,
                            palette,
                            "prefix",
                            escalation.suggested_prefix_rule.join(" "),
                        );
                    }
                }
            }
            _ => {}
        }

        lines.push(Line::from(""));
    }

    lines.extend(
        approval
            .body
            .lines()
            .map(|line| Line::from(Span::styled(line.to_string(), palette.text()))),
    );
    lines
}

fn diff_line_sign(kind: &str) -> &'static str {
    match kind {
        "added" => "+",
        "removed" => "-",
        "context" => " ",
        _ => "?",
    }
}

fn diff_line_style(kind: &str, palette: Palette) -> Style {
    match kind {
        "added" => Style::default().fg(palette.success).bg(palette.success_bg),
        "removed" => Style::default().fg(palette.danger).bg(palette.danger_bg),
        "context" => palette.text().bg(palette.diff_context_bg),
        _ => palette.text().bg(palette.surface_alt),
    }
}

fn diff_line_marker_style(kind: &str, palette: Palette) -> Style {
    diff_line_style(kind, palette).add_modifier(Modifier::BOLD)
}

fn diff_line_gutter_style(kind: &str, palette: Palette) -> Style {
    match kind {
        "added" => Style::default().fg(palette.success).bg(palette.success_bg),
        "removed" => Style::default().fg(palette.danger).bg(palette.danger_bg),
        "context" => palette.muted().bg(palette.diff_context_bg),
        _ => palette.muted().bg(palette.surface_alt),
    }
}

fn diff_file_status_style(status: &str, palette: Palette) -> Style {
    match status {
        "added" | "created" => Style::default()
            .fg(palette.success)
            .add_modifier(Modifier::BOLD),
        "deleted" | "removed" => Style::default()
            .fg(palette.danger)
            .add_modifier(Modifier::BOLD),
        _ => palette.selected().add_modifier(Modifier::BOLD),
    }
}

fn diff_hunk_style(palette: Palette) -> Style {
    Style::default()
        .fg(palette.accent)
        .bg(palette.diff_context_bg)
        .add_modifier(Modifier::BOLD)
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical_margin = (100 - percent_y) / 2;
    let horizontal_margin = (100 - percent_x) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(vertical_margin),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(vertical_margin),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horizontal_margin),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(horizontal_margin),
        ])
        .split(vertical[1])[1]
}

fn titled_block<'a>(
    title: impl Into<String>,
    palette: Palette,
    focused: bool,
    suffix: Option<String>,
) -> Block<'a> {
    let mut spans = vec![Span::styled(title.into(), palette.title())];
    if let Some(suffix) = suffix {
        spans.push(Span::styled(format!("  {suffix}"), palette.muted()));
    }
    if focused {
        spans.push(Span::styled("  ●", palette.selected()));
    }

    Block::default()
        .borders(Borders::ALL)
        .title(Line::from(spans))
}

// #365 step 2: themed extractions — see each module header.
// Formerly-`pub` fns keep their PUBLIC paths via explicit re-exports
// (the glob below is only pub(crate); a glob cannot widen visibility).
pub use activity_nav::{activity_navigator_areas, activity_navigator_model};
pub use render::{
    render, render_inline_overlay, render_viewport, render_viewport_with_finalization,
    scrollbar_thumb,
};
pub use transcript_build::{
    committed_activity_keys_for_live_finalization, committed_messages_fingerprint,
    committed_reply_matches_live_finalization, finalized_history_lines,
    finalized_history_lines_range, finalized_history_lines_range_dedup_live,
    finalized_late_activity_lines_for_coverages, finalized_live_turn_lines_between, live_ui_height,
    live_ui_height_with_finalization,
};
mod transcript_build;
#[allow(unused_imports)]
pub(crate) use transcript_build::*;
mod render;
#[allow(unused_imports)]
pub(crate) use render::*;
mod activity_nav;
#[allow(unused_imports)]
pub(crate) use activity_nav::*;

#[cfg(test)]
mod tests;
