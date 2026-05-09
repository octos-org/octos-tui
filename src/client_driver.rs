use std::collections::HashMap;

use eyre::{Result, WrapErr, eyre};
use octos_core::SessionKey;
use octos_core::app_ui::{AppUiCommand, AppUiError, AppUiEvent, AppUiStatus};
use octos_core::ui_protocol::{
    ApprovalScopesListResult, JSON_RPC_VERSION, RpcRequest, SessionOpenResult,
    TaskOutputDeltaEvent, TaskOutputReadResult, UiCursor, UiNotification, methods, rpc_error_codes,
};
use serde_json::Value;

use crate::{client_event::ClientEvent, model::DiffPreviewGetResult};

pub(crate) const MAX_TEXT_FRAME_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingRequest {
    pub(crate) method: String,
}

#[derive(Debug, Default)]
pub(crate) struct ClientDriver {
    pub(crate) pending_requests: HashMap<String, PendingRequest>,
    pub(crate) session_cursors: HashMap<SessionKey, UiCursor>,
    next_request_id: u64,
}

impl ClientDriver {
    pub(crate) fn next_request_id(&mut self) -> String {
        self.next_request_id += 1;
        format!("tui-{}", self.next_request_id)
    }

    pub(crate) fn build_tracked_request(
        &mut self,
        command: AppUiCommand,
    ) -> Result<RpcRequest<serde_json::Value>> {
        let command = self.command_with_resume_cursor(command);
        let method = command.method().to_string();
        let request_id = self.next_request_id();
        let request = rpc_request_from_command(request_id.clone(), command)?;

        self.pending_requests
            .insert(request_id, PendingRequest { method });

        Ok(request)
    }

    fn command_with_resume_cursor(&self, command: AppUiCommand) -> AppUiCommand {
        let AppUiCommand::OpenSession(mut params) = command else {
            return command;
        };

        if params.after.is_none() {
            params.after = self.session_cursors.get(&params.session_id).cloned();
        }

        AppUiCommand::OpenSession(params)
    }

    pub(crate) fn decode_rpc_text(&mut self, text: &str) -> Result<Option<ClientEvent>> {
        let event = rpc_text_to_app_event_with_pending(text, &mut self.pending_requests)?;
        if let Some(ClientEvent::App(event)) = &event {
            self.record_event_state(event);
        }
        Ok(event)
    }

    fn record_event_state(&mut self, event: &AppUiEvent) {
        match event {
            AppUiEvent::Protocol(UiNotification::SessionOpened(opened)) => {
                if let Some(cursor) = &opened.cursor {
                    self.session_cursors
                        .insert(opened.session_id.clone(), cursor.clone());
                }
            }
            AppUiEvent::Protocol(UiNotification::TurnCompleted(completed)) => {
                if let Some(cursor) = &completed.cursor {
                    self.session_cursors
                        .insert(completed.session_id.clone(), cursor.clone());
                }
            }
            AppUiEvent::Snapshot(_)
            | AppUiEvent::Protocol(_)
            | AppUiEvent::Progress(_)
            | AppUiEvent::Status(_)
            | AppUiEvent::Error(_) => {}
        }
    }

    pub(crate) fn cancel_pending_requests(&mut self, reason: &str) -> Vec<AppUiEvent> {
        let mut pending_requests = self.pending_requests.drain().collect::<Vec<_>>();
        pending_requests.sort_by(|(left_id, _), (right_id, _)| left_id.cmp(right_id));

        pending_requests
            .into_iter()
            .map(|(id, request)| {
                app_error(
                    "request_cancelled",
                    format!("{} request {id} cancelled: {reason}", request.method),
                )
            })
            .collect()
    }
}

#[allow(unreachable_patterns)]
pub(crate) fn rpc_request_from_command(
    id: String,
    command: AppUiCommand,
) -> Result<RpcRequest<serde_json::Value>> {
    let method = command.method().to_string();
    let params = match command {
        AppUiCommand::OpenSession(params) => serde_json::to_value(params),
        AppUiCommand::SubmitPrompt(params) => serde_json::to_value(params),
        AppUiCommand::InterruptTurn(params) => serde_json::to_value(params),
        AppUiCommand::RespondApproval(params) => serde_json::to_value(params),
        AppUiCommand::ListApprovalScopes(params) => serde_json::to_value(params),
        AppUiCommand::GetDiffPreview(params) => serde_json::to_value(params),
        AppUiCommand::ReadTaskOutput(params) => serde_json::to_value(params),
        _ => {
            return Err(eyre!(
                "unsupported AppUI command for first-server transport: {method}"
            ));
        }
    }
    .wrap_err_with(|| format!("failed to encode params for {method}"))?;

    Ok(RpcRequest {
        jsonrpc: JSON_RPC_VERSION.into(),
        id,
        method,
        params,
    })
}

#[cfg(test)]
pub(crate) fn rpc_text_to_app_event(text: &str) -> Result<Option<ClientEvent>> {
    let mut pending_requests = HashMap::new();
    rpc_text_to_app_event_with_pending(text, &mut pending_requests)
}

fn rpc_text_to_app_event_with_pending(
    text: &str,
    pending_requests: &mut HashMap<String, PendingRequest>,
) -> Result<Option<ClientEvent>> {
    if text.len() > MAX_TEXT_FRAME_BYTES {
        return Ok(Some(
            app_error(
                "frame_too_large",
                format!(
                    "UI protocol frame is {} bytes; max is {MAX_TEXT_FRAME_BYTES}",
                    text.len()
                ),
            )
            .into(),
        ));
    }

    let value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return Ok(Some(
                app_error(
                    "malformed_json",
                    format!("UI protocol frame is not JSON: {err}"),
                )
                .into(),
            ));
        }
    };

    rpc_value_to_app_event(value, pending_requests)
}

fn rpc_value_to_app_event(
    value: Value,
    pending_requests: &mut HashMap<String, PendingRequest>,
) -> Result<Option<ClientEvent>> {
    let Some(frame) = value.as_object() else {
        return Ok(Some(
            app_error("malformed_frame", "UI protocol frame must be a JSON object").into(),
        ));
    };

    if let Some(error) = validate_jsonrpc_v2(frame) {
        return Ok(Some(error.into()));
    }

    let has_method = frame.contains_key("method");
    let has_id = frame.contains_key("id");
    let has_result = frame.contains_key("result");
    let has_error = frame.contains_key("error");

    if has_method {
        if has_id || has_result || has_error {
            return Ok(Some(
                app_error(
                    "malformed_frame",
                    "UI protocol notification must not include id, result, or error",
                )
                .into(),
            ));
        }

        let Some(method) = frame.get("method").and_then(Value::as_str) else {
            return Ok(Some(
                app_error(
                    "malformed_frame",
                    "UI protocol notification method must be a string",
                )
                .into(),
            ));
        };

        let params = frame.get("params").cloned().unwrap_or(Value::Null);
        return Ok(Some(notification_to_app_event(method, params).into()));
    }

    if has_result || has_error || has_id {
        if !has_id {
            return Ok(Some(
                app_error("malformed_frame", "UI protocol response is missing id").into(),
            ));
        }
        if has_result == has_error {
            return Ok(Some(
                app_error(
                    "malformed_frame",
                    "UI protocol response must include exactly one of result or error",
                )
                .into(),
            ));
        }

        return if has_error {
            Ok(Some(
                error_response_to_app_event(frame, pending_requests).into(),
            ))
        } else {
            success_response_to_app_event(frame, pending_requests)
        };
    }

    Ok(Some(
        app_error(
            "malformed_frame",
            "unsupported UI protocol frame: expected JSON-RPC notification or response",
        )
        .into(),
    ))
}

fn validate_jsonrpc_v2(frame: &serde_json::Map<String, Value>) -> Option<AppUiEvent> {
    match frame.get("jsonrpc") {
        Some(Value::String(version)) if version == JSON_RPC_VERSION => None,
        Some(Value::String(version)) => Some(app_error(
            "invalid_jsonrpc",
            format!("unsupported JSON-RPC version: {version}"),
        )),
        Some(_) => Some(app_error(
            "invalid_jsonrpc",
            "UI protocol frame jsonrpc field must be \"2.0\"",
        )),
        None => Some(app_error(
            "invalid_jsonrpc",
            "UI protocol frame is missing jsonrpc \"2.0\"",
        )),
    }
}

fn success_response_to_app_event(
    frame: &serde_json::Map<String, Value>,
    pending_requests: &mut HashMap<String, PendingRequest>,
) -> Result<Option<ClientEvent>> {
    let id = match response_id(frame) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return Ok(Some(
                app_error(
                    "malformed_frame",
                    "UI protocol success response id must not be null",
                )
                .into(),
            ));
        }
        Err(event) => return Ok(Some((*event).into())),
    };

    let Some(result) = frame.get("result").cloned() else {
        return Ok(Some(
            app_error(
                "malformed_frame",
                "UI protocol response is missing result field",
            )
            .into(),
        ));
    };

    let pending_request = pending_requests.remove(&id);
    let Some(pending_request) = pending_request else {
        return Ok(None);
    };

    match pending_request.method.as_str() {
        methods::SESSION_OPEN => match serde_json::from_value::<SessionOpenResult>(result) {
            Ok(result) => Ok(Some(
                AppUiEvent::Protocol(UiNotification::SessionOpened(result.opened)).into(),
            )),
            Err(err) => Ok(Some(
                app_error(
                    "invalid_result",
                    format!(
                        "failed to decode UI protocol result for {}: {err}",
                        methods::SESSION_OPEN
                    ),
                )
                .into(),
            )),
        },
        methods::DIFF_PREVIEW_GET => match serde_json::from_value::<DiffPreviewGetResult>(result) {
            Ok(result) => Ok(Some(ClientEvent::DiffPreview(result))),
            Err(err) => Ok(Some(
                app_error(
                    "invalid_result",
                    format!(
                        "failed to decode UI protocol result for {}: {err}",
                        methods::DIFF_PREVIEW_GET
                    ),
                )
                .into(),
            )),
        },
        methods::TASK_OUTPUT_READ => match decode_task_output_read_result(result) {
            Ok(result) => {
                let text = task_output_display_text(&result);
                Ok(Some(
                    AppUiEvent::Protocol(UiNotification::TaskOutputDelta(TaskOutputDeltaEvent {
                        session_id: result.session_id,
                        task_id: result.task_id,
                        cursor: result.next_cursor,
                        text,
                    }))
                    .into(),
                ))
            }
            Err(err) => Ok(Some(
                app_error(
                    "invalid_result",
                    format!(
                        "failed to decode UI protocol result for {}: {err}",
                        methods::TASK_OUTPUT_READ
                    ),
                )
                .into(),
            )),
        },
        methods::TURN_INTERRUPT => Ok(Some(
            AppUiEvent::Status(AppUiStatus {
                message: interrupt_ack_status(&result),
            })
            .into(),
        )),
        methods::APPROVAL_RESPOND => Ok(Some(
            AppUiEvent::Status(AppUiStatus {
                message: "Approval response acknowledged".into(),
            })
            .into(),
        )),
        methods::APPROVAL_SCOPES_LIST => {
            match serde_json::from_value::<ApprovalScopesListResult>(result) {
                Ok(result) => Ok(Some(
                    AppUiEvent::Status(AppUiStatus {
                        message: approval_scopes_status(&result),
                    })
                    .into(),
                )),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            methods::APPROVAL_SCOPES_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        _ => Ok(None),
    }
}

fn decode_task_output_read_result(mut result: Value) -> serde_json::Result<TaskOutputReadResult> {
    if let Some(object) = result.as_object_mut() {
        object
            .entry("is_snapshot_projection")
            .or_insert(Value::Bool(false));
    }
    serde_json::from_value(result)
}

fn approval_scopes_status(result: &ApprovalScopesListResult) -> String {
    let count = result.scopes.len();
    match count {
        0 => "No persisted approval scopes for this session".into(),
        1 => "1 persisted approval scope for this session".into(),
        _ => format!("{count} persisted approval scopes for this session"),
    }
}

fn task_output_display_text(result: &TaskOutputReadResult) -> String {
    let mut text = result.text.clone();
    if !result.output_files.is_empty() && !text.contains("output_files:") {
        append_metadata_section(
            &mut text,
            "output_files",
            result.output_files.iter().map(String::as_str),
        );
    }
    if !result.limitations.is_empty() && !text.contains("limitations:") {
        append_metadata_section(
            &mut text,
            "limitations",
            result
                .limitations
                .iter()
                .map(|limitation| limitation.message.as_str()),
        );
    }
    text
}

fn append_metadata_section<'a>(
    text: &mut String,
    title: &str,
    items: impl IntoIterator<Item = &'a str>,
) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(title);
    text.push_str(":\n");
    for item in items {
        text.push_str("- ");
        text.push_str(item);
        text.push('\n');
    }
}

fn interrupt_ack_status(result: &Value) -> String {
    match result.get("interrupted").and_then(Value::as_bool) {
        Some(false) => "Interrupt acknowledged; turn was already idle".into(),
        Some(true) => "Interrupt acknowledged; active turn cancelled".into(),
        None => "Interrupt acknowledged".into(),
    }
}

fn error_response_to_app_event(
    frame: &serde_json::Map<String, Value>,
    pending_requests: &mut HashMap<String, PendingRequest>,
) -> AppUiEvent {
    let request_id = match response_id(frame) {
        Ok(request_id) => request_id,
        Err(event) => return *event,
    };
    let Some(error) = frame.get("error") else {
        return app_error(
            "malformed_frame",
            "UI protocol response is missing error field",
        );
    };
    if !error.is_object() {
        return app_error(
            "malformed_frame",
            "UI protocol error response error field must be an object",
        );
    }

    let pending_request = request_id
        .as_ref()
        .and_then(|id| pending_requests.remove(id));
    let code = rpc_error_code(error);
    let message = rpc_error_message(error);
    let message = match (pending_request, request_id) {
        (Some(request), Some(id)) => {
            format!("{} request {id} failed: {message}", request.method)
        }
        (None, Some(id)) => format!("request {id} failed: {message}"),
        (_, None) => message,
    };

    app_error(code, message)
}

fn response_id(
    frame: &serde_json::Map<String, Value>,
) -> std::result::Result<Option<String>, Box<AppUiEvent>> {
    let Some(id) = frame.get("id") else {
        return Err(Box::new(app_error(
            "malformed_frame",
            "UI protocol response is missing id",
        )));
    };

    match id {
        Value::Null => Ok(None),
        Value::String(value) => Ok(Some(value.clone())),
        Value::Number(value) if value.is_i64() || value.is_u64() => Ok(Some(value.to_string())),
        _ => Err(Box::new(app_error(
            "malformed_frame",
            "UI protocol response id must be a string, integer, or null",
        ))),
    }
}

fn notification_to_app_event(method: &str, params: Value) -> AppUiEvent {
    match UiNotification::from_method_and_params(method, params) {
        Ok(UiNotification::ProgressUpdated(progress)) => AppUiEvent::Progress(progress),
        Ok(notification) => AppUiEvent::Protocol(notification),
        Err(err) if err.code == rpc_error_codes::METHOD_NOT_FOUND => app_error(
            "unknown_notification",
            format!("unknown UI protocol notification: {method}"),
        ),
        Err(err) => app_error(
            "invalid_params",
            format!(
                "failed to decode UI protocol params for {method}: {}",
                err.message
            ),
        ),
    }
}

fn rpc_error_code(error: &Value) -> String {
    error
        .get("code")
        .map(|code| {
            code.as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| code.to_string())
        })
        .unwrap_or_else(|| "json_rpc_error".into())
}

fn rpc_error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("JSON-RPC error")
        .to_string()
}

fn app_error(code: impl Into<String>, message: impl Into<String>) -> AppUiEvent {
    AppUiEvent::Error(AppUiError {
        code: code.into(),
        message: message.into(),
    })
}
