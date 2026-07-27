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
        GoalObjectiveFold, PeerMeta, PlanStep as RenderedPlanStep, SessionAutonomyState,
        SessionRunState, SessionView, TaskOutputDetailState, TaskView, ThreadGraphDetailState,
        TurnActivityLog, TurnPromptAnchor, TurnStateDetailState, UserQuestionEntry,
        UserQuestionPickerState, extract_plan_steps, task_state_label,
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
    /// Whether any peer sessions are open. When true, the idle status bar swaps
    /// in a peer-aware key hint (`Ctrl+L peers | Ctrl+S sessions`) so the fleet
    /// and the way to reach a blocked peer are discoverable from the composer.
    pub peers_present: bool,
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
    HintBarModel {
        mode,
        peers_present: !app.peer_session_meta.is_empty(),
    }
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
/// A focused peer is a READ-ONLY watch surface: it has no editable composer, so
/// it reserves a single dim status row (steer peers from the master) instead of
/// the full bordered box — the reclaimed rows go to the peer's transcript.
const PEER_READONLY_BAR_ROWS: u16 = 1;

fn composer_height_for_size(app: &AppState, terminal_width: u16, terminal_height: u16) -> u16 {
    if app.focused_session_is_peer() {
        return PEER_READONLY_BAR_ROWS;
    }
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
pub(crate) fn launch_banner_active(app: &AppState) -> bool {
    app.pending_messages.is_empty()
        && app
            .active_session()
            .is_some_and(|session| session.messages.is_empty() && session.live_reply.is_none())
}

/// Claude-Code-style launch banner: a rounded box with the OCTOS logo, a
/// greeting, and the workspace path. No right-hand panel (per product call).
/// Rendered at the TOP of the transcript area for an empty session.
fn render_launch_banner(frame: &mut impl FrameLike, app: &AppState, palette: Palette, area: Rect) {
    let width = area.width as usize;
    if width < 12 || area.height < 6 {
        return;
    }
    let inner_w = width - 2;
    let show_figlet = area.width >= 48 && area.height >= 14;
    let border = Style::default().fg(palette.frame);
    let accent = Style::default()
        .fg(palette.accent)
        .add_modifier(Modifier::BOLD);
    let highlight = Style::default()
        .fg(palette.highlight)
        .add_modifier(Modifier::BOLD);

    // A content row: `│` + centered content (display width `content_w`) + `│`.
    let centered = |content: Vec<Span<'static>>, content_w: usize| -> Line<'static> {
        let pad = inner_w.saturating_sub(content_w);
        let left = pad / 2;
        let right = pad - left;
        let mut spans = vec![Span::styled("│", border), Span::raw(" ".repeat(left))];
        spans.extend(content);
        spans.push(Span::raw(" ".repeat(right)));
        spans.push(Span::styled("│", border));
        Line::from(spans)
    };

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Top border with an embedded title.
    let title = "─ octos ─";
    let top_dashes = inner_w.saturating_sub(title.chars().count());
    lines.push(Line::from(vec![
        Span::styled("╭", border),
        Span::styled(title, accent),
        Span::styled(format!("{}╮", "─".repeat(top_dashes)), border),
    ]));
    lines.push(centered(vec![], 0));
    if show_figlet {
        let fig_w = ONBOARDING_LOGO_ART
            .lines()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        for art in ONBOARDING_LOGO_ART.lines() {
            lines.push(centered(
                vec![Span::styled(format!("{art:<fig_w$}"), accent)],
                fig_w,
            ));
        }
        lines.push(centered(vec![], 0));
    }
    let greeting = match app
        .active_session()
        .and_then(|session| session.profile_id.as_deref())
    {
        Some(profile) => t!("app.banner.greeting_named", profile = profile).to_string(),
        None => t!("app.banner.greeting_default").to_string(),
    };
    let greeting_w = greeting.width();
    lines.push(centered(
        vec![Span::styled(greeting, highlight)],
        greeting_w,
    ));
    let cwd = short_path(app.workspace.root.as_str());
    let cwd_w = cwd.width();
    lines.push(centered(vec![Span::styled(cwd, palette.muted())], cwd_w));
    lines.push(centered(vec![], 0));
    let hint = t!("app.banner.hint").to_string();
    let hint_w = hint.width();
    lines.push(centered(vec![Span::styled(hint, palette.muted())], hint_w));
    lines.push(Line::from(Span::styled(
        format!("╰{}╯", "─".repeat(inner_w)),
        border,
    )));

    let banner_height = (lines.len() as u16).min(area.height);
    let banner_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: banner_height,
    };
    frame.render_widget(Paragraph::new(Text::from(lines)), banner_area);
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
/// at the 160ms tick in [`spinner_frame`] that is a 960ms swirl + a 320ms
/// sparkle per 1280ms cycle. Every frame is exactly one terminal cell wide
/// (ambiguous-width-but-1 glyphs; same shipped precedent as ✻ / ⚠), which the
/// fixed marker layout math depends on.
const SPINNER_FRAMES: [&str; 8] = ["◜", "◠", "◝", "◞", "◡", "◟", "✦", "✧"];

/// Current spinner frame, advancing ~every 160ms off a process-lifetime clock
/// (independent of any turn timer, so it keeps animating while background
/// sub-agents run after the parent turn has finished). The event loop redraws
/// every ~25ms, so this reads as smooth motion.
fn spinner_frame() -> &'static str {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_millis();
    SPINNER_FRAMES[(elapsed / 160) as usize % SPINNER_FRAMES.len()]
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

/// Number of figlet rows to reveal based on elapsed time since the banner
/// first became active. Returns 0 when the timestamp is not yet set, and
/// clamps at the art's actual line count once the animation completes.
fn banner_visible_rows(start: Option<std::time::Instant>) -> usize {
    const ROW_INTERVAL_MS: u128 = 120;
    let total_rows = ONBOARDING_LOGO_ART.lines().count();
    match start {
        None => 0,
        Some(t) => ((t.elapsed().as_millis() / ROW_INTERVAL_MS) as usize + 1)
            .min(total_rows),
    }
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

        if !preview.files.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!("app.diff.select_stage_hint").into_owned(),
                    palette.selected(),
                ),
            ]));
        }

        if !preview.files.is_empty() {
            let file_idx = diff
                .selected_file
                .min(preview.files.len().saturating_sub(1));
            if let Some(file) = preview.files.get(file_idx) {
                push_diff_file_lines(
                    lines,
                    palette,
                    file_idx,
                    diff.selected_file,
                    diff.selected_hunk,
                    file,
                    expanded,
                );
            }
        }
        if preview.files.len() > 1 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!(
                        "app.diff.more_files_hidden",
                        count = preview.files.len() - 1
                    )
                    .into_owned(),
                    palette.muted(),
                ),
            ]));
        }
    } else if diff.loading {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(t!("app.diff.loading").to_string(), palette.selected()),
        ]));
    } else if let Some(error) = &diff.error {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(error.clone(), Style::default().fg(palette.danger)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(t!("app.empty.no_diff").to_string(), palette.muted()),
        ]));
    }
}

fn push_diff_content_line(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    line: &crate::model::DiffPreviewLine,
) {
    let sign = diff_line_sign(&line.kind);
    let old_line = line
        .old_line
        .map(|line| line.to_string())
        .unwrap_or_else(|| "-".into());
    let new_line = line
        .new_line
        .map(|line| line.to_string())
        .unwrap_or_else(|| "-".into());
    let marker_style = diff_line_marker_style(&line.kind, palette);
    let gutter_style = diff_line_gutter_style(&line.kind, palette);
    let body_style = diff_line_style(&line.kind, palette);
    lines.push(Line::from(vec![
        Span::styled("    ", gutter_style),
        Span::styled(format!("{sign} "), marker_style),
        Span::styled(format!("{old_line:>4} {new_line:>4} "), gutter_style),
        Span::styled(line.content.clone(), body_style),
    ]));
}

fn push_diff_file_lines(
    lines: &mut Vec<Line<'static>>,
    palette: Palette,
    file_idx: usize,
    selected_file: usize,
    selected_hunk: usize,
    file: &crate::model::DiffPreviewFile,
    expanded: bool,
) {
    let path = match &file.old_path {
        Some(old_path) if old_path != &file.path => format!("{old_path} -> {}", file.path),
        _ => file.path.clone(),
    };
    let (added, removed) = diff_file_line_counts(file);
    let badge = diff_file_type_badge(&file.path);
    lines.push(Line::from(vec![
        Span::styled("    ", palette.muted()),
        Span::styled(
            format!(" {badge:<5} "),
            diff_file_badge_style(badge, palette),
        ),
        Span::styled(" ", palette.muted()),
        Span::styled(
            file.status.clone(),
            diff_file_status_style(&file.status, palette),
        ),
        Span::styled("  ", palette.muted()),
        Span::styled(format!("+{added} "), Style::default().fg(palette.success)),
        Span::styled(format!("-{removed} "), Style::default().fg(palette.danger)),
        Span::styled(" ", palette.muted()),
        Span::styled(path, palette.text().add_modifier(Modifier::BOLD)),
    ]));

    if file.hunks.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.diff.line_unavailable").into_owned(),
                palette.muted(),
            ),
        ]));
    }

    let hunk_idx = selected_hunk.min(file.hunks.len().saturating_sub(1));

    if expanded {
        // Ctrl+O review mode for staging: show EVERY hunk header so the diff
        // structure stays navigable, and the SELECTED hunk's COMPLETE body so
        // the user can see exactly what they are about to stage (the collapsed
        // view caps each hunk at 4 lines, which is the "can't see the diff"
        // complaint). Non-selected hunks stay header-only to keep the inline
        // view bounded; navigate with the hunk keys to expand another.
        for (idx, hunk) in file.hunks.iter().enumerate() {
            let selected = file_idx == selected_file && idx == selected_hunk;
            let marker = if selected { "  › " } else { "  ├ " };
            lines.push(Line::from(vec![
                Span::styled(marker, palette.selected()),
                Span::styled(hunk.header.clone(), diff_hunk_style(palette)),
            ]));
            if selected {
                for line in &hunk.lines {
                    push_diff_content_line(lines, palette, line);
                }
            }
        }
        return;
    }

    if hunk_idx > 0 {
        lines.push(Line::from(vec![
            Span::styled("    ", palette.muted()),
            Span::styled(
                t!("app.diff.more_hunks_hidden", count = hunk_idx).into_owned(),
                palette.muted(),
            ),
        ]));
    }
    for (rendered_hunk_idx, hunk) in file.hunks.iter().enumerate().skip(hunk_idx).take(1) {
        let hunk_idx = rendered_hunk_idx;
        let selected = file_idx == selected_file && hunk_idx == selected_hunk;
        let marker = if selected { "  › " } else { "  ├ " };
        lines.push(Line::from(vec![
            Span::styled(marker, palette.selected()),
            Span::styled(hunk.header.clone(), diff_hunk_style(palette)),
        ]));
        for line in hunk.lines.iter().take(4) {
            push_diff_content_line(lines, palette, line);
        }
        if hunk.lines.len() > 4 {
            lines.push(Line::from(vec![
                Span::styled("    ", palette.muted()),
                Span::styled(
                    t!("app.diff.more_lines_hidden", count = hunk.lines.len() - 4).into_owned(),
                    palette.muted(),
                ),
            ]));
        }
    }
    if file.hunks.len() > 1 {
        let hidden_after = file.hunks.len().saturating_sub(hunk_idx.saturating_add(1));
        if hidden_after == 0 {
            return;
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
    /// Index (into the draft's logical lines) of the first VISIBLE line —
    /// i.e. how many whole draft lines scrolled off above the window. Lets
    /// the renderer replay markdown fence state over the hidden prefix so a
    /// ``` block whose opener scrolled away keeps its interior styling.
    first_line_index: usize,
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

/// #407: the durable peer roster — sessions tracked for their whole lifetime,
/// not just the open-in-flight window. Returns `(session_id, slug, meta)`
/// triples sorted deterministically by `(created, session_key)` so a fleet
/// staged in one burst (where `Instant::now()` can tie on coarse clocks)
/// doesn't flicker row order across frames (review F10).
pub(crate) fn peer_dock_roster(app: &AppState) -> Vec<(&octos_core::SessionKey, &PeerMeta)> {
    let mut peers: Vec<_> = app.peer_session_meta.iter().collect();
    // Stable tie-break on the session key string — deterministic regardless
    // of HashMap iteration order or clock granularity.
    peers.sort_by(|a, b| {
        a.1.created
            .cmp(&b.1.created)
            .then_with(|| a.0.0.cmp(&b.0.0))
    });
    peers
}

/// #407: `(total, live, blocked, unread)` counts for the Peer Dock pill.
/// - `total`   = durable roster size + still-pending (opening) peers
/// - `live`    = peer's turn is streaming or pre-token-armed RIGHT NOW
/// - `blocked` = peer has a stashed approval/question waiting on the user (⚠)
/// - `unread`  = peer has turn-terminals the user hasn't focused since
///
/// Review F1: keys off the durable `peer_session_meta` roster, NOT
/// `pending_peer_kickoffs` (which empties the moment a peer opens, making
/// the old counts structurally ~0). Live/blocked/unread are looked up in
/// `app.sessions`, which an opened peer IS in — so the counts are real.
pub(crate) fn peer_dock_counts(app: &AppState) -> (usize, usize, usize, usize) {
    let roster = peer_dock_roster(app);
    let total = roster.len() + app.pending_peer_kickoffs.len();
    if total == 0 {
        return (0, 0, 0, 0);
    }
    let mut live = 0usize;
    let mut blocked = 0usize;
    let mut unread = 0usize;
    for (session_id, _meta) in &roster {
        if app.session_turn_live(session_id) {
            live += 1;
        }
        if app.session_blocked_reason(session_id).is_some() {
            blocked += 1;
        }
        if let Some(n) = app.unread_turns.get(*session_id).copied() {
            if n > 0 {
                unread += 1;
            }
        }
    }
    (total, live, blocked, unread)
}

/// #407: the collapsed Peer Dock pill — one glanceable line mirroring the
/// agent dock pill: `👥 N peers · M live · K⚠ waiting · J● unread — Ctrl+J`.
/// The `blocked` and `unread` segments appear only when non-zero so a calm
/// fleet reads as just `👥 2 peers · 1 live`.
pub(crate) fn peer_dock_pill_line(app: &AppState, palette: Palette) -> Line<'static> {
    let (total, live, blocked, unread) = peer_dock_counts(app);
    let mut spans: Vec<Span<'static>> = vec![Span::styled(
        t!(
            "app.hint.peer_dock_pill",
            count = total.to_string(),
            live = live.to_string()
        )
        .into_owned(),
        palette.text().bg(palette.surface),
    )];
    if blocked > 0 {
        spans.push(Span::styled(
            t!(
                "app.hint.peer_dock_pill_blocked",
                count = blocked.to_string()
            )
            .into_owned(),
            Style::default()
                .fg(palette.highlight)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if unread > 0 {
        spans.push(Span::styled(
            t!("app.hint.peer_dock_pill_unread", count = unread.to_string()).into_owned(),
            Style::default()
                .fg(palette.highlight)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", t!("app.hint.peer_dock_toggle_hint")),
        palette.muted().bg(palette.surface),
    ));
    Line::from(spans)
}

/// #407: one-line activity summary for a peer row in the dock — blocked
/// reason first (it needs the user), then the live stream tail, then the
/// `opening…` placeholder for still-pending peers, else `idle`. Returns
/// `String` (review F11: the prior `Cow<'a, str>` was a fiction — every
/// arm allocated).
pub(crate) fn peer_activity_line(app: &AppState, session_id: &octos_core::SessionKey) -> String {
    // Blocked reason wins — a peer waiting on you is the row's whole point.
    if let Some(reason) = app.session_blocked_reason(session_id) {
        return t!(
            "app.hint.peer_dock_row_blocked",
            reason = reason.to_string()
        )
        .into_owned();
    }
    if app.sessions.iter().any(|s| &s.id == session_id) {
        match app.session_activity_line(session_id) {
            Some(line) => line,
            None => t!("app.hint.peer_dock_row_idle").into_owned(),
        }
    } else {
        // Pending open — brief staged, session/opened in flight. NOT idle.
        t!("app.hint.peer_dock_row_opening").into_owned()
    }
}

/// #407: terminal-height floor for the Peer Dock. Below this many rows the
/// dock collapses to height 0 so a tiny terminal never corrupts the layout.
/// Mirrors [`AGENT_STRIP_MIN_TERMINAL_ROWS`].
const PEER_STRIP_MIN_TERMINAL_ROWS: u16 = 12;

/// #407: max peer rows the dock may claim below its title row. Larger fleets
/// stay fully reachable via `+N` on the title row + the session strip (Alt+S).
/// Mirrors [`AGENT_STRIP_MAX_AGENT_ROWS`].
const PEER_STRIP_MAX_PEER_ROWS: u16 = 4;

/// #407: rows the Peer Dock occupies under the composer — mirrors
/// [`agent_strip_height`]: 0 when no peers / transcript pager active /
/// terminal too short, 1 when collapsed (the pill row — this IS the layout
/// reservation for the collapsed fleet summary), or `1 + capped_rows` when
/// expanded. Both the height reservation and the render pass call this with
/// the same terminal height so they always agree.
pub(crate) fn peer_strip_height(app: &AppState, terminal_height: u16) -> u16 {
    if app.transcript_pager_active
        || terminal_height < PEER_STRIP_MIN_TERMINAL_ROWS
        || peer_dock_roster(app).is_empty()
    {
        0
    } else if app.peer_dock_collapsed {
        1
    } else {
        1 + peer_strip_peer_rows(app, terminal_height)
    }
}

/// #407: visible peer rows — capped by [`PEER_STRIP_MAX_PEER_ROWS`] and by
/// the rows the terminal can spare beyond [`PEER_STRIP_MIN_TERMINAL_ROWS`].
/// Mirrors [`agent_strip_agent_rows`].
fn peer_strip_peer_rows(app: &AppState, terminal_height: u16) -> u16 {
    let roster = peer_dock_roster(app).len().min(u16::MAX as usize) as u16;
    roster
        .min(PEER_STRIP_MAX_PEER_ROWS)
        .min(terminal_height.saturating_sub(PEER_STRIP_MIN_TERMINAL_ROWS))
}

/// The peer session keys whose rows the Peer Dock ACTUALLY DRAWS at
/// `terminal_height` this frame — the roster prefix `peer_strip_lines` renders
/// (`roster.iter().take(rows)`), or empty when the dock is collapsed (the pill
/// shows no per-peer affordance) or height-0 (pager active / terminal too short
/// / no peers). Used to gate the dock's approve/deny keys so a peer whose ⚠
/// affordance is off-screen (below the row cap) or hidden can't be actioned.
pub(crate) fn visible_peer_dock_keys(
    app: &AppState,
    terminal_height: u16,
) -> Vec<octos_core::SessionKey> {
    if app.peer_dock_collapsed || peer_strip_height(app, terminal_height) == 0 {
        return Vec::new();
    }
    let rows = peer_strip_peer_rows(app, terminal_height) as usize;
    peer_dock_roster(app)
        .into_iter()
        .take(rows)
        .map(|(session_id, _)| session_id.clone())
        .collect()
}

/// #407: logical lines for the vertical Peer Dock. Row 0 is the title row
/// (the collapsed pill when `peer_dock_collapsed`). Each following row is
/// one peer: glyph (⚠ blocked / ✻ live / ○ idle) + slug + muted activity
/// detail. Mirrors [`agent_strip_lines`]. Split from rendering so the layout
/// logic is unit-testable without a frame; `peer_rows` must be the value the
/// height reservation was computed with (`peer_strip_height` - 1).
pub(crate) fn peer_strip_lines(
    app: &AppState,
    palette: Palette,
    peer_rows: u16,
) -> Vec<Line<'static>> {
    if app.peer_dock_collapsed {
        return vec![peer_dock_pill_line(app, palette)];
    }
    let roster = peer_dock_roster(app);
    let total = roster.len();
    let rows = (peer_rows as usize).min(total);
    let hidden = total - rows;
    let mut lines: Vec<Line<'static>> = Vec::new();
    // Title row: peer dock title + overflow marker when the roster is larger
    // than the visible window.
    let mut title_spans: Vec<Span<'static>> = vec![Span::styled(
        t!("app.hint.peer_dock_title").into_owned(),
        palette.muted().bg(palette.surface),
    )];
    if hidden > 0 {
        title_spans.push(Span::styled(
            format!(" +{hidden} "),
            palette.muted().bg(palette.surface),
        ));
    }
    title_spans.push(Span::styled(
        format!("  {}", t!("app.hint.peer_dock_toggle_hint")),
        palette.muted().bg(palette.surface),
    ));
    lines.push(Line::from(title_spans));
    // One row per visible peer.
    for (session_id, meta) in roster.iter().take(rows) {
        let blocked = app.session_blocked_reason(session_id).is_some();
        let live = app.session_turn_live(session_id);
        let done = app.peer_is_done(session_id);
        // Priority: blocked (needs you) > live (streaming) > done (finished) >
        // idle (opened, never run). `✓` done reads distinctly from `○` idle so a
        // finished fleet is obvious at a glance instead of looking un-started.
        let glyph = if blocked {
            "⚠"
        } else if live {
            "✻"
        } else if done {
            "✓"
        } else {
            "○"
        };
        let glyph_style = if blocked {
            Style::default()
                .fg(palette.highlight)
                .bg(palette.surface)
                .add_modifier(Modifier::BOLD)
        } else if live {
            Style::default().fg(palette.accent).bg(palette.surface)
        } else if done {
            palette.text().bg(palette.surface)
        } else {
            palette.muted().bg(palette.surface)
        };
        let detail = peer_activity_line(app, session_id);
        let mut row_spans = vec![
            Span::styled(format!(" {glyph} "), glyph_style),
            Span::styled(
                meta.slug.chars().take(20).collect::<String>(),
                palette.text().bg(palette.surface),
            ),
            Span::styled(format!("  {detail}"), palette.muted().bg(palette.surface)),
        ];
        // Mirror the agent dock's run stats so the fleet's progress/cost reads
        // at a glance without opening each peer: elapsed since the peer was
        // opened, then cumulative received (↓) tokens for its session. Tokens
        // come from `session_usage`, which `apply_progress` keys by the event's
        // session_id — so a background peer's usage lands here just like the
        // focused session's.
        // Freeze the elapsed at the run duration (created→finished_at) once the
        // peer is done, instead of letting "age since opened" tick up forever.
        let elapsed_ms = if done {
            meta.finished_at
                .map(|finished| finished.saturating_duration_since(meta.created).as_millis() as i64)
                .unwrap_or(0)
        } else {
            meta.created.elapsed().as_millis() as i64
        };
        let elapsed = format_short_duration(elapsed_ms);
        row_spans.push(Span::styled(
            format!("  · {elapsed}"),
            palette.muted().bg(palette.surface),
        ));
        if let Some((_input, Some(output), _cost)) = app.session_usage.get(*session_id) {
            row_spans.push(Span::styled(
                format!(" · ↓ {}", humanize_token_count(*output)),
                palette.muted().bg(palette.surface),
            ));
        }
        // Peer operator console: a peer with a stashed approval gets an
        // actionable affordance on its row so the operator answers it from the
        // master via Alt+Y / Alt+N (see the event loop) WITHOUT switching to the
        // peer. Only APPROVALS get the yes/no affordance (a question-blocked peer
        // needs the picker); the ⚠ glyph + `peer_activity_line` already carry the
        // reason. Kept INLINE — no extra line — so the one-row-per-peer height
        // reservation (`peer_strip_height`) stays exact.
        if app.pending_session_approvals.contains_key(*session_id) {
            row_spans.push(Span::styled(
                "  [Alt+Y approve · Alt+N deny]".to_string(),
                Style::default()
                    .fg(palette.highlight)
                    .bg(palette.surface)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(row_spans));
    }
    lines
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
    // No caret on a read-only peer bar (there is no input surface). The height-1
    // area already yields None from `composer_cursor_position`; this is the
    // explicit intent guard.
    if app.focus != FocusPane::Composer || app.focused_session_is_peer() {
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
    let mut first_line_index = 0usize;

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
            first_line_index = index;
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
        first_line_index = index;
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
        first_line_index,
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

fn render_status(app: &AppState, palette: Palette) -> Paragraph<'static> {
    let mode = if app.readonly {
        t!("app.status.read_only").to_string()
    } else {
        t!("app.status.interactive").to_string()
    };
    let turn = app
        .active_turn()
        .map(|(_, turn_id)| {
            t!(
                "app.status.turn_active",
                id = short_id(&turn_id.0.to_string())
            )
            .to_string()
        })
        .unwrap_or_else(|| t!("app.status.turn_idle").to_string());
    let profile = app
        .active_session()
        .and_then(|session| session.profile_id.as_deref())
        .unwrap_or("default");
    let cwd = app.workspace.root.as_str();
    let policy = if app.readonly {
        t!("app.status.sends_disabled").to_string()
    } else {
        t!("app.status.approval_gated").to_string()
    };
    let context = app
        .active_session()
        .map(|session| {
            t!(
                "app.statusbar.msgs_tasks",
                msgs = session.messages.len(),
                tasks = session.tasks.len()
            )
            .into_owned()
        })
        .unwrap_or_else(|| t!("app.status.no_session").to_string());
    let work = status_bar_work_text(app);
    let key_hint = hint_bar_text(hint_bar_model(app));

    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", t!("app.status.state_label")),
            palette.title().bg(palette.surface_alt),
        ),
        Span::styled(
            run_state_marker(&app.run_state).to_string(),
            run_state_style(&app.run_state, palette).bg(palette.surface_alt),
        ),
        Span::styled(" ", palette.muted().bg(palette.surface_alt)),
        Span::styled(
            run_state_status_label(&app.run_state).to_string(),
            run_state_style(&app.run_state, palette).bg(palette.surface_alt),
        ),
        Span::styled(format!(" {work}"), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(policy.to_string(), palette.text().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(profile.to_string(), palette.text().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(context, palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(app.status.clone(), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(
            format!("{mode} {turn}"),
            palette.muted().bg(palette.surface_alt),
        ),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(short_path(cwd), palette.muted().bg(palette.surface_alt)),
        Span::styled(" | ", palette.muted().bg(palette.surface_alt)),
        Span::styled(key_hint, palette.selected().bg(palette.surface_alt)),
    ]))
    .style(Style::default().fg(palette.text).bg(palette.surface_alt))
}

fn hint_bar_text(model: HintBarModel) -> String {
    match model.mode {
        HintBarMode::StatusbarKeys if model.peers_present => {
            t!("app.hint.statusbar_keys_peers").into_owned()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cli::ThemeName,
        model::{
            ApprovalModalState, DiffPreview, DiffPreviewFile, DiffPreviewGetResult,
            DiffPreviewHunk, DiffPreviewLine, SessionView,
        },
        store::Store,
        viewport::ScrollbackTracker,
    };
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
        assert!(text.contains("Tab inspector"));
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
        // Single-select uses radio parens, not checkbox brackets.
        assert!(text.contains("( ) axum"));
        assert!(text.contains("( ) actix"));
        // The always-present free-text "Other" row.
        assert!(text.contains("Other"));
        assert!(text.contains("Enter = submit answer(s)"));
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

        // Multi-select uses checkbox brackets.
        assert!(text.contains("[ ] stable"));
        assert!(text.contains("[ ] nightly"));
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
        assert!(text.contains("state • Working"));
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
    fn render_assistant_markdown_is_left_aligned_without_marker_leakage() {
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
        assert_eq!(bullet.find("- "), Some(0));
        // The table is now drawn as a real bordered grid, so its rows start with
        // the box border rather than the raw cell text — still no marker leakage.
        assert!(table.starts_with("│"));
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
        assert!(text.contains("Create your local Octos profile"));
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

    /// Regression: the harness-row context `LineGauge` label (`ctx ~N%`) must
    /// inherit the theme `surface` background. `LineGauge` paints its whole
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

        // The gauge label is rendered on the harness row (the `ctx ~N%` text).
        let label_style = style_for_text(&buffer, "ctx ~").expect("gauge label rendered");
        assert_eq!(
            label_style.bg,
            Some(palette.surface),
            "gauge label must use the surface bg, not the raw terminal background"
        );

        // The whole gauge column (label + filled/unfilled line) must be a single
        // contiguous surface-backed band — no stray bg=Reset cells.
        let rows = rendered_rows(&buffer);
        let gauge_row = row_index_containing(&rows, "ctx ~");
        let width = usize::from(buffer.area.width);
        let row_start = gauge_row * width;
        let first_label_col = rows[gauge_row].find("ctx ~").expect("label col");
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
        assert!(text.contains("[paste] Pasted block"));
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
        assert!(text.contains("Tested"));
        assert!(text.contains("$ cargo test"));
        assert!(text.contains("running 6 tests"));
        assert!(text.contains("1 more line(s) hidden (Ctrl+O expand)"));
        assert!(text.contains("1.2s"));
        assert!(!text.contains("Progress"));
        assert!(!text.contains("Work  sticky"));
        assert!(text.contains("call call-1"));
        assert!(text.contains("gpt-5-codex"));
        assert!(text.contains("state"));
        assert!(text.contains("running"));
        assert!(text.contains("approval"));
        assert!(text.contains("1 msgs/0 tasks"));
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
        let command = text.find("$ cargo test").expect("activity command");

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
        let command = text.find("$ cargo build").expect("tool command");
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

        assert!(text.contains("Waited"));
        assert!(text.contains("20s"));
        assert!(text.contains("Wrote"));
        assert!(text.contains("18ms"));
        assert!(!text.contains("Command  ▸ shell"));
        assert!(!text.contains("Tool  ▸ write_file"));
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
        assert!(text.contains("preview ready"));
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

        assert!(text.contains("failed"));
        assert!(text.contains("✗"));
        assert!(text.contains("✓"));
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
    fn render_diff_preview_modal_includes_status_files_and_hunks() {
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
        assert!(text.contains("ready"));
        assert!(text.contains("pending_store"));
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
        assert_eq!(autonomy_indicator_height(&app), 0);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
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

        assert_eq!(autonomy_indicator_height(&app), 1);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert_eq!(lines.len(), 1);

        let text = rendered_text(&app);
        assert!(
            text.contains("Goal:"),
            "goal row must surface 'Goal:' label"
        );
        assert!(text.contains("finish the OAuth refactor"));
        assert!(text.contains("active"));
        assert!(text.contains("12000/50000"));
        assert!(!text.contains("Loops:"), "loops row must be hidden");
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

        assert_eq!(autonomy_indicator_height(&app), 2);
        let lines = autonomy_indicator_lines(&app, Palette::for_theme(ThemeName::Codex));
        assert_eq!(lines.len(), 2);

        let text = rendered_text(&app);
        assert!(text.contains("Goal:"));
        assert!(text.contains("finish OAuth refactor"));
        assert!(text.contains("Loops: 2 running"));
        assert!(text.contains("5m deploy-check"));
        assert!(text.contains("self-paced PR-watch"));
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
            text.contains("ctx ~50%"),
            "ctx % from token_estimate (approximate marker): {text:?}"
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
            rendered.contains("Tab inspector"),
            "composer hint not clobbered: {rendered:?}"
        );
        // Regression (duplicate ctx%): on a wide terminal (rendered_text uses
        // 120 cols, so the gauge column is drawn) the percent must render ONCE —
        // as the LineGauge on the right, NOT also as the textual `· ctx ~N%`
        // label on the left. Pre-fix this row showed both "· ctx ~50%" and
        // "ctx ~50% ───" on the same line.
        assert_eq!(
            rendered.matches("ctx ~").count(),
            1,
            "ctx% must render exactly once (gauge only) on a wide terminal: {rendered:?}"
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
            text.contains("ctx ~25%"),
            "ctx label must carry the approximate marker: {text:?}"
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
        // composer's top-border chrome ("Composer  Enter send | Tab inspector")
        // is fully intact — the collision that caused the prior revert
        // (249fe652) cannot recur because the indicator is never on the border.
        let app = autonomy_app_state();
        assert_eq!(harness_status_height(&app), 0);
        let text = rendered_text(&app);
        assert!(text.contains("Composer"), "{text:?}");
        assert!(text.contains("Tab inspector"), "{text:?}");
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
            super::live_ui_height_with_finalization(app, width, height, live_finalization);
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
            inserted.contains("Agent task completed") && inserted.contains("$ cargo test"),
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
        assert!(first_text.contains("$ cargo test"));

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
            !second_text.contains("$ cargo test"),
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
            text.contains("$ cargo build"),
            "missing tool detail: {text:?}"
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
        let h = super::live_ui_height(&app, 100, height);
        assert!(
            h <= height.saturating_sub(super::LIVE_VIEWPORT_MIN_SCROLLBACK),
            "live UI height {h} must leave >= {} rows of scrollback on a {height}-row screen",
            super::LIVE_VIEWPORT_MIN_SCROLLBACK
        );
        assert!(h >= 1);
    }

    #[test]
    fn wants_fullscreen_overlay_tracks_inspector_and_modals() {
        let mut app = chat_app(vec![Message::user("hi")]);
        assert!(
            !super::wants_fullscreen_overlay(&app),
            "plain chat should use the inline viewport, not alt-screen"
        );
        app.focus = FocusPane::Workspace;
        assert!(
            super::wants_fullscreen_overlay(&app),
            "inspector panes should use the full-screen overlay"
        );
        app.focus = FocusPane::Composer;
        app.task_output.active = true;
        assert!(
            super::wants_fullscreen_overlay(&app),
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
