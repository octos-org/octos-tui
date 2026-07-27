//! M15-E autonomy command parsing for `/agents`, `/goal`, and `/loop`.
//!
//! Parses user-typed slash commands into typed intents the TUI can
//! dispatch as backend Octos UI calls. The parser is the source of truth
//! for syntax shape — actual dispatch (issuing `agent/list`,
//! `session/goal/set`, `loop/create`, …) is wired in a later PR once
//! the backend exposes those Octos UI methods.
//!
//! Contract reference: octos-tui#47 (M15-E) and upstream
//! `UPCR-2026-021` (Agent / Goal / Loop autonomy). The canonical spec
//! lives at
//! `octos/docs/OCTOS_UI_PROTOCOL_CHANGE_REQUEST_UPCR_2026_021_AGENT_GOAL_LOOP_AUTONOMY.md`
//! in the upstream octos repo. The earlier
//! `docs/M15_AGENT_GOAL_LOOP_TUI_CONTRACT.md` working title was never
//! landed — reference UPCR-2026-021 directly instead. The TUI must never:
//!
//! - Probe these methods on servers that did not advertise
//!   [`super::model::APPUI_FEATURE_CODING_AUTONOMY_V1`].
//! - Schedule timers locally for `/loop`. Loop firing is backend-owned.
//! - Invent default intervals; if the user did not supply one, the
//!   parsed intent records "self-paced" and the backend decides.
//!
//! Menu surface: `/agents`, `/goal`, and `/loop` are currently
//! discoverable only via slash entry. A menu-surface wrapper (the
//! sub-menu shape sketched in #74 acceptance criterion 2) is deferred
//! to M15-F UX work — no other autonomy slash commands have menu
//! surfaces today, so adding one in isolation here would create an
//! inconsistent UX. Tracking issue: octos-tui#74 (acceptance criterion
//! #2 — `Add menu entries in src/menu/providers.rs`).

use std::time::Duration;

/// Parsed `/agents` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsCommand {
    /// `/agents` (no subcommand) or `/agents list`.
    List,
    /// `/agents status [<agent_id>]`. `None` ⇒ show all.
    Status(Option<String>),
    /// `/agents output <agent_id>`.
    Output(String),
    /// `/agents artifacts <agent_id>`.
    Artifacts(String),
    /// `/agents artifact <agent_id> <artifact_id>` or
    /// `/agents artifact <agent_id> path:<artifact_path>`.
    ArtifactRead {
        agent_id: String,
        selector: AgentArtifactSelector,
    },
    /// `/agents interrupt <agent_id>`.
    Interrupt(String),
    /// `/agents close <agent_id>`.
    Close(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentArtifactSelector {
    Id(String),
    Path(String),
}

/// Parsed `/task` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCommand {
    /// `/task artifact <task_id> <artifact_id>` or
    /// `/task artifact <task_id> path:<artifact_path>`.
    ArtifactRead {
        task_id: String,
        selector: TaskArtifactSelector,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskArtifactSelector {
    Id(String),
    Path(String),
}

/// Parsed `/thread` / `/threads` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadCommand {
    /// `/threads` or `/thread graph`.
    Graph,
}

/// Parsed `/turn` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnCommand {
    /// `/turn state [<turn_id>]`.
    State(Option<String>),
}

/// Parsed `/goal` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    /// `/goal <objective> [--budget <n>[k|m]]` — set (or re-budget) an
    /// active goal. `token_budget` is `None` when the user did not pass
    /// `--budget`, in which case the backend applies its own default.
    Set {
        objective: String,
        token_budget: Option<u64>,
    },
    /// Bare `/goal` — show current goal.
    Show,
    /// `/goal pause`.
    Pause,
    /// `/goal resume`.
    Resume,
    /// `/goal stop` — mark the goal complete (stops autonomous
    /// continuations for good; `clear` additionally forgets the record).
    Stop,
    /// `/goal clear`.
    Clear,
}

/// `/loop` cadence the user requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopCadence {
    /// Backend decides cadence (user did not supply `every <interval>`).
    SelfPaced,
    /// User requested a fixed cadence.
    Every(Duration),
    /// User asked for the maintenance cadence (long, backend-tuned).
    Maintenance,
}

/// Parsed `/loop` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopCommand {
    /// `/loop` (bare → maintenance, empty prompt),
    /// `/loop <prompt>`, `/loop <interval> <prompt>`,
    /// `/loop <prompt> every <interval>`, `/loop maintenance <prompt>`.
    ///
    /// Per UPCR-2026-021 §"Parsing rules": bare `/loop` (no prompt, no
    /// interval, no verb) creates a maintenance loop. The backend
    /// resolves the prompt from `.octos/loop.md`, then `~/.octos/loop.md`,
    /// then a built-in fallback.
    Create {
        prompt: String,
        cadence: LoopCadence,
    },
    /// `/loop list`.
    List,
    /// `/loop delete <id>`.
    Delete(String),
    /// `/loop pause <id>`.
    Pause(String),
    /// `/loop resume <id>`.
    Resume(String),
    /// `/loop fire-now <id>`.
    FireNow(String),
}

/// Top-level parsed autonomy command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomyCommand {
    Agents(AgentsCommand),
    Task(TaskCommand),
    Thread(ThreadCommand),
    Turn(TurnCommand),
    Goal(GoalCommand),
    Loop(LoopCommand),
}

/// Errors the parser can raise. Each carries enough context that the
/// TUI can render a structured hint without inventing UX of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutonomyParseError {
    /// Caller passed something that isn't one of `/agents`, `/goal`,
    /// `/loop`.
    UnknownCommand(String),
    /// `/agents <verb>` where verb is unrecognized.
    UnknownAgentsVerb(String),
    /// `/task <verb>` where verb is unrecognized.
    UnknownTaskVerb(String),
    /// `/thread <verb>` where verb is unrecognized.
    UnknownThreadVerb(String),
    /// `/turn <verb>` where verb is unrecognized.
    UnknownTurnVerb(String),
    /// A subcommand that requires an `<agent_id>` or `<loop_id>` was
    /// missing one.
    MissingId { command: &'static str },
    /// `/loop` create with empty prompt.
    EmptyLoopPrompt,
    /// `/goal <objective>` with empty objective (after stripping
    /// whitespace).
    EmptyGoalObjective,
    /// `/goal ... --budget <value>` where the value was missing or did
    /// not parse as a positive token count.
    InvalidBudget(String),
    /// `/loop ... every <interval>` where the interval failed to parse.
    InvalidInterval(String),
}

impl std::fmt::Display for AutonomyParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCommand(name) => write!(f, "unknown autonomy command: /{name}"),
            Self::UnknownAgentsVerb(verb) => {
                write!(f, "unknown /agents subcommand: {verb}")
            }
            Self::UnknownTaskVerb(verb) => {
                write!(f, "unknown /task subcommand: {verb}")
            }
            Self::UnknownThreadVerb(verb) => {
                write!(f, "unknown /thread subcommand: {verb}")
            }
            Self::UnknownTurnVerb(verb) => {
                write!(f, "unknown /turn subcommand: {verb}")
            }
            Self::MissingId { command } => {
                write!(f, "{command} requires an id argument")
            }
            Self::EmptyLoopPrompt => f.write_str("/loop requires a prompt"),
            Self::EmptyGoalObjective => f.write_str("/goal requires an objective"),
            Self::InvalidBudget(raw) => {
                write!(
                    f,
                    "could not parse --budget `{raw}` (try 500k, 2m, or 50000)"
                )
            }
            Self::InvalidInterval(raw) => {
                write!(f, "could not parse interval `{raw}`")
            }
        }
    }
}

impl std::error::Error for AutonomyParseError {}

/// Parse a slash command (e.g. `/loop every 5m run tests`) into a
/// typed [`AutonomyCommand`]. The leading slash is optional.
///
/// Returns `Ok(None)` for an empty input. Unknown commands return
/// `Err`.
pub fn parse_autonomy_slash(input: &str) -> Result<Option<AutonomyCommand>, AutonomyParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let rest = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let (head, tail) = split_head(rest);
    match head {
        "agents" | "agent" => Ok(Some(AutonomyCommand::Agents(parse_agents(tail)?))),
        "task" => Ok(Some(AutonomyCommand::Task(parse_task(tail)?))),
        "thread" | "threads" => Ok(Some(AutonomyCommand::Thread(parse_thread(tail)?))),
        "turn" => Ok(Some(AutonomyCommand::Turn(parse_turn(tail)?))),
        "goal" => Ok(Some(AutonomyCommand::Goal(parse_goal(tail)?))),
        "loop" => Ok(Some(AutonomyCommand::Loop(parse_loop(tail)?))),
        other => Err(AutonomyParseError::UnknownCommand(other.to_string())),
    }
}

fn split_head(input: &str) -> (&str, &str) {
    match input.split_once(char::is_whitespace) {
        Some((head, tail)) => (head, tail.trim()),
        None => (input, ""),
    }
}

fn require_id(verb: &'static str, tail: &str) -> Result<String, AutonomyParseError> {
    let id = tail.trim();
    if id.is_empty() {
        Err(AutonomyParseError::MissingId { command: verb })
    } else {
        Ok(id.to_string())
    }
}

fn parse_agents(tail: &str) -> Result<AgentsCommand, AutonomyParseError> {
    let (verb, args) = split_head(tail);
    match verb {
        "" | "list" => Ok(AgentsCommand::List),
        "status" => {
            let id = args.trim();
            if id.is_empty() {
                Ok(AgentsCommand::Status(None))
            } else {
                Ok(AgentsCommand::Status(Some(id.to_string())))
            }
        }
        "output" => Ok(AgentsCommand::Output(require_id("/agents output", args)?)),
        "artifacts" => Ok(AgentsCommand::Artifacts(require_id(
            "/agents artifacts",
            args,
        )?)),
        "artifact" | "artifact-read" | "read-artifact" => parse_agent_artifact_read(args),
        "interrupt" => Ok(AgentsCommand::Interrupt(require_id(
            "/agents interrupt",
            args,
        )?)),
        "close" => Ok(AgentsCommand::Close(require_id("/agents close", args)?)),
        other => Err(AutonomyParseError::UnknownAgentsVerb(other.to_string())),
    }
}

fn parse_agent_artifact_read(args: &str) -> Result<AgentsCommand, AutonomyParseError> {
    let (agent_id, selector_raw) = split_head(args);
    if agent_id.is_empty() {
        return Err(AutonomyParseError::MissingId {
            command: "/agents artifact",
        });
    }
    let selector_raw = selector_raw.trim();
    if selector_raw.is_empty() {
        return Err(AutonomyParseError::MissingId {
            command: "/agents artifact <agent_id>",
        });
    }
    let selector = if let Some(path) = selector_raw.strip_prefix("path:") {
        let path = path.trim();
        if path.is_empty() {
            return Err(AutonomyParseError::MissingId {
                command: "/agents artifact <agent_id>",
            });
        }
        AgentArtifactSelector::Path(path.to_string())
    } else if let Some(id) = selector_raw.strip_prefix("id:") {
        let id = id.trim();
        if id.is_empty() {
            return Err(AutonomyParseError::MissingId {
                command: "/agents artifact <agent_id>",
            });
        }
        AgentArtifactSelector::Id(id.to_string())
    } else {
        AgentArtifactSelector::Id(selector_raw.to_string())
    };
    Ok(AgentsCommand::ArtifactRead {
        agent_id: agent_id.to_string(),
        selector,
    })
}

fn parse_task(tail: &str) -> Result<TaskCommand, AutonomyParseError> {
    let (verb, args) = split_head(tail);
    match verb {
        "artifact" | "artifact-read" | "read-artifact" => parse_task_artifact_read(args),
        other => Err(AutonomyParseError::UnknownTaskVerb(other.to_string())),
    }
}

fn parse_task_artifact_read(args: &str) -> Result<TaskCommand, AutonomyParseError> {
    let (task_id, selector_raw) = split_head(args);
    if task_id.is_empty() {
        return Err(AutonomyParseError::MissingId {
            command: "/task artifact",
        });
    }
    let selector_raw = selector_raw.trim();
    if selector_raw.is_empty() {
        return Err(AutonomyParseError::MissingId {
            command: "/task artifact <task_id>",
        });
    }
    let selector = if let Some(path) = selector_raw.strip_prefix("path:") {
        let path = path.trim();
        if path.is_empty() {
            return Err(AutonomyParseError::MissingId {
                command: "/task artifact <task_id>",
            });
        }
        TaskArtifactSelector::Path(path.to_string())
    } else if let Some(id) = selector_raw.strip_prefix("id:") {
        let id = id.trim();
        if id.is_empty() {
            return Err(AutonomyParseError::MissingId {
                command: "/task artifact <task_id>",
            });
        }
        TaskArtifactSelector::Id(id.to_string())
    } else {
        TaskArtifactSelector::Id(selector_raw.to_string())
    };
    Ok(TaskCommand::ArtifactRead {
        task_id: task_id.to_string(),
        selector,
    })
}

fn parse_thread(tail: &str) -> Result<ThreadCommand, AutonomyParseError> {
    let (verb, args) = split_head(tail);
    match verb {
        "" | "graph" | "graph-get" if args.is_empty() => Ok(ThreadCommand::Graph),
        "" | "graph" | "graph-get" => Err(AutonomyParseError::UnknownThreadVerb(
            format!("{verb} {args}").trim().to_string(),
        )),
        other => Err(AutonomyParseError::UnknownThreadVerb(other.to_string())),
    }
}

fn parse_turn(tail: &str) -> Result<TurnCommand, AutonomyParseError> {
    let (verb, args) = split_head(tail);
    match verb {
        "" | "state" | "state-get" => {
            let id = args.trim();
            Ok(TurnCommand::State((!id.is_empty()).then(|| id.to_string())))
        }
        other => Err(AutonomyParseError::UnknownTurnVerb(other.to_string())),
    }
}

fn parse_goal(tail: &str) -> Result<GoalCommand, AutonomyParseError> {
    let trimmed = tail.trim();
    if trimmed.is_empty() {
        return Ok(GoalCommand::Show);
    }
    match trimmed {
        "pause" => return Ok(GoalCommand::Pause),
        "resume" => return Ok(GoalCommand::Resume),
        "stop" | "complete" | "done" => return Ok(GoalCommand::Stop),
        "clear" => return Ok(GoalCommand::Clear),
        _ => {}
    }
    // Pull an optional `--budget <value>` flag out of the remainder; the
    // rest is the objective. Setting a goal with a budget both creates a
    // fresh goal AND re-activates a budget_limited one (the dispatch
    // sends status=active), so `/goal <obj> --budget 5m` is the way to
    // un-freeze a goal that hit its cap.
    let (objective, token_budget) = extract_budget_flag(trimmed)?;
    let objective = objective.trim().to_string();
    if objective.is_empty() {
        Err(AutonomyParseError::EmptyGoalObjective)
    } else {
        Ok(GoalCommand::Set {
            objective,
            token_budget,
        })
    }
}

/// Remove an optional `--budget <value>` / `--budget=<value>` flag from
/// the goal remainder, returning the objective text (flag stripped) and
/// the parsed token budget. A `--budget` with a missing or unparseable
/// value is a hard error so it surfaces a hint instead of silently
/// folding the flag into the objective text. When the flag is absent the
/// budget is `None` and the backend default applies.
fn extract_budget_flag(input: &str) -> Result<(String, Option<u64>), AutonomyParseError> {
    // Fast path: no `--budget` token → the objective is the input verbatim
    // (outer trim only), preserving any internal whitespace the user typed.
    // Detect the flag as a whole whitespace-delimited token so an
    // objective that merely contains the substring `--budget` mid-word is
    // never mangled. Only when a real flag is present do we tokenize (and
    // accept single-space normalization of the objective as the cost of
    // the opt-in flag syntax).
    let has_flag = input
        .split_whitespace()
        .any(|word| word == "--budget" || word.starts_with("--budget="));
    if !has_flag {
        return Ok((input.trim().to_string(), None));
    }
    let mut objective_words: Vec<&str> = Vec::new();
    let mut token_budget: Option<u64> = None;
    let mut words = input.split_whitespace();
    while let Some(word) = words.next() {
        if let Some(value) = word.strip_prefix("--budget=") {
            token_budget = Some(parse_token_budget(value)?);
        } else if word == "--budget" {
            let value = words
                .next()
                .ok_or_else(|| AutonomyParseError::InvalidBudget(word.to_string()))?;
            token_budget = Some(parse_token_budget(value)?);
        } else {
            objective_words.push(word);
        }
    }
    Ok((objective_words.join(" "), token_budget))
}

/// Parse a human-friendly token budget: a positive number with an
/// optional `k` (×1_000) or `m` (×1_000_000) suffix, case-insensitive.
/// Examples: `50000`, `500k`, `2m`, `1.5m`. The backend enforces the hard
/// ceiling; this only rejects values that are not a positive number.
fn parse_token_budget(raw: &str) -> Result<u64, AutonomyParseError> {
    let raw = raw.trim();
    let invalid = || AutonomyParseError::InvalidBudget(raw.to_string());
    let lower = raw.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(rest) = lower.strip_suffix('k') {
        (rest, 1_000_f64)
    } else if let Some(rest) = lower.strip_suffix('m') {
        (rest, 1_000_000_f64)
    } else {
        (lower.as_str(), 1_f64)
    };
    let value: f64 = digits.trim().parse().map_err(|_| invalid())?;
    if !value.is_finite() || value <= 0.0 {
        return Err(invalid());
    }
    let tokens = (value * multiplier).round();
    // `u64::MAX as f64` rounds UP to 2^64, so a plain `> u64::MAX as f64`
    // lets exactly-2^64 through and the cast then saturates to u64::MAX.
    // Reject at 2^64 (exclusive) instead. (Absurd magnitudes like this are
    // rejected server-side anyway, but returning InvalidBudget gives the
    // user a hint rather than a silently clamped value.)
    const TWO_POW_64: f64 = 18_446_744_073_709_551_616.0;
    if !tokens.is_finite() || tokens < 1.0 || tokens >= TWO_POW_64 {
        return Err(invalid());
    }
    Ok(tokens as u64)
}

fn parse_loop(tail: &str) -> Result<LoopCommand, AutonomyParseError> {
    let trimmed = tail.trim();
    if trimmed.is_empty() {
        // Per UPCR-2026-021 §"Parsing rules" line 298: without an
        // interval and without a prompt, create a maintenance loop.
        // The backend resolves the prompt from `.octos/loop.md`, then
        // `~/.octos/loop.md`, then a built-in fallback.
        return Ok(LoopCommand::Create {
            prompt: String::new(),
            cadence: LoopCadence::Maintenance,
        });
    }
    let (verb, args) = split_head(trimmed);
    // Verb-style subcommands take precedence so users can refer to
    // loop ids that happen to look like prompts.
    match verb {
        "list" => return Ok(LoopCommand::List),
        "delete" => return Ok(LoopCommand::Delete(require_id("/loop delete", args)?)),
        "pause" => return Ok(LoopCommand::Pause(require_id("/loop pause", args)?)),
        "resume" => return Ok(LoopCommand::Resume(require_id("/loop resume", args)?)),
        "fire-now" | "fire_now" | "firenow" => {
            return Ok(LoopCommand::FireNow(require_id("/loop fire-now", args)?));
        }
        _ => {}
    }
    parse_loop_create(trimmed)
}

fn parse_loop_create(body: &str) -> Result<LoopCommand, AutonomyParseError> {
    // Forms (matched in priority order):
    //   "maintenance <prompt>"            -> Maintenance cadence
    //   "every <interval> <prompt>"       -> fixed cadence
    //   "<interval> <prompt>"             -> fixed cadence (shorthand)
    //   "<prompt> every <interval>"       -> fixed cadence (suffix)
    //   "<prompt>"                        -> self-paced
    let (head, rest) = split_head(body);
    if head == "maintenance" {
        let prompt = rest.trim().to_string();
        if prompt.is_empty() {
            return Err(AutonomyParseError::EmptyLoopPrompt);
        }
        return Ok(LoopCommand::Create {
            prompt,
            cadence: LoopCadence::Maintenance,
        });
    }
    if head == "every" {
        let (interval_raw, prompt_raw) = split_head(rest);
        if interval_raw.is_empty() {
            return Err(AutonomyParseError::InvalidInterval(String::new()));
        }
        let interval = parse_interval(interval_raw)?;
        let prompt = prompt_raw.trim().to_string();
        if prompt.is_empty() {
            return Err(AutonomyParseError::EmptyLoopPrompt);
        }
        return Ok(LoopCommand::Create {
            prompt,
            cadence: LoopCadence::Every(interval),
        });
    }
    if let Some(interval) = try_parse_interval_token(head) {
        let prompt = rest.trim().to_string();
        if prompt.is_empty() {
            return Err(AutonomyParseError::EmptyLoopPrompt);
        }
        // Per UPCR-2026-021 §"Parsing rules" line 295-296: if both
        // leading and trailing intervals are present, reject with
        // `loop_invalid_interval`. Otherwise the trailing `every ...`
        // would silently be treated as part of the prompt body.
        if let Some((_, trailing_interval_raw)) = prompt.rsplit_once(" every ") {
            let trailing = trailing_interval_raw.trim();
            if parse_interval(trailing).is_ok() {
                return Err(AutonomyParseError::InvalidInterval(format!(
                    "{head} ... every {trailing}"
                )));
            }
        }
        return Ok(LoopCommand::Create {
            prompt,
            cadence: LoopCadence::Every(interval),
        });
    }
    // Suffix form: "... every <interval>".
    if let Some((prompt_raw, interval_raw)) = body.rsplit_once(" every ") {
        let prompt = prompt_raw.trim().to_string();
        if prompt.is_empty() {
            return Err(AutonomyParseError::EmptyLoopPrompt);
        }
        let interval = parse_interval(interval_raw.trim())?;
        return Ok(LoopCommand::Create {
            prompt,
            cadence: LoopCadence::Every(interval),
        });
    }
    Ok(LoopCommand::Create {
        prompt: body.to_string(),
        cadence: LoopCadence::SelfPaced,
    })
}

fn try_parse_interval_token(token: &str) -> Option<Duration> {
    parse_interval(token).ok()
}

/// Parse an interval like `5m`, `30s`, `2h`, `1500ms`. Bare integers are
/// rejected — the user must spell out a unit so the backend cannot
/// silently inherit a TUI default.
fn parse_interval(raw: &str) -> Result<Duration, AutonomyParseError> {
    let s = raw.trim();
    if s.is_empty() {
        return Err(AutonomyParseError::InvalidInterval(s.to_string()));
    }
    let (digits_end, unit_start) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(idx) if idx > 0 => (idx, idx),
        _ => return Err(AutonomyParseError::InvalidInterval(s.to_string())),
    };
    let digits = &s[..digits_end];
    let unit = &s[unit_start..];
    let value: u64 = digits
        .parse()
        .map_err(|_| AutonomyParseError::InvalidInterval(s.to_string()))?;
    // Unit scaling uses checked multiplication: a huge value (e.g. 19
    // digits of days) would overflow u64 — a debug panic or a silent
    // release wrap-around cadence. Overflow is just another invalid
    // interval.
    let dur = match unit {
        "ms" => Some(Duration::from_millis(value)),
        "s" | "sec" | "secs" => Some(Duration::from_secs(value)),
        "m" | "min" | "mins" => value.checked_mul(60).map(Duration::from_secs),
        "h" | "hr" | "hrs" => value.checked_mul(60 * 60).map(Duration::from_secs),
        "d" | "day" | "days" => value.checked_mul(60 * 60 * 24).map(Duration::from_secs),
        _ => return Err(AutonomyParseError::InvalidInterval(s.to_string())),
    };
    dur.ok_or_else(|| AutonomyParseError::InvalidInterval(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_list_is_default() {
        assert_eq!(
            parse_autonomy_slash("/agents").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::List))
        );
        assert_eq!(
            parse_autonomy_slash("/agents list").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::List))
        );
        // /agent (singular) aliases to /agents per existing alias
        // convention in the registry.
        assert_eq!(
            parse_autonomy_slash("/agent").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::List))
        );
    }

    #[test]
    fn agents_status_takes_optional_id() {
        assert_eq!(
            parse_autonomy_slash("/agents status").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::Status(None)))
        );
        assert_eq!(
            parse_autonomy_slash("/agents status reviewer-1").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::Status(Some(
                "reviewer-1".to_string()
            ))))
        );
    }

    #[test]
    fn agents_output_requires_id() {
        assert_eq!(
            parse_autonomy_slash("/agents output").unwrap_err(),
            AutonomyParseError::MissingId {
                command: "/agents output"
            }
        );
        assert_eq!(
            parse_autonomy_slash("/agents output ag-7").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::Output(
                "ag-7".into()
            )))
        );
    }

    #[test]
    fn agents_artifact_read_accepts_id_or_path_selector() {
        assert_eq!(
            parse_autonomy_slash("/agents artifact ag-7 artifact-1").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::ArtifactRead {
                agent_id: "ag-7".into(),
                selector: AgentArtifactSelector::Id("artifact-1".into()),
            }))
        );
        assert_eq!(
            parse_autonomy_slash("/agents read-artifact ag-7 path:reports/out.md").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::ArtifactRead {
                agent_id: "ag-7".into(),
                selector: AgentArtifactSelector::Path("reports/out.md".into()),
            }))
        );
    }

    #[test]
    fn agents_unknown_verb_errors() {
        assert_eq!(
            parse_autonomy_slash("/agents wat").unwrap_err(),
            AutonomyParseError::UnknownAgentsVerb("wat".into())
        );
    }

    #[test]
    fn task_artifact_read_accepts_id_or_path_selector() {
        assert_eq!(
            parse_autonomy_slash("/task artifact 00000000-0000-0000-0000-000000000007 summary")
                .unwrap(),
            Some(AutonomyCommand::Task(TaskCommand::ArtifactRead {
                task_id: "00000000-0000-0000-0000-000000000007".into(),
                selector: TaskArtifactSelector::Id("summary".into()),
            }))
        );
        assert_eq!(
            parse_autonomy_slash(
                "/task read-artifact 00000000-0000-0000-0000-000000000007 path:reports/out.md",
            )
            .unwrap(),
            Some(AutonomyCommand::Task(TaskCommand::ArtifactRead {
                task_id: "00000000-0000-0000-0000-000000000007".into(),
                selector: TaskArtifactSelector::Path("reports/out.md".into()),
            }))
        );
    }

    #[test]
    fn task_unknown_verb_errors() {
        assert_eq!(
            parse_autonomy_slash("/task wat").unwrap_err(),
            AutonomyParseError::UnknownTaskVerb("wat".into())
        );
    }

    #[test]
    fn thread_graph_parses_bare_or_graph_verb() {
        assert_eq!(
            parse_autonomy_slash("/threads").unwrap(),
            Some(AutonomyCommand::Thread(ThreadCommand::Graph))
        );
        assert_eq!(
            parse_autonomy_slash("/thread graph").unwrap(),
            Some(AutonomyCommand::Thread(ThreadCommand::Graph))
        );
    }

    #[test]
    fn thread_unknown_verb_errors() {
        assert_eq!(
            parse_autonomy_slash("/thread wat").unwrap_err(),
            AutonomyParseError::UnknownThreadVerb("wat".into())
        );
    }

    #[test]
    fn turn_state_parses_optional_turn_id() {
        assert_eq!(
            parse_autonomy_slash("/turn state").unwrap(),
            Some(AutonomyCommand::Turn(TurnCommand::State(None)))
        );
        assert_eq!(
            parse_autonomy_slash("/turn state 00000000-0000-0000-0000-000000000011").unwrap(),
            Some(AutonomyCommand::Turn(TurnCommand::State(Some(
                "00000000-0000-0000-0000-000000000011".into()
            ))))
        );
    }

    #[test]
    fn turn_unknown_verb_errors() {
        assert_eq!(
            parse_autonomy_slash("/turn wat").unwrap_err(),
            AutonomyParseError::UnknownTurnVerb("wat".into())
        );
    }

    #[test]
    fn goal_bare_shows_state() {
        assert_eq!(
            parse_autonomy_slash("/goal").unwrap(),
            Some(AutonomyCommand::Goal(GoalCommand::Show))
        );
    }

    #[test]
    fn goal_verbs_take_priority_over_text() {
        for verb in ["pause", "resume", "clear", "stop", "complete", "done"] {
            let parsed = parse_autonomy_slash(&format!("/goal {verb}")).unwrap();
            let expected = match verb {
                "pause" => GoalCommand::Pause,
                "resume" => GoalCommand::Resume,
                "clear" => GoalCommand::Clear,
                "stop" | "complete" | "done" => GoalCommand::Stop,
                _ => unreachable!(),
            };
            assert_eq!(parsed, Some(AutonomyCommand::Goal(expected)));
        }
    }

    #[test]
    fn goal_set_carries_full_objective() {
        let parsed = parse_autonomy_slash("/goal ship the supervised-task UX by Friday").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Goal(GoalCommand::Set {
                objective: "ship the supervised-task UX by Friday".into(),
                token_budget: None,
            }))
        );
    }

    #[test]
    fn goal_set_parses_budget_flag_with_suffixes() {
        // Trailing flag, `k`/`m` suffixes, `--budget=` form, and mid-text
        // placement all resolve to raw token counts with the flag removed
        // from the objective.
        for (input, expected_budget) in [
            ("/goal improve kim score --budget 2m", 2_000_000_u64),
            ("/goal improve kim score --budget 500k", 500_000),
            ("/goal improve kim score --budget=1.5m", 1_500_000),
            ("/goal improve kim score --budget 50000", 50_000),
        ] {
            assert_eq!(
                parse_autonomy_slash(input).unwrap(),
                Some(AutonomyCommand::Goal(GoalCommand::Set {
                    objective: "improve kim score".into(),
                    token_budget: Some(expected_budget),
                })),
                "input: {input}"
            );
        }
    }

    #[test]
    fn goal_set_rejects_malformed_budget() {
        for input in [
            "/goal do things --budget",     // missing value
            "/goal do things --budget abc", // non-numeric
            "/goal do things --budget 0",   // non-positive
            "/goal do things --budget -5",  // negative
            // 2^64 exactly: `u64::MAX as f64` rounds to this, so it must
            // be rejected rather than silently clamped to u64::MAX.
            "/goal do things --budget 18446744073709551616",
            "/goal do things --budget 20000000000000000000", // > 2^64
        ] {
            assert!(
                matches!(
                    parse_autonomy_slash(input),
                    Err(AutonomyParseError::InvalidBudget(_))
                ),
                "expected InvalidBudget for: {input}"
            );
        }
    }

    #[test]
    fn goal_set_without_budget_preserves_objective_whitespace() {
        // A no-flag objective must reach dispatch verbatim (outer trim
        // only) — the budget parsing must not normalize internal spacing.
        let parsed = parse_autonomy_slash("/goal keep   the    spacing").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Goal(GoalCommand::Set {
                objective: "keep   the    spacing".into(),
                token_budget: None,
            }))
        );
    }

    #[test]
    fn goal_budget_flag_only_still_requires_objective() {
        assert!(matches!(
            parse_autonomy_slash("/goal --budget 2m"),
            Err(AutonomyParseError::EmptyGoalObjective)
        ));
    }

    #[test]
    fn bare_loop_creates_maintenance() {
        // Per UPCR-2026-021 §"Parsing rules" line 298: bare `/loop`
        // (no prompt, no interval, no verb) creates a maintenance loop.
        // The backend resolves the prompt from `.octos/loop.md`, then
        // `~/.octos/loop.md`, then a built-in fallback.
        assert_eq!(
            parse_autonomy_slash("/loop").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: String::new(),
                cadence: LoopCadence::Maintenance,
            }))
        );
        // Whitespace-only tail behaves identically.
        assert_eq!(
            parse_autonomy_slash("/loop   ").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: String::new(),
                cadence: LoopCadence::Maintenance,
            }))
        );
    }

    #[test]
    fn bare_loop_with_only_subcommand_keeps_list() {
        // `/loop list` still routes to the list verb — only fully bare
        // `/loop` is rerouted to maintenance create.
        assert_eq!(
            parse_autonomy_slash("/loop list").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::List))
        );
    }

    #[test]
    fn dual_interval_rejects() {
        // Per UPCR-2026-021 §"Parsing rules" line 295-296: if both a
        // leading and a trailing interval are present, reject with
        // `loop_invalid_interval`. Today the leading interval would
        // silently win and `every 10m` would be treated as prompt text.
        assert!(matches!(
            parse_autonomy_slash("/loop 5m run tests every 10m").unwrap_err(),
            AutonomyParseError::InvalidInterval(_)
        ));
    }

    #[test]
    fn loop_create_self_paced_when_no_interval() {
        let parsed = parse_autonomy_slash("/loop check the deploy").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "check the deploy".into(),
                cadence: LoopCadence::SelfPaced,
            }))
        );
    }

    #[test]
    fn loop_create_with_explicit_every_interval() {
        let parsed = parse_autonomy_slash("/loop every 5m run flaky tests").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "run flaky tests".into(),
                cadence: LoopCadence::Every(Duration::from_secs(300)),
            }))
        );
    }

    #[test]
    fn loop_create_with_leading_interval_shorthand() {
        let parsed = parse_autonomy_slash("/loop 30s ping the queue").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "ping the queue".into(),
                cadence: LoopCadence::Every(Duration::from_secs(30)),
            }))
        );
    }

    #[test]
    fn loop_create_with_suffix_every() {
        let parsed = parse_autonomy_slash("/loop ping the queue every 2h").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "ping the queue".into(),
                cadence: LoopCadence::Every(Duration::from_secs(7200)),
            }))
        );
    }

    #[test]
    fn loop_create_maintenance_cadence() {
        let parsed = parse_autonomy_slash("/loop maintenance prune old artifacts").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "prune old artifacts".into(),
                cadence: LoopCadence::Maintenance,
            }))
        );
    }

    #[test]
    fn loop_verb_routes_take_priority() {
        assert_eq!(
            parse_autonomy_slash("/loop list").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::List))
        );
        assert_eq!(
            parse_autonomy_slash("/loop delete loop-7").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::Delete("loop-7".into())))
        );
        assert_eq!(
            parse_autonomy_slash("/loop pause loop-7").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::Pause("loop-7".into())))
        );
        assert_eq!(
            parse_autonomy_slash("/loop resume loop-7").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::Resume("loop-7".into())))
        );
        assert_eq!(
            parse_autonomy_slash("/loop fire-now loop-7").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::FireNow("loop-7".into())))
        );
        assert_eq!(
            parse_autonomy_slash("/loop fire_now loop-7").unwrap(),
            Some(AutonomyCommand::Loop(LoopCommand::FireNow("loop-7".into())))
        );
    }

    #[test]
    fn loop_delete_requires_id() {
        assert_eq!(
            parse_autonomy_slash("/loop delete").unwrap_err(),
            AutonomyParseError::MissingId {
                command: "/loop delete"
            }
        );
    }

    #[test]
    fn loop_create_rejects_bare_integer_interval() {
        // "5 run tests" must NOT silently get a 5-second cadence. The
        // TUI cannot invent a default unit; the entire form is treated
        // as a self-paced prompt.
        let parsed = parse_autonomy_slash("/loop 5 run tests").unwrap();
        assert_eq!(
            parsed,
            Some(AutonomyCommand::Loop(LoopCommand::Create {
                prompt: "5 run tests".into(),
                cadence: LoopCadence::SelfPaced,
            }))
        );
    }

    #[test]
    fn loop_create_rejects_invalid_interval_unit() {
        assert!(matches!(
            parse_autonomy_slash("/loop every 5xz hello").unwrap_err(),
            AutonomyParseError::InvalidInterval(_)
        ));
    }

    #[test]
    fn should_reject_interval_when_unit_multiplication_overflows() {
        // 10^19 parses as u64, but *60*60*24 overflows u64. This must
        // surface as the invalid-interval error, not a debug panic /
        // release wrap-around cadence.
        assert!(matches!(
            parse_autonomy_slash("/loop every 9999999999999999999d hello").unwrap_err(),
            AutonomyParseError::InvalidInterval(_)
        ));
        assert!(matches!(
            parse_autonomy_slash("/loop every 9999999999999999999h hello").unwrap_err(),
            AutonomyParseError::InvalidInterval(_)
        ));
        assert!(matches!(
            parse_autonomy_slash("/loop every 9999999999999999999m hello").unwrap_err(),
            AutonomyParseError::InvalidInterval(_)
        ));
        // A huge but non-overflowing value still parses.
        assert!(parse_autonomy_slash("/loop every 9999999999s hello").is_ok());
    }

    #[test]
    fn unknown_top_level_command_errors() {
        assert_eq!(
            parse_autonomy_slash("/foo bar").unwrap_err(),
            AutonomyParseError::UnknownCommand("foo".into())
        );
    }

    #[test]
    fn empty_input_returns_none() {
        assert_eq!(parse_autonomy_slash("   ").unwrap(), None);
    }

    #[test]
    fn leading_slash_is_optional() {
        assert_eq!(
            parse_autonomy_slash("agents list").unwrap(),
            Some(AutonomyCommand::Agents(AgentsCommand::List))
        );
    }
}
