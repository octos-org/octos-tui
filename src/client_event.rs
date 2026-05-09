use octos_core::{SessionKey, app_ui::AppUiEvent};

use crate::{model::DiffPreviewGetResult, permission_profile::PermissionProfileSelection};

#[derive(Debug, Clone)]
pub enum ClientEvent {
    App(Box<AppUiEvent>),
    DiffPreview(DiffPreviewGetResult),
    PermissionProfile(PermissionProfileClientEvent),
}

impl From<AppUiEvent> for ClientEvent {
    fn from(event: AppUiEvent) -> Self {
        Self::App(Box::new(event))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionProfileClientEvent {
    pub session_id: SessionKey,
    pub current: PermissionProfileSelection,
    pub message: String,
}
