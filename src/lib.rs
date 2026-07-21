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
pub mod highlight;
pub mod history;
pub mod insert_history;
pub mod keymap;
pub mod menu;
pub mod model;
pub mod profiles;
pub mod sanitize;
pub mod store;
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
