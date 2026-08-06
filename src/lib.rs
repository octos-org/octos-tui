// i18n: load `locales/*.yml` (relative to the crate root) and generate the
// `t!` macro + `rust_i18n::set_locale` / `available_locales!`. English is the
// source/fallback locale; `--lang zh` (or OCTOS_LANG=zh) switches the UI.
// New strings: add a key under `locales/en.yml` and its `zh` translation.
#[macro_use]
extern crate rust_i18n;
rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod autonomy;
pub mod backend_ensure;
pub mod cli;
pub mod client_event;
pub mod clipboard;
pub mod cmd;
pub mod event_loop;
pub mod file_picker;
pub mod highlight;
pub mod history;
pub mod insert_history;
pub mod keymap;
pub mod menu;
pub mod model;
pub mod profiles;
pub mod sanitize;
pub mod store;
pub mod terminal_probe;
pub mod theme;
pub mod transport;
pub mod tui_terminal;
pub mod viewport;

#[cfg(test)]
mod i18n_tests {
    /// The i18n scaffold loads both locale files and resolves keys. Uses the
    /// per-call `locale =` override (NOT `set_locale`) so it can't mutate the
    /// process-global locale and flake other tests that assume English.
    #[test]
    fn resolves_keys_in_en_and_zh() {
        assert_eq!(
            &*t!("composer.placeholder", locale = "en"),
            "Ask Octos to change code..."
        );
        assert_eq!(
            &*t!("composer.placeholder", locale = "zh"),
            "让 Octos 帮你改代码……"
        );
    }

    /// Both shipped locales are registered (guards against a misnamed locale
    /// file silently dropping a language).
    #[test]
    fn ships_en_and_zh_locales() {
        let locales = rust_i18n::available_locales!();
        assert!(locales.contains(&"en"), "missing en: {locales:?}");
        assert!(locales.contains(&"zh"), "missing zh: {locales:?}");
    }

    /// #363/#364: the `@` file-picker menu + `!` shell-escape mode keys resolve
    /// in BOTH locales (rust-i18n echoes the key back on a miss).
    #[test]
    fn composer_escape_and_file_picker_keys_resolve_in_en_and_zh() {
        let keys = [
            "menu.file_picker.title",
            "menu.file_picker.search",
            "menu.file_picker.footer",
            "menu.file_picker.item.empty.label",
            "menu.file_picker.item.empty.desc",
            "menu.file_picker.item.truncated.label",
            "status.bang_mode_hint",
            "status.bang_mode_cancelled",
            "status.bang_cwd",
            "status.inserted_at_cursor",
            "status.file_picker_closed",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// #324: the session-switcher strings resolve in BOTH locales.
    #[test]
    fn sessions_popup_keys_resolve_in_en_and_zh() {
        let keys = [
            "command.sessions.desc",
            "menu.sessions.title",
            "menu.sessions.subtitle",
            "menu.sessions.footer",
            "menu.sessions.item.current",
            "menu.sessions.item.switch_desc",
            "menu.sessions.item.empty",
            "menu.sessions.item.empty_desc",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(&*value, key, "missing {locale} translation for {key}");
            }
        }
    }

    /// #1768: the /undo snapshot picker strings resolve in BOTH locales.
    #[test]
    fn undo_picker_keys_resolve_in_en_and_zh() {
        let keys = [
            "command.undo.desc",
            "status.undo_no_session",
            "menu.undo.title",
            "menu.undo.subtitle",
            "menu.undo.footer",
            "menu.undo.age.just_now",
            "menu.undo.item.refresh.label",
            "menu.undo.item.refresh.desc",
            "menu.undo.item.stale.label",
            "menu.undo.item.stale.desc",
            "menu.undo.item.unavailable.label",
            "menu.undo.item.unavailable.desc",
            "menu.undo.item.disabled.label",
            "menu.undo.item.disabled.desc",
            "menu.undo.item.empty.label",
            "menu.undo.item.empty.desc",
            "menu.undo.item.snap.desc",
            "menu.undo_confirm.title",
            "menu.undo_confirm.subtitle",
            "menu.undo_confirm.yes_desc",
            "menu.undo_confirm.item.empty.label",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// #395 + octos#1801 v2: the `/peer` + `/gather` command strings
    /// (registry descriptions + dispatch / kickoff / fan-in status lines)
    /// resolve in BOTH locales.
    #[test]
    fn peer_command_keys_resolve_in_en_and_zh() {
        let keys = [
            "command.peer.desc",
            "status.peer_usage",
            "status.peer_preparing",
            "status.peer_prepare_in_flight",
            "status.session_blocked_hint",
            "menu.sessions.item.blocked_reason",
            "status.peer_opening",
            "status.peer_started",
            "status.peer_switched",
            "status.peer_fleet_opening",
            "status.peer_staged_by_agent",
            "status.peer_staged_known",
            "command.gather.desc",
            "status.gather_requesting",
            "status.gather_no_peers",
            "status.gather_submitted",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// octos#1807: the `turn/steer` status string resolves in BOTH locales
    /// (rust-i18n echoes the key back on a miss).
    #[test]
    fn turn_steer_keys_resolve_in_en_and_zh() {
        let keys = ["status.steered_into_turn"];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// PR384 fixes: the research-lane wizard strings (lane-aware save row,
    /// lane-key picker, saved status/target labels) resolve in BOTH locales.
    #[test]
    fn research_lane_wizard_keys_resolve_in_en_and_zh() {
        let keys = [
            "onboarding.provider.research_lane",
            "onboarding.provider.save_research_lane",
            "menu.onboard.item.save_research_lane.desc",
            "menu.research_lane_key.title",
            "menu.research_lane_key.subtitle",
            "menu.research_lane_key.occupied",
            "menu.research_lane_key.vacant",
            "menu.research_lane_key.item.cheap.desc",
            "menu.research_lane_key.item.strong.desc",
            "status.research_lane_saved",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// #362: the side-by-side diff view toggle strings (footer hints + status
    /// feedback) resolve in BOTH locales.
    #[test]
    fn diff_view_toggle_keys_resolve_in_en_and_zh() {
        let keys = [
            "app.diff.toggle_side_by_side_hint",
            "app.diff.toggle_unified_hint",
            "app.diff.side_by_side_too_narrow",
            "status.diff_view_side_by_side",
            "status.diff_view_unified",
            "status.diff_view_too_narrow",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }

    /// Recovery advice must name flags that ACTUALLY EXIST.
    ///
    /// This string shipped for a long time as
    /// "Start the TUI with `octos tui --target <stdio:...|ws://...>`", which is
    /// wrong twice over: `octos` has no `tui` subcommand, and there is no
    /// `--target` flag. It is the advice shown when the TUI cannot find a
    /// transport — i.e. to a user who is already stuck — so following it landed
    /// them on "unrecognized subcommand".
    ///
    /// Assert against the real clap definition rather than a hardcoded list, so
    /// this keeps working as flags are added or renamed.
    #[test]
    fn doctor_recovery_advice_names_only_real_cli_flags() {
        let command = crate::cli::cli_command();
        let real: std::collections::HashSet<String> = command
            .get_arguments()
            .filter_map(|arg| arg.get_long())
            .map(|long| format!("--{long}"))
            .collect();
        assert!(
            real.contains("--endpoint") && real.contains("--stdio-command"),
            "sanity: the flags this advice names must be discoverable from clap"
        );

        for locale in ["en", "zh"] {
            let advice = t!("status.doctor_no_transport_recovery", locale = locale);

            // Only flags addressed to THIS binary count. A quoted value like
            // `--stdio-command "octos serve --stdio"` carries the SERVER's
            // flags, which clap here knows nothing about, so drop quoted spans
            // before scanning.
            let mut outside = String::new();
            let mut in_quotes = false;
            for ch in advice.chars() {
                if ch == '"' {
                    in_quotes = !in_quotes;
                    continue;
                }
                if !in_quotes {
                    outside.push(ch);
                }
            }

            // Every `--flag` token the advice mentions must be a real long flag.
            for token in outside.split_whitespace() {
                let flag: String = token
                    .trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                    .to_string();
                if !flag.starts_with("--") || flag.len() <= 2 {
                    continue;
                }
                assert!(
                    real.contains(&flag),
                    "{locale} recovery advice names `{flag}`, which is not a \
                     real CLI flag. Advice: {advice}"
                );
            }

            // `octos` is the SERVER binary and has no `tui` subcommand; the TUI
            // is its own binary. Pin the specific wrong invocation that shipped.
            assert!(
                !advice.contains("octos tui"),
                "{locale} recovery advice tells the user to run `octos tui`, \
                 which is not a subcommand. Advice: {advice}"
            );
        }
    }

    /// UX2 A.3/B.2: the new onboarding teaching-panel + workspace-step keys
    /// resolve in BOTH locales (rust-i18n echoes the key on a miss, so a typo or
    /// a missing `zh` translation would leave the dotted key in the output).
    #[test]
    fn onboarding_ux2_keys_resolve_in_en_and_zh() {
        let keys = [
            "onboarding.language.title",
            "onboarding.language.description",
            "onboarding.wizard.explain_title",
            "onboarding.wizard.explain.language",
            "onboarding.wizard.explain.profile",
            "onboarding.wizard.explain.provider",
            "onboarding.wizard.explain.connect",
            "onboarding.wizard.explain.save",
            "onboarding.wizard.explain.workspace",
            "onboarding.wizard.explain.activate",
            "onboarding.wizard.workspace_title",
            "onboarding.wizard.workspace_open_label",
            "onboarding.wizard.workspace_open_description",
            "onboarding.wizard.workspace_locked_reason",
            "onboarding.preview.provider.configured_title",
            "onboarding.preview.workspace.staged_title",
            "menu.lang.item.en.label",
            "menu.lang.item.zh.label",
            // Phase 2 nameable-profiles + Phase 3 startup picker keys.
            "onboarding.field.profile_name",
            "onboarding.field.profile_name_desc",
            "onboarding.value_suggested",
            "menu.profile_picker.title",
            "menu.profile_picker.subtitle",
            "menu.profile_picker.item.attach.desc",
            "menu.profile_picker.item.create.label",
            "menu.profile_picker.item.create.desc",
        ];
        for key in keys {
            for locale in ["en", "zh"] {
                let value = t!(key, locale = locale);
                assert_ne!(
                    &*value, key,
                    "missing {locale} translation for `{key}` (got the raw key back)"
                );
                assert!(
                    !value.trim().is_empty(),
                    "empty {locale} translation for `{key}`"
                );
            }
        }
    }
}
