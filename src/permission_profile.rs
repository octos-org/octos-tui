#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionNetworkPolicy {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionProfileMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionProfileSelection {
    pub mode: PermissionProfileMode,
    pub network: PermissionNetworkPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PermissionProfileUpdate {
    pub mode: Option<PermissionProfileMode>,
    pub network: Option<PermissionNetworkPolicy>,
}

impl Default for PermissionProfileSelection {
    fn default() -> Self {
        Self {
            mode: PermissionProfileMode::WorkspaceWrite,
            network: PermissionNetworkPolicy::Deny,
        }
    }
}

impl PermissionProfileSelection {
    pub fn normalized(self) -> Self {
        self
    }

    pub fn summary(self) -> String {
        let mode = match self.mode {
            PermissionProfileMode::ReadOnly => "Read Only",
            PermissionProfileMode::WorkspaceWrite => "Workspace Write",
            PermissionProfileMode::DangerFullAccess => "Full Access",
        };
        let network = match self.network {
            PermissionNetworkPolicy::Allow => "network allowed",
            PermissionNetworkPolicy::Deny => "network blocked",
        };
        format!("{mode}, {network}")
    }
}

impl PermissionProfileUpdate {
    pub fn apply_to(self, previous: PermissionProfileSelection) -> PermissionProfileSelection {
        PermissionProfileSelection {
            mode: self.mode.unwrap_or(previous.mode),
            network: self.network.unwrap_or(previous.network),
        }
    }
}
