use octos_core::ui_protocol::UiProtocolCapabilities;

use crate::menu::types::CommandSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandAvailability {
    pub task: TaskRequirement,
    pub approval: ApprovalRequirement,
    pub readonly: ReadonlyPolicy,
    pub runtime: RuntimeRequirement,
    pub connection: ConnectionRequirement,
    pub session: SessionRequirement,
    pub required_methods: &'static [&'static str],
    pub required_methods_when_capabilities: &'static [&'static str],
    pub required_methods_any: &'static [&'static str],
    pub required_features: &'static [&'static str],
    pub required_feature_flags: &'static [&'static str],
    pub unavailable: UnavailablePolicy,
}

impl CommandAvailability {
    pub const fn always() -> Self {
        Self {
            task: TaskRequirement::Any,
            approval: ApprovalRequirement::Any,
            readonly: ReadonlyPolicy::Allowed,
            runtime: RuntimeRequirement::Any,
            connection: ConnectionRequirement::Any,
            session: SessionRequirement::Any,
            required_methods: &[],
            required_methods_when_capabilities: &[],
            required_methods_any: &[],
            required_features: &[],
            required_feature_flags: &[],
            unavailable: UnavailablePolicy::Hide,
        }
    }

    pub fn local_mutating() -> Self {
        Self {
            readonly: ReadonlyPolicy::BlockMutating,
            ..Self::always()
        }
    }

    pub fn app_ui_read(required_methods: &'static [&'static str]) -> Self {
        Self {
            runtime: RuntimeRequirement::Protocol,
            connection: ConnectionRequirement::Connected,
            session: SessionRequirement::Open,
            required_methods,
            required_methods_when_capabilities: &[],
            ..Self::always()
        }
    }

    pub fn app_ui_mutating(required_methods: &'static [&'static str]) -> Self {
        Self {
            readonly: ReadonlyPolicy::BlockMutating,
            runtime: RuntimeRequirement::Protocol,
            connection: ConnectionRequirement::Connected,
            session: SessionRequirement::Open,
            required_methods,
            required_methods_when_capabilities: &[],
            ..Self::always()
        }
    }

    pub fn with_task(mut self, task: TaskRequirement) -> Self {
        self.task = task;
        self
    }

    pub fn with_session(mut self, session: SessionRequirement) -> Self {
        self.session = session;
        self
    }

    pub fn with_unavailable_policy(mut self, unavailable: UnavailablePolicy) -> Self {
        self.unavailable = unavailable;
        self
    }

    pub fn with_required_methods_any(
        mut self,
        required_methods_any: &'static [&'static str],
    ) -> Self {
        self.required_methods_any = required_methods_any;
        self
    }

    pub fn with_required_methods_when_capabilities(
        mut self,
        required_methods_when_capabilities: &'static [&'static str],
    ) -> Self {
        self.required_methods_when_capabilities = required_methods_when_capabilities;
        self
    }

    /// Require ALL of the listed capability features (e.g.
    /// `coding.autonomy.v1`). When any feature is missing the command
    /// is hidden by default (or disabled when the policy is `Disable`).
    pub fn with_required_features(mut self, required_features: &'static [&'static str]) -> Self {
        self.required_features = required_features;
        self
    }

    pub fn evaluate(&self, ctx: &AvailabilityContext<'_>) -> AvailabilityStatus {
        if self.session == SessionRequirement::Open && !ctx.session_open {
            return self.unavailable.status("requires an open session");
        }

        match self.task {
            TaskRequirement::Any => {}
            TaskRequirement::Idle if ctx.task == TaskActivity::Idle => {}
            TaskRequirement::Running if ctx.task == TaskActivity::Running => {}
            TaskRequirement::Idle => {
                return self
                    .unavailable
                    .status("requires the current turn to be idle");
            }
            TaskRequirement::Running => {
                return self
                    .unavailable
                    .status("requires an active turn or background task");
            }
        }

        match self.approval {
            ApprovalRequirement::Any => {}
            ApprovalRequirement::NoApprovalModal if !ctx.approval_modal_visible => {}
            ApprovalRequirement::ApprovalModalVisible if ctx.approval_modal_visible => {}
            ApprovalRequirement::NoApprovalModal => {
                return self.unavailable.status("approval modal has keyboard focus");
            }
            ApprovalRequirement::ApprovalModalVisible => {
                return self.unavailable.status("requires a visible approval modal");
            }
        }

        match self.runtime {
            RuntimeRequirement::Any => {}
            RuntimeRequirement::Mock if ctx.runtime == RuntimeMode::Mock => {}
            RuntimeRequirement::Protocol if ctx.runtime == RuntimeMode::Protocol => {}
            RuntimeRequirement::Mock => return self.unavailable.status("requires mock mode"),
            RuntimeRequirement::Protocol => {
                return self.unavailable.status("requires protocol mode");
            }
        }

        match self.connection {
            ConnectionRequirement::Any => {}
            ConnectionRequirement::Connected if ctx.connection == ConnectionState::Connected => {}
            ConnectionRequirement::Disconnected
                if ctx.connection == ConnectionState::Disconnected => {}
            ConnectionRequirement::Connected => {
                return self.unavailable.status("requires a connected AppUI server");
            }
            ConnectionRequirement::Disconnected => {
                return self.unavailable.status("requires disconnected mode");
            }
        }

        if let Some(flag) = self
            .required_feature_flags
            .iter()
            .find(|flag| !ctx.has_feature_flag(flag))
        {
            return self
                .unavailable
                .status(format!("feature flag `{flag}` is disabled"));
        }

        if !self.required_methods.is_empty() && ctx.capabilities.is_none() {
            return self
                .unavailable
                .status("AppUI capabilities are not available");
        }

        if let Some(method) = self.required_methods.iter().find(|method| {
            ctx.unsupported_method_reason(method).is_some() || !ctx.supports_method(method)
        }) {
            if let Some(reason) = ctx.unsupported_method_reason(method) {
                return self
                    .unavailable
                    .status(format!("AppUI method `{method}` is unsupported: {reason}"));
            }
            return self
                .unavailable
                .status(format!("AppUI method `{method}` is not available"));
        }

        if let Some(capabilities) = ctx.capabilities
            && let Some(method) = self
                .required_methods_when_capabilities
                .iter()
                .find(|method| {
                    ctx.unsupported_method_reason(method).is_some() || !ctx.supports_method(method)
                })
        {
            let unavailable = if capabilities.methods().is_empty() {
                UnavailablePolicy::Hide
            } else {
                UnavailablePolicy::Disable
            };
            if let Some(reason) = ctx.unsupported_method_reason(method) {
                return unavailable
                    .status(format!("AppUI method `{method}` is unsupported: {reason}"));
            }
            return unavailable.status(format!("AppUI method `{method}` is not available"));
        }

        if !self.required_methods_any.is_empty() && ctx.capabilities.is_none() {
            return self
                .unavailable
                .status("AppUI capabilities are not available");
        }

        if !self.required_methods_any.is_empty()
            && !self
                .required_methods_any
                .iter()
                .any(|method| ctx.supports_method(method))
        {
            if let Some((method, reason)) = self.required_methods_any.iter().find_map(|method| {
                ctx.unsupported_method_reason(method)
                    .map(|reason| (*method, reason))
            }) {
                return self
                    .unavailable
                    .status(format!("AppUI method `{method}` is unsupported: {reason}"));
            }

            let methods = self
                .required_methods_any
                .iter()
                .map(|method| format!("`{method}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return self
                .unavailable
                .status(format!("requires one of {methods}"));
        }

        if !self.required_features.is_empty() && ctx.capabilities.is_none() {
            return self
                .unavailable
                .status("AppUI capabilities are not available");
        }

        if let Some(feature) = self
            .required_features
            .iter()
            .find(|feature| !ctx.supports_feature(feature))
        {
            return self
                .unavailable
                .status(format!("AppUI feature `{feature}` is not available"));
        }

        if self.readonly == ReadonlyPolicy::BlockMutating && ctx.readonly {
            return self.unavailable.status("blocked in read-only mode");
        }

        AvailabilityStatus::available()
    }
}

impl Default for CommandAvailability {
    fn default() -> Self {
        Self::always()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskRequirement {
    Any,
    Idle,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRequirement {
    Any,
    NoApprovalModal,
    ApprovalModalVisible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadonlyPolicy {
    Allowed,
    BlockMutating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRequirement {
    Any,
    Mock,
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionRequirement {
    Any,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRequirement {
    Any,
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailablePolicy {
    Hide,
    Disable,
}

impl UnavailablePolicy {
    fn status(self, reason: impl Into<String>) -> AvailabilityStatus {
        match self {
            Self::Hide => AvailabilityStatus::hidden(reason),
            Self::Disable => AvailabilityStatus::disabled(reason),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskActivity {
    Idle,
    Running,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    Mock,
    Protocol,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Copy)]
pub struct AvailabilityContext<'a> {
    pub task: TaskActivity,
    pub approval_modal_visible: bool,
    pub readonly: bool,
    pub runtime: RuntimeMode,
    pub connection: ConnectionState,
    pub capabilities: Option<&'a CapabilitySet>,
    pub feature_flags: &'a [&'a str],
    pub session_open: bool,
}

impl<'a> AvailabilityContext<'a> {
    pub fn local() -> Self {
        Self {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Mock,
            connection: ConnectionState::Disconnected,
            capabilities: None,
            feature_flags: &[],
            session_open: false,
        }
    }

    pub fn protocol(capabilities: &'a CapabilitySet) -> Self {
        Self {
            task: TaskActivity::Idle,
            approval_modal_visible: false,
            readonly: false,
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            capabilities: Some(capabilities),
            feature_flags: &[],
            session_open: true,
        }
    }

    pub fn has_feature_flag(&self, flag: &str) -> bool {
        self.feature_flags
            .iter()
            .any(|candidate| *candidate == flag)
    }

    pub fn supports_method(&self, method: &str) -> bool {
        self.capabilities
            .map(|capabilities| capabilities.supports_method(method))
            .unwrap_or(false)
    }

    pub fn unsupported_method_reason(&self, method: &str) -> Option<&str> {
        self.capabilities
            .and_then(|capabilities| capabilities.unsupported_method_reason(method))
    }

    pub fn supports_feature(&self, feature: &str) -> bool {
        self.capabilities
            .map(|capabilities| capabilities.supports_feature(feature))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapabilitySet {
    methods: Vec<String>,
    features: Vec<String>,
    unsupported_methods: Vec<(String, String)>,
}

impl CapabilitySet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_methods<I, S>(methods: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            methods: methods.into_iter().map(Into::into).collect(),
            features: Vec::new(),
            unsupported_methods: Vec::new(),
        }
    }

    pub fn from_methods_and_features<I, S, J, T>(methods: I, features: J) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
        J: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Self {
            methods: methods.into_iter().map(Into::into).collect(),
            features: features.into_iter().map(Into::into).collect(),
            unsupported_methods: Vec::new(),
        }
    }

    pub fn supports_method(&self, method: &str) -> bool {
        self.methods.iter().any(|candidate| candidate == method)
            && self.unsupported_method_reason(method).is_none()
    }

    pub fn supports_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|candidate| candidate == feature)
    }

    pub fn unsupported_method_reason(&self, method: &str) -> Option<&str> {
        self.unsupported_methods
            .iter()
            .find_map(|(candidate, reason)| (candidate == method).then_some(reason.as_str()))
    }

    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    pub fn unsupported_methods(&self) -> &[(String, String)] {
        &self.unsupported_methods
    }
}

impl From<&UiProtocolCapabilities> for CapabilitySet {
    fn from(value: &UiProtocolCapabilities) -> Self {
        Self {
            methods: value.supported_methods.clone(),
            features: value.supported_features.clone(),
            unsupported_methods: value
                .unsupported
                .iter()
                .map(|report| (report.method.clone(), report.reason.clone()))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityStatus {
    pub disposition: AvailabilityDisposition,
    pub reason: Option<String>,
}

impl AvailabilityStatus {
    pub fn available() -> Self {
        Self {
            disposition: AvailabilityDisposition::Available,
            reason: None,
        }
    }

    pub fn hidden(reason: impl Into<String>) -> Self {
        Self {
            disposition: AvailabilityDisposition::Hidden,
            reason: Some(reason.into()),
        }
    }

    pub fn disabled(reason: impl Into<String>) -> Self {
        Self {
            disposition: AvailabilityDisposition::Disabled,
            reason: Some(reason.into()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.disposition == AvailabilityDisposition::Available
    }

    pub fn is_visible(&self) -> bool {
        self.disposition != AvailabilityDisposition::Hidden
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityDisposition {
    Available,
    Hidden,
    Disabled,
}

pub fn evaluate_command(
    command: &CommandSpec,
    ctx: &AvailabilityContext<'_>,
) -> AvailabilityStatus {
    command.availability.evaluate(ctx)
}

#[cfg(test)]
mod tests {
    use octos_core::ui_protocol::{UnsupportedCapabilityReport, methods};

    use super::*;
    use crate::menu::types::{CommandCategory, CommandEntry, InlineArgMode, MenuId};

    fn command(availability: CommandAvailability) -> CommandSpec {
        CommandSpec {
            name: "test",
            aliases: &[],
            description: "test command",
            category: CommandCategory::Developer,
            availability,
            inline_args: InlineArgMode::None,
            entry: CommandEntry::OpenMenu(MenuId::from("test")),
        }
    }

    #[test]
    fn readonly_blocks_mutating_commands_with_reason() {
        let mut ctx = AvailabilityContext::local();
        ctx.readonly = true;
        let status = evaluate_command(&command(CommandAvailability::local_mutating()), &ctx);

        assert_eq!(status.disposition, AvailabilityDisposition::Hidden);
        assert_eq!(status.reason.as_deref(), Some("blocked in read-only mode"));
    }

    #[test]
    fn missing_appui_method_is_unavailable() {
        let capabilities = CapabilitySet::from_methods([methods::TURN_START]);
        let ctx = AvailabilityContext::protocol(&capabilities);
        let status = evaluate_command(
            &command(CommandAvailability::app_ui_read(&[methods::TURN_INTERRUPT])),
            &ctx,
        );

        assert_eq!(status.disposition, AvailabilityDisposition::Hidden);
        assert!(
            status
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains(methods::TURN_INTERRUPT)
        );
    }

    #[test]
    fn missing_capability_map_has_specific_reason() {
        let ctx = AvailabilityContext {
            runtime: RuntimeMode::Protocol,
            connection: ConnectionState::Connected,
            session_open: true,
            ..AvailabilityContext::local()
        };
        let status = evaluate_command(
            &command(CommandAvailability::app_ui_read(&[methods::TURN_INTERRUPT])),
            &ctx,
        );

        assert_eq!(
            status.reason.as_deref(),
            Some("AppUI capabilities are not available")
        );
    }

    #[test]
    fn unsupported_capability_report_blocks_advertised_method() {
        let capabilities = UiProtocolCapabilities {
            supported_methods: vec![methods::TURN_INTERRUPT.into()],
            unsupported: vec![UnsupportedCapabilityReport::method(
                methods::TURN_INTERRUPT,
                "disabled by policy",
            )],
            ..UiProtocolCapabilities::new(&[], &[])
        };
        let capabilities = CapabilitySet::from(&capabilities);
        let ctx = AvailabilityContext::protocol(&capabilities);
        let status = evaluate_command(
            &command(CommandAvailability::app_ui_read(&[methods::TURN_INTERRUPT])),
            &ctx,
        );

        assert_eq!(status.disposition, AvailabilityDisposition::Hidden);
        assert_eq!(
            status.reason.as_deref(),
            Some("AppUI method `turn/interrupt` is unsupported: disabled by policy")
        );
    }

    #[test]
    fn any_of_methods_allows_command_when_one_capability_exists() {
        let capabilities = CapabilitySet::from_methods([methods::APPROVAL_SCOPES_LIST]);
        let ctx = AvailabilityContext::protocol(&capabilities);
        let status = evaluate_command(
            &command(
                CommandAvailability::app_ui_read(&[]).with_required_methods_any(&[
                    methods::APPROVAL_SCOPES_LIST,
                    methods::PERMISSION_PROFILE_SET,
                ]),
            ),
            &ctx,
        );

        assert!(status.is_available());
    }

    #[test]
    fn local_inspection_is_available_offline() {
        let ctx = AvailabilityContext::local();
        let status = evaluate_command(&command(CommandAvailability::always()), &ctx);

        assert!(status.is_available());
        assert!(status.is_visible());
    }
}
