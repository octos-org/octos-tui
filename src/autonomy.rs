//! M15-E autonomy command parsing for `/agents`, `/goal`, and `/loop`.
//!
//! Parses user-typed slash commands into typed intents the TUI can
//! dispatch as backend AppUI calls. The parser is the source of truth
//! for syntax shape — actual dispatch (issuing `agent/list`,
//! `session/goal/set`, `loop/create`, …) is wired in a later PR once
//! the backend exposes those AppUI methods.
//!
//! Contract reference: octos-tui#47 (M15-E) and
//! `docs/M15_AGENT_GOAL_LOOP_TUI_CONTRACT.md`. The TUI must never:
//!
//! - Probe these methods on servers that did not advertise
//!   [`super::model::APPUI_FEATURE_CODING_AUTONOMY_V1`].
//! - Schedule timers locally for `/loop`. Loop firing is backend-owned.
//! - Invent default intervals; if the user did not supply one, the
//!   parsed intent records "self-paced" and the backend decides.

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
    /// `/agents interrupt <agent_id>`.
    Interrupt(String),
    /// `/agents close <agent_id>`.
    Close(String),
}

/// Parsed `/goal` subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    /// `/goal <objective>` — set a new active goal.
    Set(String),
    /// Bare `/goal` — show current goal.
    Show,
    /// `/goal pause`.
    Pause,
    /// `/goal resume`.
    Resume,
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
    /// A subcommand that requires an `<agent_id>` or `<loop_id>` was
    /// missing one.
    MissingId { command: &'static str },
    /// `/loop` create with empty prompt.
    EmptyLoopPrompt,
    /// `/goal <objective>` with empty objective (after stripping
    /// whitespace).
    EmptyGoalObjective,
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
            Self::MissingId { command } => {
                write!(f, "{command} requires an id argument")
            }
            Self::EmptyLoopPrompt => f.write_str("/loop requires a prompt"),
            Self::EmptyGoalObjective => f.write_str("/goal requires an objective"),
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
        "interrupt" => Ok(AgentsCommand::Interrupt(require_id(
            "/agents interrupt",
            args,
        )?)),
        "close" => Ok(AgentsCommand::Close(require_id("/agents close", args)?)),
        other => Err(AutonomyParseError::UnknownAgentsVerb(other.to_string())),
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
        "clear" => return Ok(GoalCommand::Clear),
        _ => {}
    }
    // Otherwise treat the entire remainder as the objective.
    let objective = trimmed.to_string();
    if objective.is_empty() {
        Err(AutonomyParseError::EmptyGoalObjective)
    } else {
        Ok(GoalCommand::Set(objective))
    }
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
    let dur = match unit {
        "ms" => Duration::from_millis(value),
        "s" | "sec" | "secs" => Duration::from_secs(value),
        "m" | "min" | "mins" => Duration::from_secs(value * 60),
        "h" | "hr" | "hrs" => Duration::from_secs(value * 60 * 60),
        "d" | "day" | "days" => Duration::from_secs(value * 60 * 60 * 24),
        _ => return Err(AutonomyParseError::InvalidInterval(s.to_string())),
    };
    Ok(dur)
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
    fn agents_unknown_verb_errors() {
        assert_eq!(
            parse_autonomy_slash("/agents wat").unwrap_err(),
            AutonomyParseError::UnknownAgentsVerb("wat".into())
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
        for verb in ["pause", "resume", "clear"] {
            let parsed = parse_autonomy_slash(&format!("/goal {verb}")).unwrap();
            let expected = match verb {
                "pause" => GoalCommand::Pause,
                "resume" => GoalCommand::Resume,
                "clear" => GoalCommand::Clear,
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
            Some(AutonomyCommand::Goal(GoalCommand::Set(
                "ship the supervised-task UX by Friday".into()
            )))
        );
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
