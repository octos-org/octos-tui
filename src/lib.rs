// i18n: load `locales/*.yml` (relative to the crate root) and generate the
// `t!` macro + `rust_i18n::set_locale` / `available_locales!`. English is the
// source/fallback locale; `--lang zh` (or OCTOS_LANG=zh) switches the UI.
// New strings: add a key under `locales/en.yml` and its `zh` translation.
#[macro_use]
extern crate rust_i18n;
rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
pub mod autonomy;
pub mod cli;
pub mod client_event;
pub mod event_loop;
pub mod keymap;
pub mod menu;
pub mod model;
pub mod store;
pub mod theme;
pub mod transport;

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
}
