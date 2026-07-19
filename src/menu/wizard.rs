//! First-run setup wizard step model.
//!
//! The onboarding flow is rendered by two menu providers
//! (`onboarding_local_profile_menu` and `onboarding_provider_setup_menu` in
//! `providers.rs`), but to the user it is ONE guided wizard. This module
//! computes a single, content-agnostic projection of the wizard's progress
//! from [`OnboardingWizardState`] so each onboarding screen can show:
//!
//!   * a `Step N of M` header,
//!   * a one-line purpose for the current step ("why am I here"),
//!   * the next concrete action ("what do I do / what's next"),
//!   * a right-side checklist of every step with its completion mark.
//!
//! Keeping this in one place means the two providers stay in lock-step and the
//! progress copy is computed, not duplicated.
//!
//! NOTE (i18n): the user-facing strings here live in `locales/{en,zh}.yml`
//! under the `onboarding.wizard.*` namespace and are resolved via `t!()`. CJK
//! width is handled by the generic menu render surface (`unicode-width`), so no
//! width math lives in this module — never compare against a rendered string.

use std::borrow::Cow;

use crate::menu::types::{MenuPreview, MenuPreviewRow};
use crate::model::{
    OnboardingProviderStatus, OnboardingWizardState, OnboardingWorkspaceValidation,
};

/// A single user-facing wizard step. The internal provider menus may contain
/// more granular rows (e.g. family vs. model vs. route), but the wizard
/// presents them grouped into these coarse, explainable steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Choose the UI language for onboarding and the current session.
    Language,
    /// Create the local profile. On a nameable-profiles server this is a single
    /// "Name this profile" prompt (`requested_id`); on older servers it falls
    /// back to name / username / email.
    Profile,
    /// Choose the model family + model + provider route.
    Provider,
    /// Enter the API key and verify the route with a live test.
    Connect,
    /// Save the verified provider into the profile JSON.
    Save,
    /// Stage + validate the workspace folder the agent will code in.
    Workspace,
    /// Open the coding session and drop into the working surface.
    Activate,
}

impl WizardStep {
    /// Ordered list of every step, used for the checklist + N-of-M math.
    pub const ALL: [WizardStep; 7] = [
        WizardStep::Language,
        WizardStep::Profile,
        WizardStep::Provider,
        WizardStep::Connect,
        WizardStep::Save,
        WizardStep::Workspace,
        WizardStep::Activate,
    ];

    /// 1-based ordinal for "Step N of M".
    pub fn number(self) -> usize {
        Self::ALL
            .iter()
            .position(|step| *step == self)
            .map(|index| index + 1)
            .unwrap_or(1)
    }

    /// Stable i18n key suffix for this step (NOT user-facing text). Used to
    /// build `onboarding.wizard.step.*` / `onboarding.wizard.purpose.*` keys
    /// without ever switching on a translated string.
    fn key(self) -> &'static str {
        match self {
            WizardStep::Language => "language",
            WizardStep::Profile => "profile",
            WizardStep::Provider => "provider",
            WizardStep::Connect => "connect",
            WizardStep::Save => "save",
            WizardStep::Workspace => "workspace",
            WizardStep::Activate => "activate",
        }
    }

    /// Short checklist label (right-side panel).
    pub fn short_title(self) -> Cow<'static, str> {
        t!(format!("onboarding.wizard.step.{}", self.key()))
    }

    /// One-line purpose ("why this step exists").
    pub fn purpose(self) -> Cow<'static, str> {
        t!(format!("onboarding.wizard.purpose.{}", self.key()))
    }

    /// Multi-line explanatory prose for the right-side teaching panel: what
    /// this step is for, what the user should do, and why it matters. The
    /// source string embeds `\n` line breaks (see `locales/{en,zh}.yml`); the
    /// menu render surface handles wrapping and CJK width.
    pub fn explanation(self) -> Cow<'static, str> {
        t!(format!("onboarding.wizard.explain.{}", self.key()))
    }
}

/// Computed snapshot of the wizard's progress, derived from the wizard state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WizardProgress {
    pub current: WizardStep,
    /// Completion mark per step, in [`WizardStep::ALL`] order.
    pub done: [bool; 7],
}

impl WizardProgress {
    /// Derive progress from the wizard state. `local_create_supported` selects
    /// whether the Profile step is part of the flow (solo/local mode) or
    /// implicitly satisfied by an already-resolved server profile.
    /// `saved_primary_ready` is the server-truth short-circuit (#203): a
    /// hydrated primary provider with a key while the draft is untouched —
    /// exactly the state the provider rows render as "(saved)" — satisfies
    /// the Provider/Connect/Save steps without any draft input.
    pub fn from_state(
        state: &OnboardingWizardState,
        current_profile: Option<&str>,
        local_create_supported: bool,
        saved_primary_ready: bool,
    ) -> Self {
        let language_done = true;
        let profile_done = state.effective_profile_id(current_profile).is_some()
            || (!local_create_supported && current_profile.is_some());
        let provider_done = state.selection_ready() || saved_primary_ready;
        let connect_done = state.provider_tested
            || saved_primary_ready
            || matches!(
                state.provider_status(),
                OnboardingProviderStatus::SavedPrimary
            );
        let save_done = connect_done
            && (saved_primary_ready
                || matches!(
                    state.provider_status(),
                    OnboardingProviderStatus::SavedPrimary | OnboardingProviderStatus::SavedFallback
                ));
        let workspace_done = matches!(
            state.workspace_validation,
            OnboardingWorkspaceValidation::Valid { .. }
        );
        // Activate is only "done" once the session is open, which tears the
        // wizard down — so within the wizard it is never marked complete.
        let activate_done = false;

        let done = [
            language_done,
            profile_done,
            provider_done,
            connect_done,
            save_done,
            workspace_done,
            activate_done,
        ];

        // The current step is the first incomplete one (Activate is the
        // terminal step once everything before it is done).
        let current = WizardStep::ALL
            .iter()
            .zip(done.iter())
            .find(|(_, complete)| !**complete)
            .map(|(step, _)| *step)
            .unwrap_or(WizardStep::Activate);

        Self { current, done }
    }

    /// `Step N of M — <Short title>` header for the menu subtitle.
    pub fn header(&self) -> String {
        t!(
            "onboarding.wizard.header",
            number = self.current.number(),
            total = WizardStep::ALL.len(),
            title = self.current.short_title(),
        )
        .into_owned()
    }

    /// Full subtitle: header + one-line purpose of the current step.
    pub fn subtitle(&self) -> String {
        t!(
            "onboarding.wizard.subtitle",
            header = self.header(),
            purpose = self.current.purpose(),
        )
        .into_owned()
    }

    /// Footer hint naming the next concrete action.
    pub fn footer_hint(&self, next_action: &str) -> String {
        t!("onboarding.wizard.footer", next = next_action).into_owned()
    }

    /// UX2 A.3: the right-side TEACHING panel. Replaces the sparse
    /// checklist-only pane ("little information… waste of space") with genuinely
    /// explanatory prose that updates per step:
    ///
    ///   * a compact progress line (`Step N of M`) + the per-step checklist so
    ///     the user always sees where they are and what is left,
    ///   * a blank separator,
    ///   * the current step's title, then multi-line prose explaining what the
    ///     step is for, what to do, and why it matters.
    ///
    /// Rendered as `MenuPreview::Text` so the body is free-flowing prose the
    /// menu surface wraps (CJK width handled there), not `[label]: [value]`
    /// rows. The `\n`-joined body keeps all width math in the render surface.
    pub fn explanation_preview(&self) -> MenuPreview {
        let mut body = String::new();
        body.push_str(&t!(
            "onboarding.wizard.explain_progress",
            number = self.current.number(),
            total = WizardStep::ALL.len(),
        ));
        body.push('\n');
        for (step, &complete) in WizardStep::ALL.iter().zip(self.done.iter()) {
            let marker = if *step == self.current {
                "▸"
            } else if complete {
                "✓"
            } else {
                "·"
            };
            body.push('\n');
            body.push_str(marker);
            body.push(' ');
            body.push_str(step.number().to_string().as_str());
            body.push_str(". ");
            body.push_str(&step.short_title());
        }
        body.push_str("\n\n");
        body.push_str(&t!(
            "onboarding.wizard.explain_now",
            title = self.current.short_title(),
        ));
        body.push('\n');
        body.push_str(&self.current.explanation());
        MenuPreview::Text {
            title: Some(t!("onboarding.wizard.explain_title").into_owned()),
            body,
        }
    }

    /// Right-side checklist preview: one row per step, current marked `>`,
    /// completed marked `[x]`, pending `[ ]`.
    pub fn checklist_preview(&self) -> MenuPreview {
        let rows = WizardStep::ALL
            .iter()
            .zip(self.done.iter())
            .map(|(step, &complete)| {
                let marker = if *step == self.current {
                    ">"
                } else if complete {
                    "[x]"
                } else {
                    "[ ]"
                };
                MenuPreviewRow {
                    label: format!("{marker} {}", step.number()),
                    value: step.short_title().into_owned(),
                }
            })
            .collect();
        MenuPreview::KeyValues {
            title: Some(t!("onboarding.wizard.progress_title").into_owned()),
            rows,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OnboardingWorkspaceValidation;

    fn valid_workspace() -> OnboardingWorkspaceValidation {
        OnboardingWorkspaceValidation::Valid {
            canonical: "/tmp/ws".into(),
            writable: true,
            has_workspace_toml: false,
        }
    }

    /// A wizard state with a resolved profile (step 1 complete).
    fn state_with_profile() -> OnboardingWizardState {
        OnboardingWizardState {
            profile_id: Some("alice".into()),
            ..OnboardingWizardState::default()
        }
    }

    /// A wizard state with profile + a ready provider selection (steps 1-2).
    fn state_with_selection() -> OnboardingWizardState {
        let mut state = state_with_profile();
        state.provider.family_id = "gpt".into();
        state.provider.model_id = "gpt-x".into();
        state.provider.route.route_id = "openai".into();
        state
    }

    #[test]
    fn fresh_state_defaults_language_and_starts_on_profile_step() {
        let state = OnboardingWizardState::default();
        let progress = WizardProgress::from_state(&state, None, true, false);
        assert_eq!(progress.current, WizardStep::Profile);
        assert_eq!(progress.current.number(), 2);
        // Assert via the same i18n key (NOT a hardcoded English literal) so the
        // test tracks the source string across locales/wording changes. The
        // language step is already satisfied by the default English locale, so
        // the first required input is Profile at 2-of-7.
        assert_eq!(
            progress.header(),
            t!(
                "onboarding.wizard.header",
                number = 2,
                total = 7,
                title = WizardStep::Profile.short_title(),
            )
        );
        assert!(progress.done[0], "language defaults to complete");
        assert!(progress.done[1..].iter().all(|done| !done));
    }

    #[test]
    fn resolved_profile_advances_to_provider_step() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true, false);
        assert_eq!(progress.current, WizardStep::Provider);
        assert!(progress.done[1]);
    }

    #[test]
    fn ready_selection_advances_to_connect_step() {
        let progress = WizardProgress::from_state(&state_with_selection(), None, true, false);
        assert_eq!(progress.current, WizardStep::Connect);
        assert!(progress.done[2], "provider step complete");
    }

    #[test]
    fn validated_workspace_with_save_lands_on_activate() {
        let mut state = state_with_selection();
        state.provider_tested = true;
        state.provider_saved = true;
        state.workspace_validation = valid_workspace();

        let progress = WizardProgress::from_state(&state, None, true, false);
        assert_eq!(progress.current, WizardStep::Activate);
        assert!(progress.done[..6].iter().all(|done| *done));
        assert!(!progress.done[6], "activate never self-marks complete");
    }

    #[test]
    fn checklist_marks_current_completed_and_pending() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true, false);
        let MenuPreview::KeyValues { rows, .. } = progress.checklist_preview() else {
            panic!("expected key-value checklist");
        };
        assert_eq!(rows.len(), 7);
        assert!(rows[0].label.starts_with("[x]"), "language done");
        assert!(rows[1].label.starts_with("[x]"), "profile done");
        assert!(rows[2].label.starts_with('>'), "provider current");
        assert!(rows[3].label.starts_with("[ ]"), "connect pending");
    }

    /// UX2 A.3: the teaching panel is explanatory prose (not `[ ]/[x]` rows).
    /// It must carry a progress line, the per-step list with a current marker,
    /// and the current step's multi-line explanation body.
    #[test]
    fn explanation_preview_is_prose_with_progress_and_current_step() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true, false);
        let MenuPreview::Text { title, body } = progress.explanation_preview() else {
            panic!("expected free-text teaching panel");
        };
        assert!(title.is_some(), "teaching panel has a title");
        // The current step (Provider) explanation prose is present and the
        // current-step marker appears. Assert via the same i18n source so the
        // test tracks wording/locale changes instead of a hardcoded literal.
        assert!(
            body.contains(WizardStep::Provider.explanation().as_ref()),
            "current step explanation prose is shown: {body}"
        );
        assert!(body.contains('▸'), "current step is marked: {body}");
        assert!(body.contains('✓'), "completed step is marked: {body}");
        // Genuinely explanatory: the prose is more than a one-line label.
        assert!(
            body.lines().count() >= WizardStep::ALL.len() + 2,
            "panel is multi-line teaching prose, not a bare checklist: {body}"
        );
    }

    #[test]
    fn server_profile_without_local_create_skips_profile_step() {
        let state = OnboardingWizardState::default();
        let progress = WizardProgress::from_state(&state, Some("server-prof"), false, false);
        assert!(
            progress.done[1],
            "server-authenticated profile satisfies step 1"
        );
        assert_eq!(progress.current, WizardStep::Provider);
    }

    /// #203: a server-hydrated saved primary (untouched draft) satisfies the
    /// Provider/Connect/Save steps, so progress agrees with the "(saved)" row
    /// labels and lands on Workspace instead of demanding draft input.
    #[test]
    fn saved_primary_ready_completes_provider_steps() {
        let progress = WizardProgress::from_state(&state_with_profile(), None, true, true);
        assert!(progress.done[2], "provider satisfied by server truth");
        assert!(progress.done[3], "connect satisfied by server truth");
        assert!(progress.done[4], "save satisfied by server truth");
        assert_eq!(progress.current, WizardStep::Workspace);
    }
}
