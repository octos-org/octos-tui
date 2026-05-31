use std::collections::{HashMap, VecDeque};

use chrono::Utc;
use eyre::{Result, WrapErr, eyre};
use futures::{SinkExt, StreamExt};
use octos_core::app_ui::{
    AppUiError, AppUiEvent, AppUiSession, AppUiSnapshot, AppUiStatus, AppUiTask,
};
use octos_core::ui_protocol::{
    ApprovalCommandDetails, ApprovalDiffDetails, ApprovalFilesystemDetails, ApprovalNetworkDetails,
    ApprovalRequestedEvent, ApprovalSandboxDetails, ApprovalSandboxEscalationDetails,
    ApprovalSandboxEscalationEndpoint, ApprovalScopesListResult, ApprovalTypedDetails,
    MessageDeltaEvent, OutputCursor, PermissionProfileListResult, PermissionProfileSelection,
    PermissionProfileSetResult, PreviewId, SessionOpenParams, SessionOpenResult, SessionOpened,
    TaskArtifactReadResult, TaskOutputDeltaEvent, TaskOutputReadResult, TaskRuntimeState,
    TaskUpdatedEvent, ToolCompletedEvent, ToolProgressEvent, ToolStartedEvent, TurnCompletedEvent,
    TurnId, TurnStartedEvent, UiCursor, UiNotification, UiPaneSnapshot, UiProtocolCapabilities,
    WarningEvent, approval_kinds, methods, rpc_error_codes,
};
use octos_core::ui_protocol::{
    JSON_RPC_VERSION, MAX_TEXT_FRAME_BYTES, RpcRequest, UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1,
    UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1, UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1,
    UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1, UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1,
    UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1, UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1,
    UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1, UI_PROTOCOL_V1,
};
use octos_core::{Message, SessionKey, TaskId};
use serde_json::Value;
use std::process::Stdio;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    runtime::Runtime,
    sync::mpsc,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message as WsMessage, client::IntoClientRequest, handshake::client::Request as WsRequest,
    },
};

use crate::{
    cli::{Cli, Mode},
    client_event::{
        AuthLogoutClientEvent, AuthMeClientEvent, AuthSendCodeClientEvent, AuthStatusClientEvent,
        AuthVerifyClientEvent, AutonomyClientEvent, AutonomyResult, CapabilitiesClientEvent,
        ClientEvent, McpConfigListClientEvent, McpConfigMutationClientEvent, McpStatusClientEvent,
        ModelListClientEvent, ModelSelectClientEvent, PermissionProfileClientEvent,
        ProfileLlmCatalogClientEvent, ProfileLlmListClientEvent, ProfileLlmMutationClientEvent,
        ProfileLocalCreateClientEvent, ProfileSkillsListClientEvent,
        ProfileSkillsMutationClientEvent, ProfileSkillsRegistrySearchClientEvent,
        SessionStatusClientEvent, ToolConfigListClientEvent, ToolConfigMutationClientEvent,
        ToolStatusClientEvent,
    },
    model::{
        AppUiAuthToken, AppUiCommand, AuthLogoutResult, AuthMeResult, AuthSendCodeResult,
        AuthStatusResult, AuthVerifyResult, ConfigCapabilitiesListParams,
        ConfigCapabilitiesListResult, DiffPreview, DiffPreviewFile, DiffPreviewGetResult,
        DiffPreviewHunk, DiffPreviewLine, McpConfigEntry, McpConfigListResult,
        McpConfigMutationResult, McpStatus, McpStatusListResult, McpStatusSummary, ModelListResult,
        ModelSelectResult, ModelStatus, ProfileLlmCatalogResult, ProfileLlmListParams,
        ProfileLlmListResult, ProfileLlmMutationResult, ProfileLocalCreateResult,
        ProfileSkillEntry, ProfileSkillRegistryPackage, ProfileSkillsListResult,
        ProfileSkillsMutationResult, ProfileSkillsRegistrySearchResult, RuntimeHealthStatus,
        RuntimePolicyMcpServer, RuntimePolicyStamp, SessionStatusReadResult, ToolConfigEntry,
        ToolConfigListResult, ToolConfigMutationResult, ToolPolicyDenial, ToolStatus,
        ToolStatusListResult, ToolStatusSummary, auth_me_email, auth_me_profile_id,
    },
};

// AppUI can emit large terminal bursts (`message/persisted`, tool summaries,
// replay/lifecycle frames) at turn completion. Keep the transport reader well
// ahead of rendering so stdio stdout does not back up into the backend writer.
const PROTOCOL_TRANSPORT_QUEUE_CAPACITY: usize = 4096;
const MAX_PENDING_REQUESTS: usize = 256;

#[derive(Debug, Clone, Default)]
pub struct AppUiLaunch {
    pub endpoint: Option<AppUiEndpoint>,
    pub base_url: Option<String>,
    pub session_id: Option<SessionKey>,
    pub profile_id: Option<String>,
    pub cwd: Option<String>,
    pub auth_token: Option<String>,
    pub readonly: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppUiEndpoint {
    WebSocket {
        url: String,
        auth_token: Option<String>,
        profile_id: Option<String>,
    },
    Stdio {
        command: String,
    },
}

impl AppUiEndpoint {
    pub fn websocket(url: impl Into<String>, auth_token: Option<String>) -> Self {
        Self::WebSocket {
            url: url.into(),
            auth_token,
            profile_id: None,
        }
    }

    pub fn websocket_with_profile(
        url: impl Into<String>,
        auth_token: Option<String>,
        profile_id: Option<String>,
    ) -> Self {
        Self::WebSocket {
            url: url.into(),
            auth_token,
            profile_id,
        }
    }

    pub fn stdio(command: impl Into<String>) -> Self {
        Self::Stdio {
            command: command.into(),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::WebSocket { url, .. } => url,
            Self::Stdio { command } => command,
        }
    }
}

pub trait AppUiBackend {
    fn bootstrap(&mut self) -> Result<AppUiSnapshot>;
    fn send(&mut self, command: AppUiCommand) -> Result<()>;
    fn next_event(&mut self) -> Result<Option<ClientEvent>>;
}

pub fn build_backend(cli: &Cli) -> Box<dyn AppUiBackend> {
    let launch = launch_from_cli(cli);
    match cli.mode {
        Mode::Mock => Box::new(MockAppUiBackend::new(launch)),
        Mode::Protocol => Box::new(ProtocolAppUiBackend::new(launch)),
    }
}

fn launch_from_cli(cli: &Cli) -> AppUiLaunch {
    let auth_token = auth_token_from_cli(cli);
    AppUiLaunch {
        endpoint: endpoint_from_cli(cli, auth_token.clone()),
        base_url: cli.base_url.clone(),
        session_id: cli.session.clone().map(SessionKey),
        profile_id: cli.profile_id.clone(),
        cwd: launch_cwd_from_cli(cli),
        auth_token,
        readonly: cli.readonly,
    }
}

fn endpoint_from_cli(cli: &Cli, auth_token: Option<String>) -> Option<AppUiEndpoint> {
    if let Some(command) = &cli.stdio_command {
        return Some(AppUiEndpoint::stdio(command.clone()));
    }

    cli.base_url
        .clone()
        .map(|url| AppUiEndpoint::websocket_with_profile(url, auth_token, cli.profile_id.clone()))
}

fn launch_cwd_from_cli(cli: &Cli) -> Option<String> {
    cli.cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .map(|path| path.to_string_lossy().to_string())
}

fn auth_token_from_cli(cli: &Cli) -> Option<String> {
    cli.auth_token
        .clone()
        .and_then(clean_auth_token)
        .or_else(|| {
            std::env::var("OCTOS_AUTH_TOKEN")
                .ok()
                .and_then(clean_auth_token)
        })
}

fn clean_auth_token(token: String) -> Option<String> {
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_owned())
}

pub struct ProtocolAppUiBackend {
    launch: AppUiLaunch,
    runtime: Option<Runtime>,
    runtime_error: Option<String>,
    driver: Option<ProtocolTransportDriver>,
    connection_state: ProtocolConnectionState,
    disconnected_status_reported: bool,
    queue: VecDeque<ClientEvent>,
    protocol: ProtocolExchange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolConnectionState {
    Disconnected,
    Connected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingRequest {
    method: String,
}

#[derive(Debug, Default)]
struct ProtocolExchange {
    pending_requests: HashMap<String, PendingRequest>,
    session_cursors: HashMap<SessionKey, UiCursor>,
    next_request_id: u64,
}

enum ProtocolTransportDriver {
    WebSocket(WebSocketTransportDriver),
    Stdio(StdioTransportDriver),
}

enum TransportCommand {
    Text(String),
    Pong(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportFrame {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TransportEvent {
    Frame(TransportFrame),
    Disconnected(String),
    Error {
        code: String,
        message: String,
        disconnect: bool,
    },
}

struct WebSocketTransportDriver {
    endpoint: String,
    auth_token: Option<String>,
    profile_id: Option<String>,
    command_tx: Option<mpsc::Sender<TransportCommand>>,
    event_rx: Option<mpsc::Receiver<TransportEvent>>,
    task: Option<tokio::task::JoinHandle<()>>,
    connected: bool,
}

struct StdioTransportDriver {
    command: String,
    command_tx: Option<mpsc::Sender<TransportCommand>>,
    event_rx: Option<mpsc::Receiver<TransportEvent>>,
    task: Option<tokio::task::JoinHandle<()>>,
    connected: bool,
}

impl ProtocolExchange {
    fn next_request_id(&mut self) -> String {
        self.next_request_id += 1;
        format!("tui-{}", self.next_request_id)
    }

    fn build_tracked_request(
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

    fn decode_rpc_text(&mut self, text: &str) -> Result<Option<ClientEvent>> {
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

    fn cancel_pending_requests(&mut self, reason: &str) -> Vec<AppUiEvent> {
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

impl ProtocolTransportDriver {
    fn from_endpoint(endpoint: &AppUiEndpoint) -> Result<Self> {
        match endpoint {
            AppUiEndpoint::WebSocket {
                url,
                auth_token,
                profile_id,
            } if is_websocket_url(url) => Ok(Self::WebSocket(WebSocketTransportDriver::new(
                url.clone(),
                auth_token.clone(),
                profile_id.clone(),
            ))),
            AppUiEndpoint::WebSocket { .. } => Err(eyre!(
                "UI protocol endpoint must be a WebSocket URL starting with ws:// or wss://"
            )),
            AppUiEndpoint::Stdio { command, .. } => {
                Ok(Self::Stdio(StdioTransportDriver::new(command.clone())?))
            }
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::WebSocket(driver) => driver.label(),
            Self::Stdio(driver) => driver.label(),
        }
    }

    fn is_connected(&self) -> bool {
        match self {
            Self::WebSocket(driver) => driver.is_connected(),
            Self::Stdio(driver) => driver.is_connected(),
        }
    }

    fn connect(&mut self, runtime: &Runtime) -> Result<()> {
        match self {
            Self::WebSocket(driver) => driver.connect(runtime),
            Self::Stdio(driver) => driver.connect(runtime),
        }
    }

    fn disconnect(&mut self) {
        match self {
            Self::WebSocket(driver) => driver.disconnect(),
            Self::Stdio(driver) => driver.disconnect(),
        }
    }

    fn send_text(&mut self, text: String) -> Result<()> {
        match self {
            Self::WebSocket(driver) => driver.send_text(text),
            Self::Stdio(driver) => driver.send_text(text),
        }
    }

    fn send_pong(&mut self, payload: Vec<u8>) -> Result<()> {
        match self {
            Self::WebSocket(driver) => driver.send_pong(payload),
            Self::Stdio(driver) => driver.send_pong(payload),
        }
    }

    fn poll_event(&mut self) -> Result<Option<TransportEvent>> {
        match self {
            Self::WebSocket(driver) => driver.poll_event(),
            Self::Stdio(driver) => driver.poll_event(),
        }
    }
}

impl WebSocketTransportDriver {
    fn new(endpoint: String, auth_token: Option<String>, profile_id: Option<String>) -> Self {
        Self {
            endpoint,
            auth_token,
            profile_id,
            command_tx: None,
            event_rx: None,
            task: None,
            connected: false,
        }
    }

    fn label(&self) -> &str {
        &self.endpoint
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn connect(&mut self, runtime: &Runtime) -> Result<()> {
        if self.connected {
            return Ok(());
        }

        self.disconnect();

        let request = websocket_request(
            &self.endpoint,
            self.auth_token.as_deref(),
            self.profile_id.as_deref(),
        )?;
        let (stream, _) = runtime
            .block_on(async { connect_async(request).await })
            .wrap_err_with(|| {
                format!("failed to connect UI protocol endpoint {}", self.endpoint)
            })?;
        let (mut sink, mut stream) = stream.split();
        let (command_tx, mut command_rx) = mpsc::channel(PROTOCOL_TRANSPORT_QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(PROTOCOL_TRANSPORT_QUEUE_CAPACITY);

        let task = runtime.spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        let result = match command {
                            TransportCommand::Text(text) => {
                                sink.send(WsMessage::Text(text.into())).await
                            }
                            TransportCommand::Pong(payload) => {
                                sink.send(WsMessage::Pong(payload.into())).await
                            }
                        };

                        if let Err(err) = result {
                            let _ = event_tx.send(TransportEvent::Error {
                                code: "transport_send".into(),
                                message: format!("failed to send UI protocol WebSocket message: {err}"),
                                disconnect: true,
                            }).await;
                            break;
                        }
                    }
                    message = stream.next() => {
                        let Some(message) = message else {
                            let _ = event_tx.send(TransportEvent::Disconnected(
                                "UI protocol WebSocket closed; reconnect will retry on next send/read.".into(),
                            )).await;
                            break;
                        };

                        match message {
                            Ok(WsMessage::Text(text)) => {
                                if event_tx
                                    .send(TransportEvent::Frame(TransportFrame::Text(text.to_string())))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(WsMessage::Binary(bytes)) => {
                                if event_tx
                                    .send(TransportEvent::Frame(TransportFrame::Binary(bytes.to_vec())))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(WsMessage::Ping(payload)) => {
                                if event_tx
                                    .send(TransportEvent::Frame(TransportFrame::Ping(payload.to_vec())))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(WsMessage::Pong(_)) => {
                                if event_tx
                                    .send(TransportEvent::Frame(TransportFrame::Pong))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            Ok(WsMessage::Close(_)) => {
                                let _ = event_tx.send(TransportEvent::Frame(TransportFrame::Close)).await;
                                break;
                            }
                            Ok(WsMessage::Frame(_)) => {}
                            Err(err) => {
                                let _ = event_tx.send(TransportEvent::Error {
                                    code: "transport_read".into(),
                                    message: format!("failed to read UI protocol WebSocket message: {err}"),
                                    disconnect: true,
                                }).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        self.command_tx = Some(command_tx);
        self.event_rx = Some(event_rx);
        self.task = Some(task);
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        self.command_tx = None;
        self.event_rx = None;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.connected = false;
    }

    fn send_text(&mut self, text: String) -> Result<()> {
        self.command_tx
            .as_ref()
            .filter(|_| self.connected)
            .ok_or_else(|| eyre!("UI protocol WebSocket is not connected"))?
            .try_send(TransportCommand::Text(text))
            .map_err(|err| bounded_send_error("UI protocol WebSocket writer", err))
    }

    fn send_pong(&mut self, payload: Vec<u8>) -> Result<()> {
        self.command_tx
            .as_ref()
            .filter(|_| self.connected)
            .ok_or_else(|| eyre!("UI protocol WebSocket is not connected"))?
            .try_send(TransportCommand::Pong(payload))
            .map_err(|err| bounded_send_error("UI protocol WebSocket writer", err))
    }

    fn poll_event(&mut self) -> Result<Option<TransportEvent>> {
        let Some(event_rx) = self.event_rx.as_mut() else {
            return Ok(None);
        };

        match event_rx.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    TransportEvent::Disconnected(_)
                        | TransportEvent::Error {
                            disconnect: true,
                            ..
                        }
                        | TransportEvent::Frame(TransportFrame::Close)
                ) {
                    self.connected = false;
                }
                Ok(Some(event))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.connected = false;
                Ok(Some(TransportEvent::Disconnected(
                    "UI protocol WebSocket driver stopped; reconnect will retry on next send/read."
                        .into(),
                )))
            }
        }
    }
}

impl StdioTransportDriver {
    fn new(command: String) -> Result<Self> {
        let command = command.trim().to_string();
        if command.is_empty() {
            return Err(eyre!("UI protocol stdio command must not be empty"));
        }

        Ok(Self {
            command,
            command_tx: None,
            event_rx: None,
            task: None,
            connected: false,
        })
    }

    fn label(&self) -> &str {
        &self.command
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn connect(&mut self, runtime: &Runtime) -> Result<()> {
        if self.connected {
            return Ok(());
        }

        self.disconnect();

        let mut child = runtime
            .block_on(async {
                let mut command = shell_command(&self.command);
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
            })
            .wrap_err_with(|| {
                format!(
                    "failed to launch UI protocol stdio command {}",
                    self.command
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| eyre!("UI protocol stdio child stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| eyre!("UI protocol stdio child stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| eyre!("UI protocol stdio child stderr was not piped"))?;
        let mut stdin = stdin;
        let mut lines = BufReader::new(stdout).lines();
        let mut stderr_lines = BufReader::new(stderr).lines();
        let mut stderr_open = true;
        let (command_tx, mut command_rx) = mpsc::channel(PROTOCOL_TRANSPORT_QUEUE_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel(PROTOCOL_TRANSPORT_QUEUE_CAPACITY);

        let task = runtime.spawn(async move {
            loop {
                tokio::select! {
                    command = command_rx.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        let result: std::io::Result<()> = match command {
                            TransportCommand::Text(text) => {
                                async {
                                    stdin.write_all(text.as_bytes()).await?;
                                    stdin.write_all(b"\n").await?;
                                    stdin.flush().await?;
                                    Ok(())
                                }
                                .await
                            }
                            TransportCommand::Pong(_) => Ok(()),
                        };

                        if let Err(err) = result {
                            let _ = event_tx.send(TransportEvent::Error {
                                code: "transport_send".into(),
                                message: format!("failed to write UI protocol stdio request: {err}"),
                                disconnect: true,
                            }).await;
                            break;
                        }
                    }
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(text)) => {
                                if event_tx.send(stdio_text_frame_event(text)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => {
                                let _ = event_tx.send(TransportEvent::Disconnected(
                                    "UI protocol stdio stdout closed; reconnect will relaunch on next send/read.".into(),
                                )).await;
                                break;
                            }
                            Err(err) => {
                                let _ = event_tx.send(TransportEvent::Error {
                                    code: "transport_read".into(),
                                    message: format!("failed to read UI protocol stdio response: {err}"),
                                    disconnect: true,
                                }).await;
                                break;
                            }
                        }
                    }
                    line = stderr_lines.next_line(), if stderr_open => {
                        match line {
                            Ok(Some(_)) => {}
                            Ok(None) => {
                                stderr_open = false;
                            }
                            Err(err) => {
                                stderr_open = false;
                                let _ = event_tx.send(TransportEvent::Error {
                                    code: "transport_stderr".into(),
                                    message: format!("failed to drain UI protocol stdio stderr: {err}"),
                                    disconnect: false,
                                }).await;
                            }
                        }
                    }
                    status = child.wait() => {
                        let message = match status {
                            Ok(status) => format!(
                                "UI protocol stdio child exited with {status}; reconnect will relaunch on next send/read."
                            ),
                            Err(err) => format!(
                                "failed to wait for UI protocol stdio child: {err}; reconnect will relaunch on next send/read."
                            ),
                        };
                        let _ = event_tx.send(TransportEvent::Disconnected(message)).await;
                        break;
                    }
                }
            }
        });

        self.command_tx = Some(command_tx);
        self.event_rx = Some(event_rx);
        self.task = Some(task);
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        self.command_tx = None;
        self.event_rx = None;
        if let Some(task) = self.task.take() {
            task.abort();
        }
        self.connected = false;
    }

    fn send_text(&mut self, text: String) -> Result<()> {
        self.command_tx
            .as_ref()
            .filter(|_| self.connected)
            .ok_or_else(|| eyre!("UI protocol stdio transport is not connected"))?
            .try_send(TransportCommand::Text(text))
            .map_err(|err| bounded_send_error("UI protocol stdio writer", err))
    }

    fn send_pong(&mut self, _payload: Vec<u8>) -> Result<()> {
        Ok(())
    }

    fn poll_event(&mut self) -> Result<Option<TransportEvent>> {
        let Some(event_rx) = self.event_rx.as_mut() else {
            return Ok(None);
        };

        match event_rx.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    TransportEvent::Disconnected(_)
                        | TransportEvent::Error {
                            disconnect: true,
                            ..
                        }
                ) {
                    self.connected = false;
                }
                Ok(Some(event))
            }
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.connected = false;
                Ok(Some(TransportEvent::Disconnected(
                    "UI protocol stdio driver stopped; reconnect will relaunch on next send/read."
                        .into(),
                )))
            }
        }
    }
}

fn stdio_text_frame_event(text: String) -> TransportEvent {
    if text.len() > MAX_TEXT_FRAME_BYTES {
        return TransportEvent::Error {
            code: "frame_too_large".into(),
            message: format!(
                "UI protocol stdio frame is {} bytes; max is {MAX_TEXT_FRAME_BYTES}",
                text.len()
            ),
            disconnect: true,
        };
    }
    TransportEvent::Frame(TransportFrame::Text(text))
}

fn bounded_send_error(
    label: &str,
    err: mpsc::error::TrySendError<TransportCommand>,
) -> eyre::Report {
    match err {
        mpsc::error::TrySendError::Full(_) => {
            eyre!("{label} queue is full; reconnect will retry on next send/read")
        }
        mpsc::error::TrySendError::Closed(_) => eyre!("{label} is closed"),
    }
}

fn shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    }

    #[cfg(not(windows))]
    {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    }
}

impl ProtocolAppUiBackend {
    pub fn new(launch: AppUiLaunch) -> Self {
        let (runtime, runtime_error) = match Runtime::new() {
            Ok(runtime) => (Some(runtime), None),
            Err(err) => (None, Some(err.to_string())),
        };

        Self {
            launch,
            runtime,
            runtime_error,
            driver: None,
            connection_state: ProtocolConnectionState::Disconnected,
            disconnected_status_reported: false,
            queue: VecDeque::new(),
            protocol: ProtocolExchange::default(),
        }
    }

    fn endpoint_label(&self) -> Result<String> {
        self.launch
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.label().to_string())
            .ok_or_else(|| eyre!("--mode protocol requires --endpoint <ws://...|wss://...> or --stdio-command <CMD>"))
    }

    fn ensure_driver(&mut self) -> Result<()> {
        if self.driver.is_none() {
            let endpoint =
                self.launch.endpoint.as_ref().ok_or_else(|| {
                    eyre!("--mode protocol requires --endpoint <ws://...|wss://...> or --stdio-command <CMD>")
                })?;
            self.driver = Some(ProtocolTransportDriver::from_endpoint(endpoint)?);
        }
        Ok(())
    }

    fn ensure_connected(&mut self) -> Result<()> {
        if self
            .driver
            .as_ref()
            .is_some_and(ProtocolTransportDriver::is_connected)
        {
            return Ok(());
        }

        self.ensure_driver()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| runtime_unavailable(self.runtime_error.as_deref()))?;
        let driver = self
            .driver
            .as_mut()
            .ok_or_else(|| eyre!("UI protocol transport driver is not initialized"))?;

        let should_reopen_session =
            self.disconnected_status_reported && self.launch.session_id.is_some();
        driver.connect(runtime)?;
        let endpoint = driver.label().to_string();
        self.mark_connected(&endpoint);
        if should_reopen_session {
            self.send_launch_session_open()?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn is_connected(&self) -> bool {
        self.driver
            .as_ref()
            .is_some_and(ProtocolTransportDriver::is_connected)
    }

    fn mark_connected(&mut self, endpoint: &str) {
        let should_report_reconnect = self.disconnected_status_reported;
        self.connection_state = ProtocolConnectionState::Connected;
        self.disconnected_status_reported = false;

        if should_report_reconnect {
            self.queue.push_back(
                AppUiEvent::Status(AppUiStatus {
                    message: format!("UI protocol reconnected to {endpoint}."),
                })
                .into(),
            );
        }
    }

    fn mark_disconnected(&mut self, message: impl Into<String>) {
        let message = message.into();
        if let Some(driver) = self.driver.as_mut() {
            driver.disconnect();
        }
        let cancelled_requests = self.protocol.cancel_pending_requests(&message);

        let should_report = self.connection_state != ProtocolConnectionState::Disconnected
            || !self.disconnected_status_reported;
        self.connection_state = ProtocolConnectionState::Disconnected;

        if should_report {
            self.queue
                .push_back(AppUiEvent::Status(AppUiStatus { message }).into());
            self.disconnected_status_reported = true;
        }
        self.queue
            .extend(cancelled_requests.into_iter().map(Into::into));
    }

    fn launch_session_open_command(&self) -> Option<AppUiCommand> {
        self.launch.session_id.clone().map(|session_id| {
            AppUiCommand::OpenSession(SessionOpenParams {
                session_id,
                topic: None,
                profile_id: self.launch.profile_id.clone(),
                cwd: self.launch.cwd.clone(),
                after: None,
            })
        })
    }

    fn send_launch_session_open(&mut self) -> Result<()> {
        let Some(command) = self.launch_session_open_command() else {
            return Ok(());
        };
        self.send(command)
    }

    fn build_tracked_request(
        &mut self,
        command: AppUiCommand,
    ) -> Result<RpcRequest<serde_json::Value>> {
        self.protocol.build_tracked_request(command)
    }

    fn decode_rpc_text(&mut self, text: &str) -> Result<Option<ClientEvent>> {
        self.protocol.decode_rpc_text(text)
    }

    fn readonly_allows_command(command: &AppUiCommand) -> bool {
        matches!(
            command,
            AppUiCommand::ListConfigCapabilities(_)
                | AppUiCommand::OpenSession(_)
                | AppUiCommand::ReadSessionStatus(_)
                | AppUiCommand::ListModels(_)
                | AppUiCommand::ListApprovalScopes(_)
                | AppUiCommand::ListPermissionProfiles(_)
                | AppUiCommand::ListMcpStatus(_)
                | AppUiCommand::ListToolStatus(_)
                | AppUiCommand::ListMcpConfig(_)
                | AppUiCommand::ListToolConfig(_)
                | AppUiCommand::GetDiffPreview(_)
                | AppUiCommand::ReadTaskOutput(_)
                | AppUiCommand::ReadTaskArtifact(_)
                | AppUiCommand::AuthStatus(_)
                | AppUiCommand::AuthMe(_)
                | AppUiCommand::ProfileLlmCatalog(_)
                | AppUiCommand::ProfileLlmList(_)
                | AppUiCommand::ProfileLlmFetchModels(_)
                | AppUiCommand::ProfileSkillsList(_)
                | AppUiCommand::ProfileSkillsRegistrySearch(_)
                // M15-E read-only autonomy inspection. Reconnect
                // hydration depends on these, and `--readonly` users
                // still want to see backend agent/goal/loop state.
                | AppUiCommand::ListAgents(_)
                | AppUiCommand::ReadAgentStatus(_)
                | AppUiCommand::ReadAgentOutput(_)
                | AppUiCommand::ListAgentArtifacts(_)
                | AppUiCommand::ReadAgentArtifact(_)
                | AppUiCommand::GetSessionGoal(_)
                | AppUiCommand::ListLoops(_)
        )
    }

    fn send_text(&mut self, text: String) -> Result<()> {
        self.ensure_connected()?;
        let send_result = self
            .driver
            .as_mut()
            .ok_or_else(|| eyre!("UI protocol transport driver is not initialized"))?
            .send_text(text);

        match send_result {
            Ok(()) => Ok(()),
            Err(err) => {
                self.mark_disconnected(
                    "UI protocol disconnected while sending; reconnect will retry on next send/read.",
                );
                Err(err).wrap_err("failed to send UI protocol request")
            }
        }
    }

    fn read_next_transport_event(&mut self) -> Result<Option<TransportEvent>> {
        if let Err(err) = self.ensure_connected() {
            self.mark_disconnected(format!(
                "UI protocol disconnected; reconnect will retry on next send/read: {err:#}"
            ));
            return Ok(None);
        }

        let read_result = self
            .driver
            .as_mut()
            .ok_or_else(|| eyre!("UI protocol transport driver is not initialized"))?
            .poll_event();

        match read_result {
            Ok(event) => Ok(event),
            Err(err) => {
                self.mark_disconnected(
                    "UI protocol disconnected while reading; reconnect will retry on next send/read.",
                );
                self.queue.push_back(
                    AppUiEvent::Error(AppUiError {
                        code: "transport_read".into(),
                        message: format!("failed to read UI protocol transport message: {err}"),
                    })
                    .into(),
                );
                Ok(None)
            }
        }
    }

    fn handle_transport_event(&mut self, event: TransportEvent) -> Result<Option<ClientEvent>> {
        match event {
            TransportEvent::Frame(frame) => self.handle_transport_frame(frame),
            TransportEvent::Disconnected(message) => {
                self.mark_disconnected(message);
                Ok(self.queue.pop_front())
            }
            TransportEvent::Error {
                code,
                message,
                disconnect,
            } => {
                if disconnect {
                    self.mark_disconnected(
                        "UI protocol disconnected; reconnect will retry on next send/read.",
                    );
                }
                self.queue
                    .push_back(AppUiEvent::Error(AppUiError { code, message }).into());
                Ok(self.queue.pop_front())
            }
        }
    }

    fn handle_transport_frame(&mut self, frame: TransportFrame) -> Result<Option<ClientEvent>> {
        match frame {
            TransportFrame::Text(text) => self.decode_rpc_text(&text),
            TransportFrame::Binary(bytes) => {
                let text = match String::from_utf8(bytes) {
                    Ok(text) => text,
                    Err(err) => {
                        return Ok(Some(
                            AppUiEvent::Error(AppUiError {
                                code: "malformed_frame".into(),
                                message: format!(
                                    "UI protocol binary frame was not UTF-8 JSON: {err}"
                                ),
                            })
                            .into(),
                        ));
                    }
                };
                self.decode_rpc_text(&text)
            }
            TransportFrame::Ping(payload) => {
                let pong_result = self
                    .driver
                    .as_mut()
                    .ok_or_else(|| eyre!("UI protocol transport driver is not initialized"))?
                    .send_pong(payload);
                if let Err(err) = pong_result {
                    self.mark_disconnected(
                        "UI protocol disconnected while sending pong; reconnect will retry on next send/read.",
                    );
                    self.queue.push_back(
                        AppUiEvent::Error(AppUiError {
                            code: "transport_send".into(),
                            message: format!("failed to send UI protocol pong: {err}"),
                        })
                        .into(),
                    );
                    return Ok(self.queue.pop_front());
                }
                Ok(None)
            }
            TransportFrame::Pong => Ok(None),
            TransportFrame::Close => {
                self.mark_disconnected(
                    "UI protocol WebSocket closed; reconnect will retry on next send/read.",
                );
                Ok(self.queue.pop_front())
            }
        }
    }

    #[allow(unreachable_patterns)]
    fn enqueue_readonly_blocked_response(&mut self, command: AppUiCommand) {
        let method = command.method().to_string();
        match command {
            AppUiCommand::SubmitPrompt(params) => {
                self.queue.push_back(
                    AppUiEvent::Protocol(UiNotification::Warning(WarningEvent {
                        session_id: params.session_id,
                        turn_id: Some(params.turn_id),
                        code: "readonly".into(),
                        message: "Read-only mode blocks turn/start; no network request was sent."
                            .into(),
                    }))
                    .into(),
                );
            }
            AppUiCommand::InterruptTurn(_)
            | AppUiCommand::RespondApproval(_)
            | AppUiCommand::SetPermissionProfile(_)
            | AppUiCommand::AuthSendCode(_)
            | AppUiCommand::AuthVerify(_)
            | AppUiCommand::AuthLogout(_)
            | AppUiCommand::ProfileLocalCreate(_)
            | AppUiCommand::ProfileLlmUpsert(_)
            | AppUiCommand::ProfileLlmDelete(_)
            | AppUiCommand::ProfileLlmSelect(_)
            | AppUiCommand::ProfileLlmTest(_)
            | AppUiCommand::ProfileSkillsInstall(_)
            | AppUiCommand::ProfileSkillsRemove(_)
            | AppUiCommand::UpsertMcpConfig(_)
            | AppUiCommand::DeleteMcpConfig(_)
            | AppUiCommand::SetMcpConfigEnabled(_)
            | AppUiCommand::TestMcpConfig(_)
            | AppUiCommand::SetToolConfigEnabled(_)
            | AppUiCommand::UpsertToolConfig(_)
            | AppUiCommand::DeleteToolConfig(_)
            | AppUiCommand::TestToolConfig(_) => {
                self.queue.push_back(
                    AppUiEvent::Error(AppUiError {
                        code: "readonly".into(),
                        message: format!(
                            "Read-only mode blocks {method}; no network request was sent."
                        ),
                    })
                    .into(),
                );
            }
            _ => {
                self.queue.push_back(
                    AppUiEvent::Error(AppUiError {
                        code: "readonly_policy".into(),
                        message: format!(
                            "Read-only mode unexpectedly blocked read-style {method}; no network request was sent."
                        ),
                    })
                    .into(),
                );
            }
        };
    }
}

impl AppUiBackend for ProtocolAppUiBackend {
    fn bootstrap(&mut self) -> Result<AppUiSnapshot> {
        let endpoint = self.endpoint_label()?;
        if let Err(err) = self.ensure_connected() {
            if self.launch.readonly {
                let message =
                    format!("Protocol backend read-only; no network connection opened: {err:#}");
                self.mark_disconnected(message.clone());
                return Ok(protocol_readonly_offline_snapshot_from_launch(
                    &self.launch,
                    &endpoint,
                    message,
                ));
            }
            return Err(err);
        }

        self.send(AppUiCommand::ListConfigCapabilities(
            ConfigCapabilitiesListParams {},
        ))?;
        if let Some(profile_id) = self.launch.profile_id.clone() {
            self.send(AppUiCommand::ProfileLlmList(ProfileLlmListParams {
                profile_id: Some(profile_id),
            }))?;
        }

        if let Some(session_id) = self.launch.session_id.clone() {
            self.send(AppUiCommand::OpenSession(
                octos_core::ui_protocol::SessionOpenParams {
                    session_id,
                    topic: None,
                    profile_id: self.launch.profile_id.clone(),
                    cwd: self.launch.cwd.clone(),
                    after: None,
                },
            ))?;
        }

        Ok(protocol_snapshot_from_launch(&self.launch, &endpoint))
    }

    fn send(&mut self, command: AppUiCommand) -> Result<()> {
        if self.launch.readonly && !Self::readonly_allows_command(&command) {
            self.enqueue_readonly_blocked_response(command);
            return Ok(());
        }

        if self.protocol.pending_requests.len() >= MAX_PENDING_REQUESTS {
            // M22-B: include the rejected method in the error
            // message so onboarding (and any future callers) can
            // attribute pre-send rejections back to the command
            // that was just blocked. Without this the store cannot
            // tell which command lost its slot in the queue.
            let method = command.method();
            self.queue.push_back(
                AppUiEvent::Error(AppUiError {
                    code: "too_many_pending_requests".into(),
                    message: format!(
                        "UI protocol has {} pending request(s); refusing to enqueue {method} request",
                        self.protocol.pending_requests.len()
                    ),
                })
                .into(),
            );
            return Ok(());
        }

        let request = self.build_tracked_request(command)?;
        let request_id = request.id.clone();
        let method = request.method.clone();
        let text = serde_json::to_string(&request).wrap_err("failed to encode JSON-RPC request")?;
        if text.len() > MAX_TEXT_FRAME_BYTES {
            self.protocol.pending_requests.remove(&request_id);
            self.queue.push_back(
                AppUiEvent::Error(AppUiError {
                    code: "frame_too_large".into(),
                    message: format!(
                        "encoded {method} request {request_id} is {} bytes; max is {MAX_TEXT_FRAME_BYTES}",
                        text.len()
                    ),
                })
                .into(),
            );
            return Ok(());
        }

        if let Err(err) = self.send_text(text) {
            self.mark_disconnected(format!(
                "UI protocol disconnected; reconnect will retry on next send/read: {err:#}"
            ));
            self.protocol.pending_requests.remove(&request_id);
            self.queue.push_back(
                AppUiEvent::Error(AppUiError {
                    code: "transport_send".into(),
                    message: format!("failed to send {method} request {request_id}: {err:#}"),
                })
                .into(),
            );
        }

        Ok(())
    }

    fn next_event(&mut self) -> Result<Option<ClientEvent>> {
        if let Some(event) = self.queue.pop_front() {
            return Ok(Some(event));
        }

        loop {
            let Some(event) = self.read_next_transport_event()? else {
                if let Some(event) = self.queue.pop_front() {
                    return Ok(Some(event));
                }
                return Ok(None);
            };

            if let Some(event) = self.handle_transport_event(event)? {
                return Ok(Some(event));
            }
        }
    }
}

fn runtime_unavailable(error: Option<&str>) -> eyre::Report {
    eyre!(
        "failed to create tokio runtime for UI protocol backend: {}",
        error.unwrap_or("unknown runtime initialization error")
    )
}

fn websocket_request(
    endpoint: &str,
    auth_token: Option<&str>,
    profile_id: Option<&str>,
) -> Result<WsRequest> {
    let mut request = endpoint
        .into_client_request()
        .wrap_err("failed to build UI protocol WebSocket request")?;

    if let Some(token) = auth_token.map(str::trim).filter(|token| !token.is_empty()) {
        let value = format!("Bearer {token}")
            .parse()
            .wrap_err("failed to build UI protocol Authorization header")?;
        request.headers_mut().insert("Authorization", value);
    }
    if let Some(profile_id) = profile_id
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
    {
        let value = profile_id
            .parse()
            .wrap_err("failed to build UI protocol X-Profile-Id header")?;
        request.headers_mut().insert("X-Profile-Id", value);
    }
    request.headers_mut().insert(
        "X-Octos-Ui-Features",
        format!(
            "{UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1}, {UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1}, {UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1}, {UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1}, {UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1}, {UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1}, {UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1}, {UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1}"
        )
        .parse()
        .wrap_err("failed to build UI protocol feature header")?,
    );

    Ok(request)
}

fn protocol_snapshot_from_launch(launch: &AppUiLaunch, endpoint: &str) -> AppUiSnapshot {
    let sessions = launch
        .session_id
        .clone()
        .map(|session_id| AppUiSession {
            id: session_id,
            title: "Protocol session".into(),
            profile_id: launch.profile_id.clone(),
            messages: vec![Message::system(if launch.readonly {
                format!("Read-only {UI_PROTOCOL_V1} session; mutating commands disabled")
            } else {
                format!(
                    "Connected to {UI_PROTOCOL_V1} over {}",
                    protocol_transport_description(endpoint)
                )
            })],
            tasks: vec![],
            live_reply: None,
        })
        .into_iter()
        .collect();

    AppUiSnapshot {
        sessions,
        selected_session: 0,
        status: if launch.readonly && launch.session_id.is_some() {
            "Protocol backend connected read-only; session/open sent.".into()
        } else if launch.readonly {
            "Protocol backend connected read-only. Pass --session to open an existing session."
                .into()
        } else if launch.session_id.is_some() {
            "Protocol backend connected; session/open sent.".into()
        } else {
            "Protocol backend connected. Pass --session to open an interactive session.".into()
        },
        target: Some(protocol_target_label(endpoint)),
        readonly: launch.readonly,
    }
}

fn protocol_target_label(endpoint: &str) -> String {
    if is_websocket_url(endpoint) {
        endpoint.into()
    } else {
        format!("stdio:{endpoint}")
    }
}

fn protocol_transport_description(endpoint: &str) -> &'static str {
    if is_websocket_url(endpoint) {
        "WebSocket"
    } else {
        "stdio"
    }
}

fn tui_capabilities() -> UiProtocolCapabilities {
    let mut capabilities = UiProtocolCapabilities::first_server_slice();
    for method in [
        crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST,
        crate::model::APPUI_METHOD_SESSION_STATUS_READ,
        crate::model::APPUI_METHOD_MODEL_LIST,
        crate::model::APPUI_METHOD_MODEL_SELECT,
        crate::model::APPUI_METHOD_MCP_STATUS_LIST,
        crate::model::APPUI_METHOD_TOOL_STATUS_LIST,
        crate::model::APPUI_METHOD_AUTH_STATUS,
        crate::model::APPUI_METHOD_AUTH_SEND_CODE,
        crate::model::APPUI_METHOD_AUTH_VERIFY,
        crate::model::APPUI_METHOD_AUTH_ME,
        crate::model::APPUI_METHOD_AUTH_LOGOUT,
        crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE,
        crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG,
        crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT,
        crate::model::APPUI_METHOD_PROFILE_LLM_DELETE,
        crate::model::APPUI_METHOD_PROFILE_LLM_TEST,
        crate::model::APPUI_METHOD_PROFILE_LLM_FETCH_MODELS,
        crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST,
        crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH,
        crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL,
        crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE,
    ] {
        if !capabilities
            .supported_methods
            .iter()
            .any(|existing| existing == method)
        {
            capabilities.supported_methods.push(method.into());
        }
    }
    capabilities
}

fn protocol_readonly_offline_snapshot_from_launch(
    launch: &AppUiLaunch,
    endpoint: &str,
    status: String,
) -> AppUiSnapshot {
    let mut snapshot = protocol_snapshot_from_launch(launch, endpoint);
    snapshot.status = status;
    snapshot
}

#[allow(unreachable_patterns)]
fn rpc_request_from_command(
    id: String,
    command: AppUiCommand,
) -> Result<RpcRequest<serde_json::Value>> {
    let method = command.method().to_string();
    let params = match command {
        AppUiCommand::OpenSession(params) => serde_json::to_value(params),
        AppUiCommand::ListConfigCapabilities(params) => serde_json::to_value(params),
        AppUiCommand::ReadSessionStatus(params) => serde_json::to_value(params),
        AppUiCommand::SubmitPrompt(params) => serde_json::to_value(params),
        AppUiCommand::InterruptTurn(params) => serde_json::to_value(params),
        AppUiCommand::ListModels(params) => serde_json::to_value(params),
        AppUiCommand::SelectModel(params) => serde_json::to_value(params),
        AppUiCommand::RespondApproval(params) => serde_json::to_value(params),
        AppUiCommand::ListApprovalScopes(params) => serde_json::to_value(params),
        AppUiCommand::ListPermissionProfiles(params) => serde_json::to_value(params),
        AppUiCommand::SetPermissionProfile(params) => serde_json::to_value(params),
        AppUiCommand::ListMcpStatus(params) => serde_json::to_value(params),
        AppUiCommand::ListToolStatus(params) => serde_json::to_value(params),
        AppUiCommand::ListMcpConfig(params) => serde_json::to_value(params),
        AppUiCommand::UpsertMcpConfig(params) => serde_json::to_value(params),
        AppUiCommand::DeleteMcpConfig(params) => serde_json::to_value(params),
        AppUiCommand::SetMcpConfigEnabled(params) => serde_json::to_value(params),
        AppUiCommand::TestMcpConfig(params) => serde_json::to_value(params),
        AppUiCommand::ListToolConfig(params) => serde_json::to_value(params),
        AppUiCommand::SetToolConfigEnabled(params) => serde_json::to_value(params),
        AppUiCommand::UpsertToolConfig(params) => serde_json::to_value(params),
        AppUiCommand::DeleteToolConfig(params) => serde_json::to_value(params),
        AppUiCommand::TestToolConfig(params) => serde_json::to_value(params),
        AppUiCommand::GetDiffPreview(params) => serde_json::to_value(params),
        AppUiCommand::ListTasks(params) => serde_json::to_value(params),
        AppUiCommand::CancelTask(params) => serde_json::to_value(params),
        AppUiCommand::RestartTaskFromNode(params) => serde_json::to_value(params),
        AppUiCommand::ReadTaskOutput(params) => serde_json::to_value(params),
        AppUiCommand::ReadTaskArtifact(params) => serde_json::to_value(params),
        AppUiCommand::AuthStatus(params) => serde_json::to_value(params),
        AppUiCommand::AuthSendCode(params) => serde_json::to_value(params),
        AppUiCommand::AuthVerify(params) => serde_json::to_value(params),
        AppUiCommand::AuthMe(params) => serde_json::to_value(params),
        AppUiCommand::AuthLogout(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLocalCreate(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmCatalog(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmList(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmUpsert(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmDelete(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmSelect(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmTest(params) => serde_json::to_value(params),
        AppUiCommand::ProfileLlmFetchModels(params) => serde_json::to_value(params),
        AppUiCommand::ProfileSkillsList(params) => serde_json::to_value(params),
        AppUiCommand::ProfileSkillsRegistrySearch(params) => serde_json::to_value(params),
        AppUiCommand::ProfileSkillsInstall(params) => serde_json::to_value(params),
        AppUiCommand::ProfileSkillsRemove(params) => serde_json::to_value(params),
        AppUiCommand::ListAgents(params) => serde_json::to_value(params),
        AppUiCommand::ReadAgentStatus(params) => serde_json::to_value(params),
        AppUiCommand::ReadAgentOutput(params) => serde_json::to_value(params),
        AppUiCommand::ListAgentArtifacts(params) => serde_json::to_value(params),
        AppUiCommand::ReadAgentArtifact(params) => serde_json::to_value(params),
        AppUiCommand::InterruptAgent(params) => serde_json::to_value(params),
        AppUiCommand::CloseAgent(params) => serde_json::to_value(params),
        AppUiCommand::GetSessionGoal(params) => serde_json::to_value(params),
        AppUiCommand::SetSessionGoal(params) => serde_json::to_value(params),
        AppUiCommand::ClearSessionGoal(params) => serde_json::to_value(params),
        AppUiCommand::CreateLoop(params) => serde_json::to_value(params),
        AppUiCommand::ListLoops(params) => serde_json::to_value(params),
        AppUiCommand::DeleteLoop(params)
        | AppUiCommand::PauseLoop(params)
        | AppUiCommand::ResumeLoop(params)
        | AppUiCommand::FireLoopNow(params) => serde_json::to_value(params),
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
fn rpc_text_to_app_event(text: &str) -> Result<Option<ClientEvent>> {
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
            let preview: String = text.chars().take(80).collect();
            return Ok(Some(
                app_error(
                    "malformed_json",
                    format!("UI protocol frame is not JSON: {err}; preview={preview:?}"),
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
        if method == "server/heartbeat" {
            return Ok(None);
        }
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
        crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST => {
            match serde_json::from_value::<ConfigCapabilitiesListResult>(result) {
                Ok(result) => Ok(Some(capabilities_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
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
        crate::model::APPUI_METHOD_SESSION_STATUS_READ => {
            match serde_json::from_value::<SessionStatusReadResult>(result) {
                Ok(result) => Ok(Some(session_status_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_SESSION_STATUS_READ
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_MODEL_LIST => {
            if result.get("models").is_none()
                && let Ok(result) = serde_json::from_value::<ProfileLlmListResult>(result.clone())
            {
                Ok(Some(profile_llm_list_event(result)))
            } else {
                match serde_json::from_value::<ModelListResult>(result) {
                    Ok(result) => Ok(Some(model_list_event(result))),
                    Err(err) => Ok(Some(
                        app_error(
                            "invalid_result",
                            format!(
                                "failed to decode UI protocol result for {}: {err}",
                                crate::model::APPUI_METHOD_MODEL_LIST
                            ),
                        )
                        .into(),
                    )),
                }
            }
        }
        crate::model::APPUI_METHOD_MODEL_SELECT => {
            match serde_json::from_value::<ModelSelectResult>(result) {
                Ok(result) => Ok(Some(model_select_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_MODEL_SELECT
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_AUTH_STATUS => {
            match serde_json::from_value::<AuthStatusResult>(result) {
                Ok(result) => Ok(Some(auth_status_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!("failed to decode UI protocol result for auth/status: {err}"),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_AUTH_ME => {
            match serde_json::from_value::<AuthMeResult>(result) {
                Ok(result) => Ok(Some(auth_me_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!("failed to decode UI protocol result for auth/me: {err}"),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_AUTH_SEND_CODE => {
            match serde_json::from_value::<AuthSendCodeResult>(result) {
                Ok(result) => Ok(Some(auth_send_code_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!("failed to decode UI protocol result for auth/send_code: {err}"),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_AUTH_VERIFY => {
            match serde_json::from_value::<AuthVerifyResult>(result) {
                Ok(result) => Ok(Some(auth_verify_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!("failed to decode UI protocol result for auth/verify: {err}"),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_AUTH_LOGOUT => {
            match serde_json::from_value::<AuthLogoutResult>(result) {
                Ok(result) => Ok(Some(auth_logout_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!("failed to decode UI protocol result for auth/logout: {err}"),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE => {
            match serde_json::from_value::<ProfileLocalCreateResult>(result) {
                Ok(result) => Ok(Some(profile_local_create_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_LLM_CATALOG => {
            match serde_json::from_value::<ProfileLlmCatalogResult>(result) {
                Ok(result) => Ok(Some(profile_llm_catalog_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for profile/llm/catalog: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_LLM_UPSERT
        | crate::model::APPUI_METHOD_PROFILE_LLM_DELETE
        | crate::model::APPUI_METHOD_PROFILE_LLM_TEST => {
            match serde_json::from_value::<ProfileLlmMutationResult>(result) {
                Ok(result) => Ok(Some(profile_llm_mutation_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for profile/llm mutation: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST => {
            match serde_json::from_value::<ProfileSkillsListResult>(result) {
                Ok(result) => Ok(Some(profile_skills_list_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for profile/skills/list: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH => {
            match serde_json::from_value::<ProfileSkillsRegistrySearchResult>(result) {
                Ok(result) => Ok(Some(profile_skills_registry_search_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for profile/skills/registry/search: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL
        | crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE => {
            match serde_json::from_value::<ProfileSkillsMutationResult>(result) {
                Ok(result) => Ok(Some(profile_skills_mutation_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for profile/skills mutation: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
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
                        topic: None,
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
        methods::TASK_ARTIFACT_READ => {
            match serde_json::from_value::<TaskArtifactReadResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::TaskArtifactRead(
                    result,
                )))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    methods::TASK_ARTIFACT_READ,
                    err,
                ))),
            }
        }
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
        methods::PERMISSION_PROFILE_LIST => {
            match serde_json::from_value::<PermissionProfileListResult>(result) {
                Ok(result) => Ok(Some(permission_profile_list_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            methods::PERMISSION_PROFILE_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        methods::PERMISSION_PROFILE_SET => {
            match serde_json::from_value::<PermissionProfileSetResult>(result) {
                Ok(result) => Ok(Some(permission_profile_set_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            methods::PERMISSION_PROFILE_SET
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_MCP_CONFIG_LIST => {
            match serde_json::from_value::<McpConfigListResult>(result) {
                Ok(result) => Ok(Some(mcp_config_list_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_MCP_CONFIG_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_MCP_CONFIG_UPSERT
        | crate::model::APPUI_METHOD_MCP_CONFIG_DELETE
        | crate::model::APPUI_METHOD_MCP_CONFIG_SET_ENABLED
        | crate::model::APPUI_METHOD_MCP_CONFIG_TEST => {
            match serde_json::from_value::<McpConfigMutationResult>(result) {
                Ok(result) => Ok(Some(mcp_config_mutation_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for mcp/config mutation: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_MCP_STATUS_LIST => {
            match serde_json::from_value::<McpStatusListResult>(result) {
                Ok(result) => Ok(Some(mcp_status_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_MCP_STATUS_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_TOOL_CONFIG_LIST => {
            match serde_json::from_value::<ToolConfigListResult>(result) {
                Ok(result) => Ok(Some(tool_config_list_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_TOOL_CONFIG_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_TOOL_CONFIG_SET_ENABLED
        | crate::model::APPUI_METHOD_TOOL_CONFIG_UPSERT
        | crate::model::APPUI_METHOD_TOOL_CONFIG_DELETE
        | crate::model::APPUI_METHOD_TOOL_CONFIG_TEST => {
            match serde_json::from_value::<ToolConfigMutationResult>(result) {
                Ok(result) => Ok(Some(tool_config_mutation_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for tool/config mutation: {err}"
                        ),
                    )
                    .into(),
                )),
            }
        }
        crate::model::APPUI_METHOD_TOOL_STATUS_LIST => {
            match serde_json::from_value::<ToolStatusListResult>(result) {
                Ok(result) => Ok(Some(tool_status_event(result))),
                Err(err) => Ok(Some(
                    app_error(
                        "invalid_result",
                        format!(
                            "failed to decode UI protocol result for {}: {err}",
                            crate::model::APPUI_METHOD_TOOL_STATUS_LIST
                        ),
                    )
                    .into(),
                )),
            }
        }
        // M15-E autonomy results. We decode and forward as
        // ClientEvent::Autonomy so the store can update the per-session
        // mirror.
        crate::model::APPUI_METHOD_AGENT_LIST => {
            match serde_json::from_value::<crate::model::AgentListResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentList(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_LIST,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_STATUS_READ => {
            match serde_json::from_value::<crate::model::AgentStatusReadResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentStatus(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_STATUS_READ,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_OUTPUT_READ => {
            match serde_json::from_value::<crate::model::AgentOutputReadResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentOutput(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_OUTPUT_READ,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_ARTIFACT_LIST => {
            match serde_json::from_value::<crate::model::AgentArtifactListResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentArtifacts(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_ARTIFACT_LIST,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_ARTIFACT_READ => {
            match serde_json::from_value::<crate::model::AgentArtifactReadResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentArtifactRead(
                    result,
                )))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_ARTIFACT_READ,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_INTERRUPT => {
            match serde_json::from_value::<crate::model::AgentInterruptResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentInterrupt(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_INTERRUPT,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_AGENT_CLOSE => {
            match serde_json::from_value::<crate::model::AgentCloseResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::AgentClose(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_AGENT_CLOSE,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_SESSION_GOAL_GET => {
            match serde_json::from_value::<crate::model::SessionGoalGetResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::GoalGet(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_SESSION_GOAL_GET,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_SESSION_GOAL_SET => {
            match serde_json::from_value::<crate::model::SessionGoalSetResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::GoalSet(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_SESSION_GOAL_SET,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_SESSION_GOAL_CLEAR => {
            match serde_json::from_value::<crate::model::SessionGoalClearResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::GoalClear(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_SESSION_GOAL_CLEAR,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_LOOP_CREATE => {
            match serde_json::from_value::<crate::model::LoopCreateResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::LoopCreate(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_LOOP_CREATE,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_LOOP_LIST => {
            match serde_json::from_value::<crate::model::LoopListResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::LoopList(result)))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    crate::model::APPUI_METHOD_LOOP_LIST,
                    err,
                ))),
            }
        }
        crate::model::APPUI_METHOD_LOOP_DELETE
        | crate::model::APPUI_METHOD_LOOP_PAUSE
        | crate::model::APPUI_METHOD_LOOP_RESUME
        | crate::model::APPUI_METHOD_LOOP_FIRE_NOW => {
            match serde_json::from_value::<crate::model::LoopMutationResult>(result) {
                Ok(result) => Ok(Some(autonomy_event(AutonomyResult::LoopMutation {
                    method: pending_request.method.clone(),
                    result,
                }))),
                Err(err) => Ok(Some(autonomy_decode_error(
                    pending_request.method.as_str(),
                    err,
                ))),
            }
        }
        _ => Ok(None),
    }
}

fn autonomy_event(result: AutonomyResult) -> ClientEvent {
    ClientEvent::Autonomy(AutonomyClientEvent { result })
}

fn autonomy_decode_error(method: &str, err: serde_json::Error) -> ClientEvent {
    app_error(
        "invalid_result",
        format!("failed to decode UI protocol result for {method}: {err}"),
    )
    .into()
}

fn decode_task_output_read_result(mut result: Value) -> serde_json::Result<TaskOutputReadResult> {
    if let Some(object) = result.as_object_mut() {
        object
            .entry("is_snapshot_projection")
            .or_insert(Value::Bool(false));
    }
    serde_json::from_value(result)
}

fn capabilities_event(result: ConfigCapabilitiesListResult) -> ClientEvent {
    let message = format!(
        "AppUI capabilities refreshed: {} methods",
        result.capabilities.supported_methods.len()
    );
    ClientEvent::Capabilities(CapabilitiesClientEvent { result, message })
}

fn session_status_event(result: SessionStatusReadResult) -> ClientEvent {
    let label = result
        .runtime_policy_stamp
        .as_ref()
        .and_then(|stamp| stamp.model.as_deref())
        .or_else(|| result.model.as_ref().map(|model| model.model.as_str()))
        .unwrap_or("runtime");
    ClientEvent::SessionStatus(SessionStatusClientEvent {
        message: format!("Runtime status refreshed: {label}"),
        result,
    })
}

fn model_list_event(result: ModelListResult) -> ClientEvent {
    let count = result.models.len();
    ClientEvent::ModelList(ModelListClientEvent {
        message: match count {
            0 => "Model list refreshed: no models".into(),
            1 => "Model list refreshed: 1 model".into(),
            _ => format!("Model list refreshed: {count} models"),
        },
        result,
    })
}

fn model_select_event(result: ModelSelectResult) -> ClientEvent {
    let prefix = if result.applied {
        "Model selected"
    } else {
        "Model unchanged"
    };
    ClientEvent::ModelSelect(ModelSelectClientEvent {
        message: format!(
            "{prefix}: {} / {}",
            result.selected.provider, result.selected.model
        ),
        result,
    })
}

fn profile_llm_catalog_event(result: ProfileLlmCatalogResult) -> ClientEvent {
    let count = result.families.len();
    ClientEvent::ProfileLlmCatalog(ProfileLlmCatalogClientEvent {
        message: format!("Provider catalog refreshed: {count} family(s)"),
        result,
    })
}

fn profile_llm_list_event(result: ProfileLlmListResult) -> ClientEvent {
    let count = result.primary.iter().count() + result.fallbacks.len();
    ClientEvent::ProfileLlmList(ProfileLlmListClientEvent {
        message: match count {
            0 => "Configured providers refreshed: none".into(),
            1 => "Configured providers refreshed: 1 provider".into(),
            _ => format!("Configured providers refreshed: {count} providers"),
        },
        result,
    })
}

fn profile_llm_mutation_event(result: ProfileLlmMutationResult) -> ClientEvent {
    let count = result.models().len();
    let message = match (
        result.applied,
        result.message.as_deref(),
        result.error.as_deref(),
    ) {
        (false, Some(message), Some(error)) => format!("{message}: {error}"),
        (_, Some(message), _) => message.to_owned(),
        (false, None, Some(error)) => format!("Provider operation failed: {error}"),
        _ => format!("Provider profile updated: {count} configured provider(s)"),
    };
    ClientEvent::ProfileLlmMutation(ProfileLlmMutationClientEvent { message, result })
}

fn profile_skills_list_event(result: ProfileSkillsListResult) -> ClientEvent {
    let count = result.skills.len();
    ClientEvent::ProfileSkillsList(ProfileSkillsListClientEvent {
        message: match count {
            0 => "Profile skills refreshed: none installed".into(),
            1 => "Profile skills refreshed: 1 installed skill".into(),
            _ => format!("Profile skills refreshed: {count} installed skills"),
        },
        result,
    })
}

fn profile_skills_registry_search_event(result: ProfileSkillsRegistrySearchResult) -> ClientEvent {
    let count = result.packages.len();
    ClientEvent::ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchClientEvent {
        message: match count {
            0 => "Skill registry search returned no packages".into(),
            1 => "Skill registry search returned 1 package".into(),
            _ => format!("Skill registry search returned {count} packages"),
        },
        result,
    })
}

fn profile_skills_mutation_event(result: ProfileSkillsMutationResult) -> ClientEvent {
    let message = if !result.ok {
        result
            .message
            .clone()
            .unwrap_or_else(|| "Skill operation failed".into())
    } else if let Some(removed) = &result.removed {
        format!("Removed skill: {removed}")
    } else if !result.installed.is_empty() {
        format!("Installed skill(s): {}", result.installed.join(", "))
    } else if !result.skipped.is_empty() {
        format!("Skill install skipped: {}", result.skipped.join(", "))
    } else {
        result
            .message
            .clone()
            .unwrap_or_else(|| "Skill operation completed".into())
    };
    ClientEvent::ProfileSkillsMutation(ProfileSkillsMutationClientEvent { message, result })
}

fn auth_status_event(result: AuthStatusResult) -> ClientEvent {
    ClientEvent::AuthStatus(AuthStatusClientEvent {
        message: auth_status_message(&result),
        result,
    })
}

fn auth_send_code_event(result: AuthSendCodeResult) -> ClientEvent {
    let message = result
        .message
        .clone()
        .unwrap_or_else(|| "OTP send acknowledged".into());
    ClientEvent::AuthSendCode(AuthSendCodeClientEvent { result, message })
}

fn auth_verify_event(result: AuthVerifyResult) -> ClientEvent {
    let message = result.message.clone().unwrap_or_else(|| {
        if result.ok {
            "OTP verified"
        } else {
            "OTP verify failed"
        }
        .into()
    });
    ClientEvent::AuthVerify(AuthVerifyClientEvent { result, message })
}

fn auth_me_event(result: AuthMeResult) -> ClientEvent {
    ClientEvent::AuthMe(AuthMeClientEvent {
        message: auth_me_message(&result),
        result,
    })
}

fn auth_logout_event(result: AuthLogoutResult) -> ClientEvent {
    let message = result.message.clone().unwrap_or_else(|| {
        if result.ok {
            "Logged out"
        } else {
            "Logout failed"
        }
        .into()
    });
    ClientEvent::AuthLogout(AuthLogoutClientEvent { result, message })
}

fn profile_local_create_event(result: ProfileLocalCreateResult) -> ClientEvent {
    let action = if result.created { "created" } else { "loaded" };
    ClientEvent::ProfileLocalCreate(ProfileLocalCreateClientEvent {
        message: format!(
            "Local solo profile {action}: {} ({})",
            result.profile_id, result.email
        ),
        result,
    })
}

fn auth_status_message(result: &AuthStatusResult) -> String {
    if let Some(profile) = result.scoped_profile.as_ref() {
        format!("Authenticated for profile {}", profile.id)
    } else if result.authenticated {
        format!(
            "Authenticated{}",
            result
                .profile_id
                .as_deref()
                .map(|profile| format!(" for profile {profile}"))
                .unwrap_or_default()
        )
    } else if result.email_login_enabled || result.email_otp {
        "Not authenticated; email OTP is available".into()
    } else {
        "Not authenticated".into()
    }
}

fn auth_me_message(result: &AuthMeResult) -> String {
    let email = auth_me_email(result).unwrap_or("unknown account");
    let profile = auth_me_profile_id(result).unwrap_or("no profile");
    format!("Authenticated account: {email} ({profile})")
}

fn mcp_status_event(result: McpStatusListResult) -> ClientEvent {
    let connected = result
        .servers
        .iter()
        .filter(|server| server.status == "connected")
        .count();
    let failed = result
        .servers
        .iter()
        .filter(|server| server.status == "failed")
        .count();
    ClientEvent::McpStatus(McpStatusClientEvent {
        message: format!(
            "MCP status refreshed: {} server(s), {connected} connected, {failed} failed",
            result.servers.len()
        ),
        result,
    })
}

fn mcp_config_list_event(result: McpConfigListResult) -> ClientEvent {
    let enabled = result
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    ClientEvent::McpConfigList(McpConfigListClientEvent {
        message: format!(
            "MCP config refreshed: {} server(s), {enabled} enabled",
            result.servers.len()
        ),
        result,
    })
}

fn mcp_config_mutation_event(result: McpConfigMutationResult) -> ClientEvent {
    let subject = result
        .server
        .as_deref()
        .or_else(|| result.entry.as_ref().map(|entry| entry.name.as_str()))
        .unwrap_or("server");
    let status = if result.applied || result.ok {
        "applied"
    } else {
        "completed"
    };
    let message = result
        .message
        .clone()
        .unwrap_or_else(|| format!("MCP config {status}: {subject}"));
    ClientEvent::McpConfigMutation(McpConfigMutationClientEvent { message, result })
}

fn tool_status_event(result: ToolStatusListResult) -> ClientEvent {
    let visible = result.tools.iter().filter(|tool| tool.visible).count();
    let denied = result
        .tools
        .iter()
        .filter(|tool| tool.denial.is_some())
        .count();
    ClientEvent::ToolStatus(ToolStatusClientEvent {
        message: format!(
            "Tool status refreshed: {visible} visible, {denied} denied under {}",
            result.policy_id.as_deref().unwrap_or("server policy")
        ),
        result,
    })
}

fn tool_config_list_event(result: ToolConfigListResult) -> ClientEvent {
    let enabled = result.tools.iter().filter(|tool| tool.enabled).count();
    ClientEvent::ToolConfigList(ToolConfigListClientEvent {
        message: format!(
            "Tool config refreshed: {} tool(s), {enabled} enabled",
            result.tools.len()
        ),
        result,
    })
}

fn tool_config_mutation_event(result: ToolConfigMutationResult) -> ClientEvent {
    let subject = result
        .tool
        .as_deref()
        .or_else(|| result.entry.as_ref().map(|entry| entry.tool.as_str()))
        .unwrap_or("tool");
    let status = if result.applied || result.ok {
        "applied"
    } else {
        "completed"
    };
    let message = result
        .message
        .clone()
        .unwrap_or_else(|| format!("Tool config {status}: {subject}"));
    ClientEvent::ToolConfigMutation(ToolConfigMutationClientEvent { message, result })
}

fn approval_scopes_status(result: &ApprovalScopesListResult) -> String {
    let count = result.scopes.len();
    match count {
        0 => "No persisted approval scopes for this session".into(),
        1 => "1 persisted approval scope for this session".into(),
        _ => format!("{count} persisted approval scopes for this session"),
    }
}

fn permission_profile_list_status(result: &PermissionProfileListResult) -> String {
    format!("Permissions: {}", result.current.summary())
}

fn permission_profile_list_event(result: PermissionProfileListResult) -> ClientEvent {
    ClientEvent::PermissionProfile(PermissionProfileClientEvent {
        message: permission_profile_list_status(&result),
        session_id: result.session_id,
        current: result.current,
    })
}

fn permission_profile_set_status(result: &PermissionProfileSetResult) -> String {
    let prefix = if result.applied {
        "Permissions updated"
    } else {
        "Permissions unchanged"
    };
    format!("{prefix}: {}", result.current.summary())
}

fn permission_profile_set_event(result: PermissionProfileSetResult) -> ClientEvent {
    ClientEvent::PermissionProfile(PermissionProfileClientEvent {
        message: permission_profile_set_status(&result),
        session_id: result.session_id,
        current: result.current,
    })
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
    // Prefer the structured `data.kind` discriminator the M12-F /
    // mcp/profile/tool error families publish (`policy_rejected`,
    // `tenant_restriction`, `cloud_restriction`, `mcp_invalid_config`,
    // `tool_not_found`, `profile_not_found`, `readonly_profile`, …).
    // The numeric JSON-RPC `code` is the fallback because it collapses
    // every server-side rejection into the same generic
    // `application_error` integer and would otherwise hide the policy
    // reason from the structured error renderer.
    if let Some(kind) = error
        .get("data")
        .and_then(|data| data.get("kind"))
        .and_then(Value::as_str)
        .filter(|kind| !kind.trim().is_empty())
    {
        return kind.to_owned();
    }
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
    // Prefer `data.message` when present (the structured-error family
    // uses it for the human-readable variant of `data.kind`). Fall
    // back to the top-level JSON-RPC `message`.
    if let Some(message) = error
        .get("data")
        .and_then(|data| data.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
    {
        return message.to_owned();
    }
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

fn is_websocket_url(value: &str) -> bool {
    let value = value.trim_start();
    value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ws://"))
        || value
            .get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("wss://"))
}

fn session_opened_compat(
    session_id: SessionKey,
    active_profile_id: Option<String>,
    workspace_root: Option<String>,
    cursor: Option<UiCursor>,
    panes: Option<UiPaneSnapshot>,
) -> SessionOpened {
    serde_json::from_value(serde_json::json!({
        "session_id": session_id,
        "active_profile_id": active_profile_id,
        "workspace_root": workspace_root,
        "cursor": cursor,
        "panes": panes,
    }))
    .expect("mock session/opened payload must match octos-core")
}

#[derive(Default)]
pub struct MockAppUiBackend {
    queue: VecDeque<ClientEvent>,
    launch: AppUiLaunch,
    permission_profiles: HashMap<SessionKey, PermissionProfileSelection>,
}

impl MockAppUiBackend {
    pub fn new(launch: AppUiLaunch) -> Self {
        Self {
            queue: VecDeque::new(),
            launch,
            permission_profiles: HashMap::new(),
        }
    }

    fn profile_id(&self) -> String {
        self.launch
            .profile_id
            .clone()
            .unwrap_or_else(|| "coding".into())
    }

    fn target_label(&self) -> Option<String> {
        self.launch
            .endpoint
            .as_ref()
            .map(|endpoint| endpoint.label().to_string())
            .or_else(|| Some("local mock snapshot".into()))
    }

    fn mock_session_key(profile_id: &str, topic: &str) -> SessionKey {
        SessionKey::with_profile_topic(profile_id, "local", "prototype", topic)
    }

    fn enqueue_protocol(&mut self, notification: UiNotification) {
        self.queue
            .push_back(AppUiEvent::Protocol(notification).into());
    }

    fn enqueue_turn_script(&mut self, session_id: &SessionKey, turn_id: TurnId) {
        let build_task_id = TaskId::new();

        self.enqueue_protocol(UiNotification::TurnStarted(TurnStartedEvent {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            timestamp: Utc::now(),
            topic: None,
        }));
        self.enqueue_protocol(UiNotification::ToolStarted(ToolStartedEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            tool_call_id: "mock.read_file.1".into(),
            tool_name: "read_file".into(),
            arguments: Some(serde_json::json!({"path": "src/lib.rs"})),
        }));
        self.enqueue_protocol(UiNotification::ToolProgress(ToolProgressEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            tool_call_id: "mock.read_file.1".into(),
            message: Some("Hydrating prototype context".into()),
            progress_pct: Some(0.25),
        }));
        self.enqueue_protocol(UiNotification::MessageDelta(MessageDeltaEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            text: "Planning ".into(),
        }));
        self.enqueue_protocol(UiNotification::MessageDelta(MessageDeltaEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            text: "a safe ".into(),
        }));
        self.enqueue_protocol(UiNotification::MessageDelta(MessageDeltaEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            text: "M9 scaffold over mock transport.".into(),
        }));
        self.enqueue_protocol(UiNotification::TaskUpdated(TaskUpdatedEvent {
            session_id: session_id.clone(),
            topic: None,
            task_id: build_task_id.clone(),
            tool_call_id: None,
            title: "mock background synthesis".into(),
            state: TaskRuntimeState::Running,
            runtime_detail: Some("Synthesizing task output stream".into()),
            source: None,
            role: None,
            summary: None,
            artifact_count: None,
            runtime_policy_stamp: None,
        }));
        self.enqueue_protocol(UiNotification::TaskOutputDelta(TaskOutputDeltaEvent {
            session_id: session_id.clone(),
            topic: None,
            task_id: build_task_id.clone(),
            cursor: OutputCursor { offset: 42 },
            text: "mock worker: draft protocol notifications\n".into(),
        }));
        self.enqueue_protocol(UiNotification::ApprovalRequested(mock_approval_event(
            session_id.clone(),
            turn_id.clone(),
            mock_approval_kind(),
        )));
        self.enqueue_protocol(UiNotification::ToolCompleted(ToolCompletedEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id: turn_id.clone(),
            tool_call_id: "mock.read_file.1".into(),
            tool_name: "read_file".into(),
            success: Some(true),
            output_preview: Some("1 | pub fn demo() {}\n".into()),
            duration_ms: Some(420),
        }));
        self.enqueue_protocol(UiNotification::TaskUpdated(TaskUpdatedEvent {
            session_id: session_id.clone(),
            topic: None,
            task_id: build_task_id,
            tool_call_id: None,
            title: "mock background synthesis".into(),
            state: TaskRuntimeState::Completed,
            runtime_detail: Some("Summary ready in runtime_detail".into()),
            source: None,
            role: None,
            summary: None,
            artifact_count: None,
            runtime_policy_stamp: None,
        }));
        self.enqueue_protocol(UiNotification::Warning(WarningEvent {
            session_id: session_id.clone(),
            turn_id: Some(turn_id.clone()),
            code: "mock_protocol_boundary".into(),
            message:
                "Interactive approval, diff preview, and task output are still draft M9 surfaces."
                    .into(),
        }));
        self.enqueue_protocol(UiNotification::TurnCompleted(TurnCompletedEvent {
            session_id: session_id.clone(),
            topic: None,
            turn_id,
            cursor: Some(UiCursor {
                stream: "session_events".into(),
                seq: 1,
            }),
            tokens_in: None,
            tokens_out: None,
            session_result: None,
        }));
    }
}

impl AppUiBackend for MockAppUiBackend {
    fn bootstrap(&mut self) -> Result<AppUiSnapshot> {
        let profile_id = self.profile_id();
        let requested_session = self.launch.session_id.clone();
        let coding_session = AppUiSession {
            id: requested_session
                .clone()
                .unwrap_or_else(|| Self::mock_session_key(&profile_id, "m9")),
            title: if requested_session.is_some() {
                "Requested session".into()
            } else {
                "M9 protocol draft".into()
            },
            profile_id: Some(profile_id.clone()),
            messages: vec![
                Message::system("Mock bootstrap over octos-app-ui/v1alpha1"),
                Message::assistant(
                    "This prototype is intentionally decoupled from unresolved M8 runtime behavior.",
                ),
            ],
            tasks: vec![AppUiTask {
                id: TaskId::new(),
                title: "protocol spike".into(),
                state: TaskRuntimeState::Running,
                runtime_detail: Some("Spec + types drafted in octos-core".into()),
                output_tail: "bootstrap: seeded mock session\n".into(),
            }],
            live_reply: None,
        };

        let review_session = AppUiSession {
            id: Self::mock_session_key(&profile_id, "review"),
            title: "M8 gate review".into(),
            profile_id: Some(profile_id.clone()),
            messages: vec![
                Message::system("Known M8 runtime defects stay out of the protocol contract."),
                Message::assistant(
                    "Use this session to pressure-test session/task UI without touching server behavior.",
                ),
            ],
            tasks: vec![AppUiTask {
                id: TaskId::new(),
                title: "fix-first checklist".into(),
                state: TaskRuntimeState::Completed,
                runtime_detail: Some("Checklist written in docs/".into()),
                output_tail: "review: m8 gate recorded\n".into(),
            }],
            live_reply: None,
        };

        self.enqueue_protocol(UiNotification::SessionOpened(session_opened_compat(
            coding_session.id.clone(),
            coding_session.profile_id.clone(),
            self.launch.cwd.clone(),
            Some(UiCursor {
                stream: "session_events".into(),
                seq: 0,
            }),
            None,
        )));

        Ok(AppUiSnapshot {
            sessions: vec![coding_session, review_session],
            selected_session: 0,
            status: if self.launch.readonly {
                "Mock snapshot ready in read-only mode. Turn sends are disabled.".into()
            } else {
                "Mock backend ready. Start typing to exercise M9.1 app-ui events.".into()
            },
            target: self.target_label(),
            readonly: self.launch.readonly,
        })
    }

    #[allow(unreachable_patterns)]
    fn send(&mut self, command: AppUiCommand) -> Result<()> {
        let method = command.method().to_string();
        match command {
            AppUiCommand::ListConfigCapabilities(_) => {
                self.queue
                    .push_back(capabilities_event(ConfigCapabilitiesListResult {
                        capabilities: tui_capabilities(),
                    }));
                Ok(())
            }
            AppUiCommand::SubmitPrompt(params) => {
                if self.launch.readonly {
                    self.enqueue_protocol(UiNotification::Warning(WarningEvent {
                        session_id: params.session_id,
                        turn_id: Some(params.turn_id),
                        code: "readonly".into(),
                        message: "Read-only mode blocks turn/start.".into(),
                    }));
                    return Ok(());
                }
                self.enqueue_turn_script(&params.session_id, params.turn_id);
                Ok(())
            }
            AppUiCommand::OpenSession(params) => {
                self.enqueue_protocol(UiNotification::SessionOpened(session_opened_compat(
                    params.session_id,
                    params.profile_id,
                    params.cwd,
                    params.after,
                    None,
                )));
                Ok(())
            }
            AppUiCommand::ReadSessionStatus(params) => {
                self.queue
                    .push_back(session_status_event(mock_session_status(
                        params.session_id,
                        self.launch.cwd.clone(),
                        self.launch.readonly,
                    )));
                Ok(())
            }
            AppUiCommand::InterruptTurn(_) => {
                self.enqueue_protocol(UiNotification::Warning(WarningEvent {
                    session_id: SessionKey("local:prototype#interrupt".into()),
                    turn_id: None,
                    code: "mock_interrupt".into(),
                    message: "Interrupt is not yet wired in the mock backend.".into(),
                }));
                Ok(())
            }
            AppUiCommand::ListModels(params) => {
                self.queue.push_back(model_list_event(ModelListResult {
                    session_id: params.session_id,
                    models: vec![mock_model_status(true), mock_alt_model_status()],
                }));
                Ok(())
            }
            AppUiCommand::ProfileLlmList(_) => {
                self.queue
                    .push_back(profile_llm_list_event(mock_profile_llm_list()));
                Ok(())
            }
            AppUiCommand::SelectModel(params) => {
                let selected = ModelStatus {
                    model: params.model,
                    provider: params.provider.unwrap_or_else(|| "mock".into()),
                    title: None,
                    family: None,
                    route: params.route,
                    selected: true,
                    available: Some(true),
                    queue_mode: Some("interactive".into()),
                    qoe_policy: Some("mock".into()),
                };
                self.queue.push_back(model_select_event(ModelSelectResult {
                    session_id: params.session_id,
                    selected,
                    applied: true,
                    runtime_policy_stamp: None,
                }));
                Ok(())
            }
            AppUiCommand::ProfileLlmCatalog(_) => {
                self.queue
                    .push_back(profile_llm_catalog_event(mock_profile_llm_catalog()));
                Ok(())
            }
            AppUiCommand::ProfileLlmSelect(params) => {
                let selected = ModelStatus {
                    model: params.model_id,
                    provider: params.family_id,
                    title: None,
                    family: None,
                    route: Some(params.route_id),
                    selected: true,
                    available: Some(true),
                    queue_mode: None,
                    qoe_policy: None,
                };
                self.queue.push_back(model_select_event(ModelSelectResult {
                    session_id: Self::mock_session_key(&self.profile_id(), "m9"),
                    selected,
                    applied: true,
                    runtime_policy_stamp: None,
                }));
                Ok(())
            }
            AppUiCommand::ProfileLlmUpsert(_)
            | AppUiCommand::ProfileLlmDelete(_)
            | AppUiCommand::ProfileLlmTest(_) => {
                self.queue
                    .push_back(profile_llm_mutation_event(ProfileLlmMutationResult {
                        profile_id: Some(self.profile_id()),
                        primary: mock_profile_llm_list().primary,
                        fallbacks: mock_profile_llm_list().fallbacks,
                        applied: true,
                        llm: None,
                        runtime_policy_stamp: None,
                        message: None,
                        error: None,
                    }));
                Ok(())
            }
            AppUiCommand::AuthStatus(_) => {
                self.queue.push_back(auth_status_event(AuthStatusResult {
                    bootstrap_mode: false,
                    email_login_enabled: true,
                    admin_token_login_enabled: false,
                    allow_self_registration: true,
                    scoped_profile: None,
                    authenticated: false,
                    email_otp: true,
                    token_login: false,
                    profile_id: None,
                }));
                Ok(())
            }
            AppUiCommand::AuthMe(_) => {
                self.queue.push_back(auth_me_event(AuthMeResult::Legacy {
                    email: Some("mock@example.com".into()),
                    profile_id: Some(self.profile_id()),
                }));
                Ok(())
            }
            AppUiCommand::AuthSendCode(_) => {
                self.queue
                    .push_back(auth_send_code_event(AuthSendCodeResult {
                        ok: true,
                        message: Some("OTP send acknowledged".into()),
                    }));
                Ok(())
            }
            AppUiCommand::AuthVerify(_) => {
                self.queue.push_back(auth_verify_event(AuthVerifyResult {
                    ok: true,
                    token: Some(AppUiAuthToken::new("mock-token")),
                    user: Some(serde_json::json!({
                        "id": self.profile_id(),
                        "email": "mock@example.com"
                    })),
                    message: Some("OTP verify acknowledged".into()),
                }));
                Ok(())
            }
            AppUiCommand::AuthLogout(_) => {
                self.queue.push_back(auth_logout_event(AuthLogoutResult {
                    ok: true,
                    message: Some("Logout acknowledged".into()),
                }));
                Ok(())
            }
            AppUiCommand::ProfileLocalCreate(params) => {
                self.queue
                    .push_back(profile_local_create_event(ProfileLocalCreateResult {
                        profile_id: format!("local-{}", params.username),
                        user_id: format!("local-{}", params.username),
                        name: params.name,
                        username: params.username,
                        email: params.email,
                        created: true,
                        runtime_mode: "solo".into(),
                    }));
                Ok(())
            }
            AppUiCommand::ProfileLlmFetchModels(_) => {
                self.queue.push_back(
                    AppUiEvent::Status(AppUiStatus {
                        message: "Mock fetch_models returned no additional models".into(),
                    })
                    .into(),
                );
                Ok(())
            }
            AppUiCommand::ProfileSkillsList(_) => {
                self.queue
                    .push_back(profile_skills_list_event(mock_profile_skills()));
                Ok(())
            }
            AppUiCommand::ProfileSkillsRegistrySearch(_) => {
                self.queue
                    .push_back(profile_skills_registry_search_event(mock_skill_registry()));
                Ok(())
            }
            AppUiCommand::ProfileSkillsInstall(params) => {
                self.queue
                    .push_back(profile_skills_mutation_event(ProfileSkillsMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        installed: vec![
                            params
                                .repo
                                .rsplit('/')
                                .next()
                                .unwrap_or(params.repo.as_str())
                                .to_owned(),
                        ],
                        skipped: Vec::new(),
                        deps_installed: Vec::new(),
                        removed: None,
                        message: None,
                    }));
                Ok(())
            }
            AppUiCommand::ProfileSkillsRemove(params) => {
                self.queue
                    .push_back(profile_skills_mutation_event(ProfileSkillsMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        installed: Vec::new(),
                        skipped: Vec::new(),
                        deps_installed: Vec::new(),
                        removed: Some(params.name),
                        message: None,
                    }));
                Ok(())
            }
            AppUiCommand::RespondApproval(params) => {
                self.enqueue_protocol(UiNotification::Warning(WarningEvent {
                    session_id: params.session_id,
                    turn_id: None,
                    code: "mock_approval_response".into(),
                    message: format!("Mock approval response recorded: {:?}", params.decision),
                }));
                Ok(())
            }
            AppUiCommand::ListApprovalScopes(_) => {
                self.queue.push_back(
                    AppUiEvent::Status(AppUiStatus {
                        message: "Mock backend has no persisted approval scopes.".into(),
                    })
                    .into(),
                );
                Ok(())
            }
            AppUiCommand::ListPermissionProfiles(params) => {
                let current = self
                    .permission_profiles
                    .get(&params.session_id)
                    .copied()
                    .unwrap_or_default();
                self.queue
                    .push_back(permission_profile_list_event(PermissionProfileListResult {
                        session_id: params.session_id,
                        current,
                        profiles: Vec::new(),
                    }));
                Ok(())
            }
            AppUiCommand::SetPermissionProfile(params) => {
                let previous = self
                    .permission_profiles
                    .get(&params.session_id)
                    .copied()
                    .unwrap_or_default();
                let current = params.update.apply_to(previous);
                let applied = current != previous;
                self.permission_profiles
                    .insert(params.session_id.clone(), current);
                self.queue
                    .push_back(permission_profile_set_event(PermissionProfileSetResult {
                        session_id: params.session_id,
                        current,
                        applied,
                    }));
                Ok(())
            }
            AppUiCommand::ListMcpStatus(params) => {
                self.queue.push_back(mcp_status_event(McpStatusListResult {
                    session_id: params.session_id,
                    servers: mock_mcp_servers(),
                }));
                Ok(())
            }
            AppUiCommand::ListMcpConfig(params) => {
                self.queue
                    .push_back(mcp_config_list_event(McpConfigListResult {
                        session_id: params.session_id,
                        profile_id: params.profile_id,
                        servers: mock_mcp_config_entries(),
                    }));
                Ok(())
            }
            AppUiCommand::UpsertMcpConfig(params) => {
                self.queue
                    .push_back(mcp_config_mutation_event(McpConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        server: Some(params.server),
                        message: Some("Mock MCP config upserted".into()),
                        ..McpConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::DeleteMcpConfig(params) => {
                self.queue
                    .push_back(mcp_config_mutation_event(McpConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        server: Some(params.server),
                        message: Some("Mock MCP config deleted".into()),
                        ..McpConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::SetMcpConfigEnabled(params) => {
                self.queue
                    .push_back(mcp_config_mutation_event(McpConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        server: Some(params.server),
                        message: Some(if params.enabled {
                            "Mock MCP config enabled".into()
                        } else {
                            "Mock MCP config disabled".into()
                        }),
                        ..McpConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::TestMcpConfig(params) => {
                self.queue
                    .push_back(mcp_config_mutation_event(McpConfigMutationResult {
                        session_id: params.session_id,
                        profile_id: params.profile_id,
                        ok: true,
                        applied: false,
                        server: Some(params.server),
                        message: Some("Mock MCP config test passed".into()),
                        ..McpConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::ListToolStatus(params) => {
                self.queue
                    .push_back(tool_status_event(ToolStatusListResult {
                        session_id: params.session_id,
                        policy_id: Some("mock-coding".into()),
                        coding_tool_contract: None,
                        tools: mock_tool_statuses(),
                    }));
                Ok(())
            }
            AppUiCommand::ListToolConfig(params) => {
                self.queue
                    .push_back(tool_config_list_event(ToolConfigListResult {
                        session_id: params.session_id,
                        profile_id: params.profile_id,
                        policy_id: Some("mock-coding".into()),
                        tools: mock_tool_config_entries(),
                    }));
                Ok(())
            }
            AppUiCommand::SetToolConfigEnabled(params) => {
                self.queue
                    .push_back(tool_config_mutation_event(ToolConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        tool: Some(params.tool),
                        message: Some(if params.enabled {
                            "Mock tool config enabled".into()
                        } else {
                            "Mock tool config disabled".into()
                        }),
                        ..ToolConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::UpsertToolConfig(params) => {
                self.queue
                    .push_back(tool_config_mutation_event(ToolConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        tool: Some(params.tool),
                        message: Some("Mock tool config upserted".into()),
                        ..ToolConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::DeleteToolConfig(params) => {
                self.queue
                    .push_back(tool_config_mutation_event(ToolConfigMutationResult {
                        profile_id: params.profile_id,
                        ok: true,
                        applied: true,
                        tool: Some(params.tool),
                        message: Some("Mock tool config deleted".into()),
                        ..ToolConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::TestToolConfig(params) => {
                self.queue
                    .push_back(tool_config_mutation_event(ToolConfigMutationResult {
                        session_id: params.session_id,
                        profile_id: params.profile_id,
                        ok: true,
                        applied: false,
                        tool: Some(params.tool),
                        message: Some("Mock tool config test passed".into()),
                        ..ToolConfigMutationResult::default()
                    }));
                Ok(())
            }
            AppUiCommand::GetDiffPreview(params) => {
                self.queue
                    .push_back(ClientEvent::DiffPreview(DiffPreviewGetResult {
                        status: "ready".into(),
                        source: "mock approval fixture".into(),
                        preview: mock_diff_preview(params.session_id, params.preview_id),
                    }));
                Ok(())
            }
            AppUiCommand::ReadTaskOutput(_) => Err(eyre!(
                "mock app-ui backend does not implement task output reads yet"
            )),
            AppUiCommand::ReadTaskArtifact(_) => Err(eyre!(
                "mock app-ui backend does not implement task artifact reads yet"
            )),
            _ => Err(eyre!(
                "mock app-ui backend does not implement unsupported command {method} yet"
            )),
        }
    }

    fn next_event(&mut self) -> Result<Option<ClientEvent>> {
        Ok(self.queue.pop_front())
    }
}

fn mock_approval_kind() -> String {
    std::env::var("OCTOS_TUI_MOCK_APPROVAL_KIND").unwrap_or_else(|_| approval_kinds::COMMAND.into())
}

fn mock_model_status(selected: bool) -> ModelStatus {
    ModelStatus {
        model: "mock-coding".into(),
        provider: "mock".into(),
        title: Some("Mock Coding".into()),
        family: Some("mock".into()),
        route: None,
        selected,
        available: Some(true),
        queue_mode: Some("interactive".into()),
        qoe_policy: Some("mock".into()),
    }
}

fn mock_alt_model_status() -> ModelStatus {
    ModelStatus {
        model: "mock-review".into(),
        provider: "mock".into(),
        title: Some("Mock Review".into()),
        family: Some("mock".into()),
        route: Some("review".into()),
        selected: false,
        available: Some(true),
        queue_mode: Some("collect".into()),
        qoe_policy: Some("mock".into()),
    }
}

fn mock_profile_llm_catalog() -> ProfileLlmCatalogResult {
    let mut families = serde_json::Map::new();
    families.insert(
        "moonshot".into(),
        serde_json::json!({
            "env": "MOONSHOT_API_KEY",
            "models": [{
                "id": "kimi-k2.5",
                "endpoints": [
                    {"id": "moonshot", "label": "Official API"},
                    {
                        "id": "autodl",
                        "label": "AutoDL",
                        "base_url": "https://www.autodl.art/api/v1",
                        "api_key_env": "AUTODL_API_KEY"
                    }
                ]
            }]
        }),
    );
    families.insert(
        "minimax".into(),
        serde_json::json!({
            "env": "MINIMAX_API_KEY",
            "models": [{
                "id": "MiniMax-M2.5-highspeed",
                "endpoints": [{
                    "id": "wisemodel",
                    "label": "WiseModel",
                    "base_url": "https://open.ospreyai.cn/v1",
                    "api_key_env": "WISEMODEL_API_KEY"
                }]
            }]
        }),
    );
    ProfileLlmCatalogResult { families }
}

fn mock_profile_llm_list() -> ProfileLlmListResult {
    ProfileLlmListResult {
        profile_id: Some("coding".into()),
        primary: Some(crate::model::LlmConfiguredProvider {
            provider: "moonshot".into(),
            model: "kimi-k2.5".into(),
            family_id: Some("moonshot".into()),
            model_id: Some("kimi-k2.5".into()),
            route: None,
            route_id: Some("autodl".into()),
            base_url: Some("https://www.autodl.art/api/v1".into()),
            api_key_env: Some("AUTODL_API_KEY".into()),
            has_api_key: true,
            selected: true,
            available: Some(true),
            model_hints: None,
            cost_per_m: None,
            strong: None,
        }),
        fallbacks: Vec::new(),
        llm: None,
        runtime_policy_stamp: None,
    }
}

fn mock_profile_skills() -> ProfileSkillsListResult {
    ProfileSkillsListResult {
        profile_id: Some("coding".into()),
        count: 1,
        skills: vec![ProfileSkillEntry {
            name: "deep-search".into(),
            version: Some("0.1.0".into()),
            tool_count: 1,
            source_repo: Some("octos-org/octos-hub/skills/deep-search".into()),
            installed: true,
            status: Some("installed".into()),
        }],
    }
}

fn mock_skill_registry() -> ProfileSkillsRegistrySearchResult {
    ProfileSkillsRegistrySearchResult {
        profile_id: Some("coding".into()),
        packages: vec![ProfileSkillRegistryPackage {
            name: "deep-search".into(),
            description: "Mock registry package for deep research.".into(),
            repo: "octos-org/octos-hub/skills/deep-search".into(),
            version: Some("0.1.0".into()),
            author: Some("Octos".into()),
            license: Some("MIT".into()),
            skills: vec!["deep-search".into()],
            requires: Vec::new(),
            provides_tools: true,
            tags: vec!["research".into()],
            installed: true,
            installed_skills: vec!["deep-search".into()],
        }],
    }
}

fn mock_session_status(
    session_id: SessionKey,
    cwd: Option<String>,
    readonly: bool,
) -> SessionStatusReadResult {
    let sandbox = if readonly {
        "read-only"
    } else {
        "workspace-write"
    };
    SessionStatusReadResult {
        session_id,
        runtime_mode: Some("solo".into()),
        profile_id: Some("coding".into()),
        cwd: cwd.clone(),
        workspace_root: cwd,
        active_turn_id: None,
        runtime_policy_stamp: Some(RuntimePolicyStamp {
            runtime_mode: Some("solo".into()),
            profile_id: Some("coding".into()),
            model: Some("mock-coding".into()),
            provider: Some("mock".into()),
            approval_policy: Some(if readonly { "on-request" } else { "on-failure" }.into()),
            sandbox_mode: Some(sandbox.into()),
            sandbox: Some(sandbox.into()),
            permission_profile: Some(sandbox.into()),
            filesystem_scope: Some(if readonly { "read-only" } else { "workspace" }.into()),
            network: Some("blocked".into()),
            tool_policy_id: Some("mock-coding".into()),
            mcp_servers: vec![RuntimePolicyMcpServer::name("mock-filesystem")],
            memory_scope: Some("mock-session".into()),
            qoe_policy: Some("mock".into()),
            queue_mode: Some("interactive".into()),
            tool_contract_id: Some("codex-compatible-coding-v1".into()),
            tool_contract_version: Some("1".into()),
            model_toolset: Some("coding".into()),
            dynamic_tool_discovery: Some("enabled".into()),
        }),
        model: Some(mock_model_status(true)),
        permission_profile: Some(sandbox.into()),
        approval_policy: Some(if readonly { "on-request" } else { "on-failure" }.into()),
        sandbox_mode: Some(sandbox.into()),
        sandbox: Some(sandbox.into()),
        filesystem_scope: Some(if readonly { "read-only" } else { "workspace" }.into()),
        network: Some("blocked".into()),
        tool_policy_id: Some("mock-coding".into()),
        mcp_servers: vec!["mock-filesystem".into()],
        memory_scope: Some("mock-session".into()),
        health: Some(RuntimeHealthStatus {
            status: "healthy".into(),
            message: Some("mock backend".into()),
        }),
        mcp_summary: Some(McpStatusSummary {
            connected: 1,
            connecting: 0,
            failed: 1,
            disabled: 0,
        }),
        tool_summary: Some(ToolStatusSummary {
            visible: 2,
            enabled: 1,
            denied: 1,
            policy_id: Some("mock-coding".into()),
        }),
        usage: None,
        cursor: None,
        capabilities: Some(tui_capabilities()),
    }
}

fn mock_mcp_servers() -> Vec<McpStatus> {
    vec![
        McpStatus {
            server: "mock-filesystem".into(),
            status: "connected".into(),
            transport: Some("stdio".into()),
            endpoint: None,
            tool_count: Some(2),
            detail: Some("mock server".into()),
            last_error: None,
        },
        McpStatus {
            server: "mock-playwright".into(),
            status: "failed".into(),
            transport: Some("stdio".into()),
            endpoint: None,
            tool_count: Some(0),
            detail: None,
            last_error: Some("mock failure".into()),
        },
    ]
}

fn mock_mcp_config_entries() -> Vec<McpConfigEntry> {
    vec![
        McpConfigEntry {
            name: "mock-filesystem".into(),
            enabled: true,
            transport: Some("stdio".into()),
            command: Some("octos-mcp-filesystem".into()),
            args: vec!["/tmp".into()],
            env_keys: Vec::new(),
            status: Some("connected".into()),
            tool_count: Some(4),
            detail: Some("mock server truth".into()),
            ..McpConfigEntry::default()
        },
        McpConfigEntry {
            name: "mock-playwright".into(),
            enabled: false,
            transport: Some("stdio".into()),
            command: Some("octos-mcp-playwright".into()),
            status: Some("disabled".into()),
            last_error: Some("disabled by mock config".into()),
            ..McpConfigEntry::default()
        },
    ]
}

fn mock_tool_statuses() -> Vec<ToolStatus> {
    vec![
        ToolStatus {
            tool: "read_file".into(),
            title: Some("Read File".into()),
            source: Some("platform".into()),
            enabled: true,
            visible: true,
            tags: vec!["filesystem".into(), "read".into()],
            risk: Some("low".into()),
            policy_id: Some("mock-coding".into()),
            denial: None,
        },
        ToolStatus {
            tool: "shell".into(),
            title: Some("Shell".into()),
            source: Some("platform".into()),
            enabled: false,
            visible: true,
            tags: vec!["shell".into(), "write".into()],
            risk: Some("high".into()),
            policy_id: Some("mock-coding".into()),
            denial: Some(ToolPolicyDenial {
                code: "tool_denied".into(),
                tool: "shell".into(),
                policy: Some("mock-coding".into()),
                reason: "shell disabled in mock read-only policy".into(),
                recoverable: true,
            }),
        },
    ]
}

fn mock_tool_config_entries() -> Vec<ToolConfigEntry> {
    vec![
        ToolConfigEntry {
            tool: "search".into(),
            title: Some("Search".into()),
            source: Some("platform".into()),
            enabled: true,
            visible: true,
            tags: vec!["search".into()],
            risk: Some("low".into()),
            status: Some("ready".into()),
            detail: Some("mock server truth".into()),
        },
        ToolConfigEntry {
            tool: "web_fetch".into(),
            title: Some("Web Fetch".into()),
            source: Some("platform".into()),
            enabled: false,
            visible: true,
            tags: vec!["web".into()],
            risk: Some("medium".into()),
            status: Some("disabled".into()),
            detail: Some("disabled by mock config".into()),
        },
    ]
}

fn mock_approval_event(
    session_id: SessionKey,
    turn_id: TurnId,
    requested_kind: impl AsRef<str>,
) -> ApprovalRequestedEvent {
    let requested_kind = requested_kind.as_ref();
    let kind = match requested_kind {
        approval_kinds::DIFF => approval_kinds::DIFF,
        approval_kinds::FILESYSTEM => approval_kinds::FILESYSTEM,
        approval_kinds::NETWORK => approval_kinds::NETWORK,
        approval_kinds::SANDBOX_ESCALATION | "sandbox-escalation" => {
            approval_kinds::SANDBOX_ESCALATION
        }
        _ => approval_kinds::COMMAND,
    };

    let mut approval = ApprovalRequestedEvent::generic(
        session_id,
        octos_core::ui_protocol::ApprovalId::new(),
        turn_id,
        mock_approval_tool_name(kind),
        mock_approval_title(kind),
        mock_approval_body(kind),
    );
    approval.approval_kind = Some(kind.into());
    approval.risk = Some(mock_approval_risk(kind).into());
    approval.typed_details = Some(mock_approval_details(kind));
    approval
}

fn mock_approval_tool_name(kind: &str) -> &'static str {
    match kind {
        approval_kinds::DIFF => "diff_edit",
        approval_kinds::FILESYSTEM => "write_file",
        approval_kinds::NETWORK => "web_fetch",
        approval_kinds::SANDBOX_ESCALATION => "shell",
        _ => "shell",
    }
}

fn mock_approval_title(kind: &str) -> &'static str {
    match kind {
        approval_kinds::DIFF => "Mock diff approval boundary",
        approval_kinds::FILESYSTEM => "Mock filesystem approval boundary",
        approval_kinds::NETWORK => "Mock network approval boundary",
        approval_kinds::SANDBOX_ESCALATION => "Mock sandbox escalation boundary",
        _ => "Mock approval boundary",
    }
}

fn mock_approval_body(kind: &str) -> &'static str {
    match kind {
        approval_kinds::DIFF => "Review the structured diff preview before approving.",
        approval_kinds::FILESYSTEM => "The tool wants to write outside the workspace root.",
        approval_kinds::NETWORK => "The tool wants outbound network access.",
        approval_kinds::SANDBOX_ESCALATION => {
            "The tool wants to expand sandbox permissions for this command."
        }
        _ => "M9.14 pauses here with a typed approval surface.",
    }
}

fn mock_approval_risk(kind: &str) -> &'static str {
    match kind {
        approval_kinds::SANDBOX_ESCALATION => "high",
        approval_kinds::FILESYSTEM | approval_kinds::NETWORK => "medium",
        _ => "low",
    }
}

fn mock_approval_details(kind: &str) -> ApprovalTypedDetails {
    match kind {
        approval_kinds::DIFF => ApprovalTypedDetails {
            kind: approval_kinds::DIFF.into(),
            command: None,
            sandbox: None,
            diff: Some(ApprovalDiffDetails {
                preview_id: PreviewId::new(),
                operation: Some("apply".into()),
                file_count: Some(1),
                additions: Some(6),
                deletions: Some(2),
                summary: Some("Update the coding loop parser and tests".into()),
            }),
            filesystem: None,
            network: None,
            sandbox_escalation: None,
        },
        approval_kinds::FILESYSTEM => ApprovalTypedDetails {
            kind: approval_kinds::FILESYSTEM.into(),
            command: None,
            sandbox: None,
            diff: None,
            filesystem: Some(ApprovalFilesystemDetails {
                operation: "write".into(),
                paths: vec!["/tmp/octos-mock-approval.txt".into()],
                outside_workspace: true,
                writable_roots: vec!["/Users/yuechen/home/octos".into()],
            }),
            network: None,
            sandbox_escalation: None,
        },
        approval_kinds::NETWORK => ApprovalTypedDetails {
            kind: approval_kinds::NETWORK.into(),
            command: None,
            sandbox: None,
            diff: None,
            filesystem: None,
            network: Some(ApprovalNetworkDetails {
                operation: "fetch".into(),
                hosts: vec!["example.com".into()],
                ports: vec![443],
                urls: vec!["https://example.com".into()],
            }),
            sandbox_escalation: None,
        },
        approval_kinds::SANDBOX_ESCALATION => ApprovalTypedDetails {
            kind: approval_kinds::SANDBOX_ESCALATION.into(),
            command: None,
            sandbox: None,
            diff: None,
            filesystem: None,
            network: None,
            sandbox_escalation: Some(ApprovalSandboxEscalationDetails {
                from: Some(ApprovalSandboxEscalationEndpoint {
                    mode: Some("workspace-write".into()),
                    network_access: Some(false),
                }),
                to: Some(ApprovalSandboxEscalationEndpoint {
                    mode: Some("danger-full-access".into()),
                    network_access: Some(true),
                }),
                requested_permissions: vec!["network".into(), "write:/tmp".into()],
                justification: Some("probe a privileged command in the mock fixture".into()),
                suggested_prefix_rule: vec!["sudo".into(), "true".into()],
            }),
        },
        _ => ApprovalTypedDetails::command(
            ApprovalCommandDetails {
                argv: vec![
                    "cargo".into(),
                    "test".into(),
                    "-p".into(),
                    "octos-core".into(),
                ],
                command_line: Some("cargo test -p octos-core ui_protocol".into()),
                cwd: std::env::current_dir()
                    .ok()
                    .map(|path| path.display().to_string()),
                env_keys: vec!["RUST_BACKTRACE".into()],
                tool_call_id: Some("mock.shell.approval".into()),
            },
            Some(ApprovalSandboxDetails {
                mode: Some("workspace-write".into()),
                filesystem_access: Some("workspace-write".into()),
                network_access: Some(false),
                writable_roots: vec!["/Users/yuechen/home/octos".into()],
            }),
        ),
    }
}

fn mock_diff_preview(session_id: SessionKey, preview_id: PreviewId) -> DiffPreview {
    DiffPreview {
        session_id,
        preview_id,
        title: Some("Mock approval diff".into()),
        files: vec![DiffPreviewFile {
            path: "src/coding_loop.rs".into(),
            old_path: None,
            status: "modified".into(),
            hunks: vec![DiffPreviewHunk {
                header: "@@ -1,3 +1,5 @@".into(),
                lines: vec![
                    DiffPreviewLine {
                        kind: "context".into(),
                        content: "pub fn parse(input: &str) -> Plan {".into(),
                        old_line: Some(1),
                        new_line: Some(1),
                    },
                    DiffPreviewLine {
                        kind: "removed".into(),
                        content: "    Plan::default()".into(),
                        old_line: Some(2),
                        new_line: None,
                    },
                    DiffPreviewLine {
                        kind: "added".into(),
                        content: "    Plan::from_markdown(input)".into(),
                        old_line: None,
                        new_line: Some(2),
                    },
                ],
            }],
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AgentArtifactReadParams, ConfigCapabilitiesListParams, McpConfigDeleteParams,
        McpConfigListParams, McpConfigSetEnabledParams, McpConfigTestParams, McpConfigUpsertParams,
        McpStatusListParams, ModelListParams, ModelSelectParams, ProfileLocalCreateParams,
        ProfileSkillsInstallParams, ProfileSkillsListParams, ProfileSkillsRegistrySearchParams,
        ProfileSkillsRemoveParams, SessionStatusReadParams, ToolConfigDeleteParams,
        ToolConfigListParams, ToolConfigSetEnabledParams, ToolConfigTestParams,
        ToolConfigUpsertParams, ToolStatusListParams,
    };
    use octos_core::TaskId;
    use octos_core::ui_protocol::{
        ApprovalDecision, ApprovalRespondParams, ApprovalScopesListParams, DiffPreviewGetParams,
        InputItem, PermissionNetworkPolicy, PermissionProfileListParams, PermissionProfileMode,
        PermissionProfileSetParams, PermissionProfileUpdate, PreviewId, SessionOpenParams,
        TaskArtifactReadParams, TaskCancelParams, TaskListParams, TaskOutputReadParams,
        TaskRestartFromNodeParams, TurnInterruptParams, TurnStartParams,
    };
    use serde_json::json;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    fn unwrap_app_event(event: ClientEvent) -> AppUiEvent {
        let ClientEvent::App(event) = event else {
            panic!("expected app event");
        };
        *event
    }

    struct ProtocolCaptureServer {
        endpoint: String,
        received: mpsc::Receiver<Value>,
        thread: thread::JoinHandle<()>,
    }

    impl ProtocolCaptureServer {
        fn recv_json(&self) -> Value {
            self.received
                .recv_timeout(Duration::from_secs(2))
                .expect("protocol server captured request")
        }

        fn join(self) {
            self.thread.join().expect("protocol server exits cleanly");
        }
    }

    fn spawn_protocol_capture_server(
        expected_requests: usize,
        respond_to_session_open: bool,
    ) -> ProtocolCaptureServer {
        let (addr_tx, addr_rx) = mpsc::channel();
        let (frame_tx, frame_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let runtime = Runtime::new().expect("test protocol server runtime");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind protocol test server");
                let addr = listener.local_addr().expect("protocol test server addr");
                addr_tx.send(addr).expect("send protocol test server addr");

                let (stream, _) = listener
                    .accept()
                    .await
                    .expect("accept protocol test connection");
                let mut ws = tokio_tungstenite::accept_async(stream)
                    .await
                    .expect("accept protocol test websocket");

                for _ in 0..expected_requests {
                    let Some(message) = ws.next().await else {
                        break;
                    };
                    let message = message.expect("read protocol test websocket message");
                    let text = match message {
                        WsMessage::Text(text) => text.to_string(),
                        WsMessage::Binary(bytes) => {
                            String::from_utf8(bytes.to_vec()).expect("binary request is UTF-8")
                        }
                        _ => continue,
                    };
                    let frame: Value =
                        serde_json::from_str(&text).expect("request is JSON-RPC object");
                    frame_tx
                        .send(frame.clone())
                        .expect("capture protocol test request");

                    let method = frame.get("method").and_then(Value::as_str);
                    let response =
                        if method == Some(crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST) {
                            Some(json!({
                                "jsonrpc": "2.0",
                                "id": frame.get("id").cloned().expect("request id"),
                                "result": {
                                    "capabilities": tui_capabilities()
                                }
                            }))
                        } else if respond_to_session_open && method == Some(methods::SESSION_OPEN) {
                            Some(json!({
                                "jsonrpc": "2.0",
                                "id": frame.get("id").cloned().expect("request id"),
                                "result": {
                                    "opened": {
                                        "session_id": frame["params"]["session_id"].clone(),
                                        "active_profile_id": "coding",
                                        "workspace_root": "/repo",
                                        "cursor": {
                                            "stream": "session_events",
                                            "seq": 1
                                        }
                                    }
                                }
                            }))
                        } else {
                            None
                        };
                    if let Some(response) = response {
                        ws.send(WsMessage::Text(response.to_string().into()))
                            .await
                            .expect("send protocol test response");
                    }
                }
            });
        });
        let addr = addr_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("protocol test server starts");

        ProtocolCaptureServer {
            endpoint: format!("ws://{addr}/ui-protocol"),
            received: frame_rx,
            thread,
        }
    }

    fn next_event_until(backend: &mut ProtocolAppUiBackend) -> ClientEvent {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = backend.next_event().expect("poll protocol backend") {
                return event;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for protocol event"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn protocol_command_serializes_json_rpc_without_command_kind() {
        let request = rpc_request_from_command(
            "tui-7".into(),
            AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: SessionKey("local:test".into()),
                turn_id: TurnId::new(),
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }),
        )
        .expect("request encodes");

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, "tui-7");
        assert_eq!(request.method, methods::TURN_START);
        assert_eq!(request.params["session_id"], "local:test");
        assert!(request.params.get("kind").is_none());
        assert_eq!(request.params["input"][0]["kind"], "text");
        assert_eq!(request.params["input"][0]["text"], "hello");
    }

    #[test]
    fn protocol_turn_start_request_preserves_submitted_prompt_text() {
        let request = rpc_request_from_command(
            "tui-9".into(),
            AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: SessionKey("local:test".into()),
                turn_id: TurnId::new(),
                input: vec![
                    InputItem::Text {
                        text: "complete m9 contract".into(),
                    },
                    InputItem::Text {
                        text: "second paragraph".into(),
                    },
                ],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }),
        )
        .expect("request encodes");

        assert_eq!(request.method, methods::TURN_START);
        assert_eq!(request.params["session_id"], "local:test");
        assert_eq!(request.params["input"][0]["kind"], "text");
        assert_eq!(request.params["input"][0]["text"], "complete m9 contract");
        assert_eq!(request.params["input"][1]["kind"], "text");
        assert_eq!(request.params["input"][1]["text"], "second paragraph");
    }

    #[test]
    fn protocol_command_serializes_approval_scopes_list() {
        let request = rpc_request_from_command(
            "tui-8".into(),
            AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
                session_id: SessionKey("local:test".into()),
            }),
        )
        .expect("request encodes");

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, "tui-8");
        assert_eq!(request.method, methods::APPROVAL_SCOPES_LIST);
        assert_eq!(request.params["session_id"], "local:test");
        assert!(request.params.get("kind").is_none());
    }

    #[test]
    fn protocol_command_serializes_agent_artifact_read() {
        let request = rpc_request_from_command(
            "tui-10".into(),
            AppUiCommand::ReadAgentArtifact(AgentArtifactReadParams {
                session_id: SessionKey("local:test".into()),
                agent_id: "ag-7".into(),
                artifact_id: Some("artifact-1".into()),
                path: None,
            }),
        )
        .expect("request encodes");

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, methods::AGENT_ARTIFACT_READ);
        assert_eq!(request.params["session_id"], "local:test");
        assert_eq!(request.params["agent_id"], "ag-7");
        assert_eq!(request.params["artifact_id"], "artifact-1");
        assert!(request.params.get("path").is_none());
    }

    #[test]
    fn protocol_decodes_agent_artifact_read_result() {
        let mut exchange = ProtocolExchange::default();
        let request = exchange
            .build_tracked_request(AppUiCommand::ReadAgentArtifact(AgentArtifactReadParams {
                session_id: SessionKey("local:test".into()),
                agent_id: "ag-7".into(),
                artifact_id: Some("artifact-1".into()),
                path: None,
            }))
            .expect("tracked request");
        let response = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "session_id": "local:test",
                "agent_id": "ag-7",
                "artifact": {
                    "id": "artifact-1",
                    "title": "notes.md",
                    "kind": "markdown",
                    "status": "ready"
                },
                "content": "artifact body"
            }
        });

        let event = exchange
            .decode_rpc_text(&response.to_string())
            .expect("response decodes")
            .expect("event");

        let ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::AgentArtifactRead(result),
        }) = event
        else {
            panic!("expected agent artifact read event");
        };
        assert_eq!(result.agent_id, "ag-7");
        assert_eq!(result.artifact.id, "artifact-1");
        assert_eq!(result.content.as_deref(), Some("artifact body"));
    }

    #[test]
    fn protocol_command_serializes_task_artifact_read() {
        let task_id = TaskId::default();
        let request = rpc_request_from_command(
            "tui-11".into(),
            AppUiCommand::ReadTaskArtifact(TaskArtifactReadParams {
                session_id: SessionKey("local:test".into()),
                task_id: task_id.clone(),
                artifact_id: Some("summary".into()),
                path: None,
                cursor: None,
                limit_bytes: Some(4096),
                profile_id: Some("coding".into()),
                agent_id: None,
            }),
        )
        .expect("request encodes");

        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.method, methods::TASK_ARTIFACT_READ);
        assert_eq!(request.params["session_id"], "local:test");
        assert_eq!(request.params["task_id"], serde_json::json!(task_id));
        assert_eq!(request.params["artifact_id"], "summary");
        assert_eq!(request.params["profile_id"], "coding");
        assert!(request.params.get("path").is_none());
    }

    #[test]
    fn protocol_decodes_task_artifact_read_result() {
        let mut exchange = ProtocolExchange::default();
        let task_id = TaskId::default();
        let request = exchange
            .build_tracked_request(AppUiCommand::ReadTaskArtifact(TaskArtifactReadParams {
                session_id: SessionKey("local:test".into()),
                task_id: task_id.clone(),
                artifact_id: Some("summary".into()),
                path: None,
                cursor: None,
                limit_bytes: Some(4096),
                profile_id: None,
                agent_id: None,
            }))
            .expect("tracked request");
        let response = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "session_id": "local:test",
                "task_id": task_id,
                "artifact": {
                    "id": "summary",
                    "title": "Summary",
                    "kind": "markdown",
                    "status": "ready"
                },
                "content": "task artifact body",
                "has_more": false
            }
        });

        let event = exchange
            .decode_rpc_text(&response.to_string())
            .expect("response decodes")
            .expect("event");

        let ClientEvent::Autonomy(AutonomyClientEvent {
            result: AutonomyResult::TaskArtifactRead(result),
        }) = event
        else {
            panic!("expected task artifact read event");
        };
        assert_eq!(result.task_id, task_id);
        assert_eq!(result.artifact.id, "summary");
        assert_eq!(result.content.as_deref(), Some("task artifact body"));
    }

    #[test]
    fn protocol_command_serializes_permission_profile_commands() {
        let session_id = SessionKey("local:test".into());
        let list = rpc_request_from_command(
            "tui-9".into(),
            AppUiCommand::ListPermissionProfiles(PermissionProfileListParams {
                session_id: session_id.clone(),
            }),
        )
        .expect("list request encodes");
        assert_eq!(list.method, methods::PERMISSION_PROFILE_LIST);
        assert_eq!(list.params["session_id"], "local:test");

        let set = rpc_request_from_command(
            "tui-10".into(),
            AppUiCommand::SetPermissionProfile(PermissionProfileSetParams {
                session_id,
                update: PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Allow),
                    approval_policy: None,
                },
                runtime_mode: None,
            }),
        )
        .expect("set request encodes");
        assert_eq!(set.method, methods::PERMISSION_PROFILE_SET);
        assert_eq!(set.params["update"]["network"], "allow");
        assert!(set.params["update"].get("mode").is_none());
    }

    #[test]
    fn protocol_command_serializes_runtime_cockpit_commands() {
        let session_id = SessionKey("local:test".into());
        let capabilities = rpc_request_from_command(
            "tui-11".into(),
            AppUiCommand::ListConfigCapabilities(ConfigCapabilitiesListParams {}),
        )
        .expect("capabilities request encodes");
        assert_eq!(
            capabilities.method,
            crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST
        );
        assert!(
            capabilities
                .params
                .as_object()
                .is_some_and(|object| object.is_empty())
        );

        let local_profile = rpc_request_from_command(
            "tui-11b".into(),
            AppUiCommand::ProfileLocalCreate(ProfileLocalCreateParams {
                name: "Ada Lovelace".into(),
                username: "ada".into(),
                email: "ada@example.com".into(),
            }),
        )
        .expect("local profile request encodes");
        assert_eq!(
            local_profile.method,
            crate::model::APPUI_METHOD_PROFILE_LOCAL_CREATE
        );
        assert_eq!(local_profile.params["name"], "Ada Lovelace");
        assert_eq!(local_profile.params["username"], "ada");
        assert_eq!(local_profile.params["email"], "ada@example.com");

        let status = rpc_request_from_command(
            "tui-12".into(),
            AppUiCommand::ReadSessionStatus(SessionStatusReadParams {
                session_id: session_id.clone(),
            }),
        )
        .expect("status request encodes");
        assert_eq!(
            status.method,
            crate::model::APPUI_METHOD_SESSION_STATUS_READ
        );
        assert_eq!(status.params["session_id"], "local:test");

        let models = rpc_request_from_command(
            "tui-13".into(),
            AppUiCommand::ListModels(ModelListParams {
                session_id: session_id.clone(),
            }),
        )
        .expect("model list request encodes");
        assert_eq!(models.method, crate::model::APPUI_METHOD_MODEL_LIST);

        let select = rpc_request_from_command(
            "tui-14".into(),
            AppUiCommand::SelectModel(ModelSelectParams {
                session_id: session_id.clone(),
                model: "deepseek-v4-pro".into(),
                provider: Some("deepseek".into()),
                route: None,
            }),
        )
        .expect("model select request encodes");
        assert_eq!(select.method, crate::model::APPUI_METHOD_MODEL_SELECT);
        assert_eq!(select.params["model"], "deepseek-v4-pro");
        assert_eq!(select.params["provider"], "deepseek");

        let mcp = rpc_request_from_command(
            "tui-15".into(),
            AppUiCommand::ListMcpStatus(McpStatusListParams {
                session_id: session_id.clone(),
                include_disabled: true,
            }),
        )
        .expect("mcp status request encodes");
        assert_eq!(mcp.method, crate::model::APPUI_METHOD_MCP_STATUS_LIST);
        assert_eq!(mcp.params["include_disabled"], true);

        let tools = rpc_request_from_command(
            "tui-16".into(),
            AppUiCommand::ListToolStatus(ToolStatusListParams {
                session_id,
                include_denied: true,
            }),
        )
        .expect("tool status request encodes");
        assert_eq!(tools.method, crate::model::APPUI_METHOD_TOOL_STATUS_LIST);
        assert_eq!(tools.params["include_denied"], true);
    }

    #[test]
    fn protocol_task_control_commands_reach_the_wire() {
        let session_id = SessionKey("local:test".into());
        let task_id = TaskId::new();

        let list = rpc_request_from_command(
            "task-list-1".into(),
            AppUiCommand::ListTasks(TaskListParams {
                session_id: session_id.clone(),
                topic: Some("coding".into()),
            }),
        )
        .expect("task list request encodes");
        assert_eq!(list.method, methods::TASK_LIST);
        assert_eq!(list.params["session_id"], "local:test");
        assert_eq!(list.params["topic"], "coding");

        let cancel = rpc_request_from_command(
            "task-cancel-1".into(),
            AppUiCommand::CancelTask(TaskCancelParams {
                task_id: task_id.clone(),
                session_id: Some(session_id.clone()),
                profile_id: Some("profile-a".into()),
            }),
        )
        .expect("task cancel request encodes");
        assert_eq!(cancel.method, methods::TASK_CANCEL);
        assert_eq!(cancel.params["task_id"], task_id.0.to_string());
        assert_eq!(cancel.params["session_id"], "local:test");
        assert_eq!(cancel.params["profile_id"], "profile-a");

        let restart = rpc_request_from_command(
            "task-restart-1".into(),
            AppUiCommand::RestartTaskFromNode(TaskRestartFromNodeParams {
                task_id: task_id.clone(),
                node_id: Some("synthesize".into()),
                session_id: Some(session_id),
                profile_id: Some("profile-a".into()),
            }),
        )
        .expect("task restart request encodes");
        assert_eq!(restart.method, methods::TASK_RESTART_FROM_NODE);
        assert_eq!(restart.params["task_id"], task_id.0.to_string());
        assert_eq!(restart.params["node_id"], "synthesize");
        assert_eq!(restart.params["session_id"], "local:test");
        assert_eq!(restart.params["profile_id"], "profile-a");
    }

    #[test]
    fn protocol_command_serializes_mcp_and_tool_config_commands() {
        let mcp_list = rpc_request_from_command(
            "mcp-config-1".into(),
            AppUiCommand::ListMcpConfig(McpConfigListParams {
                session_id: Some(SessionKey("local:test".into())),
                profile_id: Some("coding".into()),
                include_disabled: true,
            }),
        )
        .expect("mcp config list encodes");
        assert_eq!(mcp_list.method, crate::model::APPUI_METHOD_MCP_CONFIG_LIST);
        assert_eq!(mcp_list.params["session_id"], "local:test");
        assert_eq!(mcp_list.params["profile_id"], "coding");
        assert_eq!(mcp_list.params["include_disabled"], true);

        let mcp_upsert = rpc_request_from_command(
            "mcp-config-2".into(),
            AppUiCommand::UpsertMcpConfig(McpConfigUpsertParams {
                profile_id: Some("coding".into()),
                server: "github".into(),
                config: json!({"transport": "stdio"}),
                enabled: Some(true),
            }),
        )
        .expect("mcp config upsert encodes");
        assert_eq!(
            mcp_upsert.method,
            crate::model::APPUI_METHOD_MCP_CONFIG_UPSERT
        );
        assert_eq!(mcp_upsert.params["server"], "github");
        assert_eq!(mcp_upsert.params["config"]["transport"], "stdio");

        let mcp_delete = rpc_request_from_command(
            "mcp-config-3".into(),
            AppUiCommand::DeleteMcpConfig(McpConfigDeleteParams {
                profile_id: Some("coding".into()),
                server: "github".into(),
            }),
        )
        .expect("mcp config delete encodes");
        assert_eq!(
            mcp_delete.method,
            crate::model::APPUI_METHOD_MCP_CONFIG_DELETE
        );

        let mcp_toggle = rpc_request_from_command(
            "mcp-config-4".into(),
            AppUiCommand::SetMcpConfigEnabled(McpConfigSetEnabledParams {
                profile_id: Some("coding".into()),
                server: "github".into(),
                enabled: false,
            }),
        )
        .expect("mcp config toggle encodes");
        assert_eq!(
            mcp_toggle.method,
            crate::model::APPUI_METHOD_MCP_CONFIG_SET_ENABLED
        );
        assert_eq!(mcp_toggle.params["enabled"], false);

        let mcp_test = rpc_request_from_command(
            "mcp-config-5".into(),
            AppUiCommand::TestMcpConfig(McpConfigTestParams {
                session_id: Some(SessionKey("local:test".into())),
                profile_id: Some("coding".into()),
                server: "github".into(),
            }),
        )
        .expect("mcp config test encodes");
        assert_eq!(mcp_test.method, crate::model::APPUI_METHOD_MCP_CONFIG_TEST);

        let tool_list = rpc_request_from_command(
            "tool-config-1".into(),
            AppUiCommand::ListToolConfig(ToolConfigListParams {
                session_id: Some(SessionKey("local:test".into())),
                profile_id: Some("coding".into()),
                include_disabled: true,
            }),
        )
        .expect("tool config list encodes");
        assert_eq!(
            tool_list.method,
            crate::model::APPUI_METHOD_TOOL_CONFIG_LIST
        );

        let tool_toggle = rpc_request_from_command(
            "tool-config-2".into(),
            AppUiCommand::SetToolConfigEnabled(ToolConfigSetEnabledParams {
                profile_id: Some("coding".into()),
                tool: "web_fetch".into(),
                enabled: true,
            }),
        )
        .expect("tool config toggle encodes");
        assert_eq!(
            tool_toggle.method,
            crate::model::APPUI_METHOD_TOOL_CONFIG_SET_ENABLED
        );

        let tool_upsert = rpc_request_from_command(
            "tool-config-3".into(),
            AppUiCommand::UpsertToolConfig(ToolConfigUpsertParams {
                profile_id: Some("coding".into()),
                tool: "browser".into(),
                config: json!({"mode": "restricted"}),
                enabled: None,
            }),
        )
        .expect("tool config upsert encodes");
        assert_eq!(
            tool_upsert.method,
            crate::model::APPUI_METHOD_TOOL_CONFIG_UPSERT
        );
        assert_eq!(tool_upsert.params["config"]["mode"], "restricted");

        let tool_delete = rpc_request_from_command(
            "tool-config-4".into(),
            AppUiCommand::DeleteToolConfig(ToolConfigDeleteParams {
                profile_id: Some("coding".into()),
                tool: "browser".into(),
            }),
        )
        .expect("tool config delete encodes");
        assert_eq!(
            tool_delete.method,
            crate::model::APPUI_METHOD_TOOL_CONFIG_DELETE
        );

        let tool_test = rpc_request_from_command(
            "tool-config-5".into(),
            AppUiCommand::TestToolConfig(ToolConfigTestParams {
                session_id: Some(SessionKey("local:test".into())),
                profile_id: Some("coding".into()),
                tool: "browser".into(),
            }),
        )
        .expect("tool config test encodes");
        assert_eq!(
            tool_test.method,
            crate::model::APPUI_METHOD_TOOL_CONFIG_TEST
        );
    }

    #[test]
    fn protocol_command_serializes_profile_skill_commands() {
        let list = rpc_request_from_command(
            "skills-1".into(),
            AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
                profile_id: Some("coding".into()),
            }),
        )
        .expect("list request encodes");
        assert_eq!(list.method, crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST);
        assert_eq!(list.params["profile_id"], "coding");

        let search = rpc_request_from_command(
            "skills-2".into(),
            AppUiCommand::ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchParams {
                profile_id: Some("coding".into()),
                q: Some("research".into()),
            }),
        )
        .expect("search request encodes");
        assert_eq!(
            search.method,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_REGISTRY_SEARCH
        );
        assert_eq!(search.params["q"], "research");

        let install = rpc_request_from_command(
            "skills-3".into(),
            AppUiCommand::ProfileSkillsInstall(ProfileSkillsInstallParams {
                profile_id: Some("coding".into()),
                repo: "octos-org/octos-hub/skills/deep-search".into(),
                branch: Some("main".into()),
                force: true,
            }),
        )
        .expect("install request encodes");
        assert_eq!(
            install.method,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_INSTALL
        );
        assert_eq!(
            install.params["repo"],
            "octos-org/octos-hub/skills/deep-search"
        );
        assert_eq!(install.params["branch"], "main");
        assert_eq!(install.params["force"], true);

        let remove = rpc_request_from_command(
            "skills-4".into(),
            AppUiCommand::ProfileSkillsRemove(ProfileSkillsRemoveParams {
                profile_id: Some("coding".into()),
                name: "deep-search".into(),
            }),
        )
        .expect("remove request encodes");
        assert_eq!(
            remove.method,
            crate::model::APPUI_METHOD_PROFILE_SKILLS_REMOVE
        );
        assert_eq!(remove.params["name"], "deep-search");
    }

    #[test]
    fn profile_skill_results_decode_to_client_events() {
        let mut pending = HashMap::new();
        pending.insert(
            "skills-list".into(),
            PendingRequest {
                method: crate::model::APPUI_METHOD_PROFILE_SKILLS_LIST.into(),
            },
        );
        let frame = json!({
            "jsonrpc": "2.0",
            "id": "skills-list",
            "result": {
                "profile_id": "coding",
                "count": 1,
                "skills": [{
                    "name": "deep-search",
                    "version": "0.1.0",
                    "tool_count": 1,
                    "installed": true,
                    "status": "installed"
                }]
            }
        })
        .to_string();

        let event = rpc_text_to_app_event_with_pending(&frame, &mut pending)
            .expect("frame decodes")
            .expect("client event");
        let ClientEvent::ProfileSkillsList(event) = event else {
            panic!("expected profile skills list event");
        };
        assert_eq!(event.result.profile_id.as_deref(), Some("coding"));
        assert_eq!(event.result.skills[0].name, "deep-search");
        assert_eq!(event.result.skills[0].status.as_deref(), Some("installed"));
    }

    #[test]
    fn protocol_readonly_policy_allows_read_style_and_blocks_mutations() {
        let session_id = SessionKey("local:test".into());
        let read_style_commands = [
            AppUiCommand::ListConfigCapabilities(ConfigCapabilitiesListParams {}),
            AppUiCommand::OpenSession(SessionOpenParams {
                session_id: session_id.clone(),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: Some("/repo".into()),
                after: None,
            }),
            AppUiCommand::ReadSessionStatus(SessionStatusReadParams {
                session_id: session_id.clone(),
            }),
            AppUiCommand::ListModels(ModelListParams {
                session_id: session_id.clone(),
            }),
            AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
                session_id: session_id.clone(),
            }),
            AppUiCommand::ListPermissionProfiles(PermissionProfileListParams {
                session_id: session_id.clone(),
            }),
            AppUiCommand::ListMcpStatus(McpStatusListParams {
                session_id: session_id.clone(),
                include_disabled: true,
            }),
            AppUiCommand::ListToolStatus(ToolStatusListParams {
                session_id: session_id.clone(),
                include_denied: true,
            }),
            AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                session_id: session_id.clone(),
                preview_id: PreviewId::new(),
            }),
            AppUiCommand::ReadTaskOutput(TaskOutputReadParams {
                session_id: session_id.clone(),
                task_id: TaskId::new(),
                cursor: Some(OutputCursor { offset: 0 }),
                limit_bytes: Some(4096),
            }),
            AppUiCommand::ProfileSkillsList(ProfileSkillsListParams {
                profile_id: Some("coding".into()),
            }),
            AppUiCommand::ProfileSkillsRegistrySearch(ProfileSkillsRegistrySearchParams {
                profile_id: Some("coding".into()),
                q: Some("search".into()),
            }),
        ];
        for command in &read_style_commands {
            assert!(
                ProtocolAppUiBackend::readonly_allows_command(command),
                "{} should stay available in read-only mode",
                command.method()
            );
        }

        let mutating_commands = [
            AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: session_id.clone(),
                turn_id: TurnId::new(),
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }),
            AppUiCommand::InterruptTurn(TurnInterruptParams {
                session_id: session_id.clone(),
                turn_id: TurnId::new(),
            }),
            AppUiCommand::SelectModel(ModelSelectParams {
                session_id: session_id.clone(),
                model: "deepseek-v4-pro".into(),
                provider: Some("deepseek".into()),
                route: None,
            }),
            AppUiCommand::RespondApproval(ApprovalRespondParams::new(
                session_id.clone(),
                octos_core::ui_protocol::ApprovalId::new(),
                ApprovalDecision::Deny,
            )),
            AppUiCommand::ProfileLocalCreate(ProfileLocalCreateParams {
                name: "Ada Lovelace".into(),
                username: "ada".into(),
                email: "ada@example.com".into(),
            }),
            AppUiCommand::SetPermissionProfile(PermissionProfileSetParams {
                session_id,
                update: PermissionProfileUpdate {
                    mode: None,
                    network: Some(PermissionNetworkPolicy::Allow),
                    approval_policy: None,
                },
                runtime_mode: None,
            }),
            AppUiCommand::ProfileSkillsInstall(ProfileSkillsInstallParams {
                profile_id: Some("coding".into()),
                repo: "octos-org/octos-hub/skills/deep-search".into(),
                branch: None,
                force: false,
            }),
            AppUiCommand::ProfileSkillsRemove(ProfileSkillsRemoveParams {
                profile_id: Some("coding".into()),
                name: "deep-search".into(),
            }),
        ];
        for command in &mutating_commands {
            assert!(
                !ProtocolAppUiBackend::readonly_allows_command(command),
                "{} should be blocked in read-only mode",
                command.method()
            );
        }
    }

    #[test]
    fn protocol_notification_maps_to_app_event() {
        let turn_id = TurnId::new();
        let frame = json!({
            "jsonrpc": "2.0",
            "method": methods::MESSAGE_DELTA,
            "params": {
                "session_id": "local:test",
                "turn_id": turn_id,
                "text": "hello"
            }
        })
        .to_string();

        let event = rpc_text_to_app_event(&frame)
            .expect("frame decodes")
            .expect("notification yields event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Protocol(UiNotification::MessageDelta(event)) = event else {
            panic!("expected message delta notification");
        };
        assert_eq!(event.session_id.0, "local:test");
        assert_eq!(event.turn_id, turn_id);
        assert_eq!(event.text, "hello");
    }

    #[test]
    fn progress_notification_maps_to_app_progress_event() {
        let frame = json!({
            "jsonrpc": "2.0",
            "method": methods::PROGRESS_UPDATED,
            "params": {
                "session_id": "local:test",
                "turn_id": null,
                "metadata": {
                    "kind": octos_core::ui_protocol::progress_kinds::STATUS,
                    "message": "indexing workspace"
                }
            }
        })
        .to_string();

        let event = rpc_text_to_app_event(&frame)
            .expect("frame decodes")
            .expect("notification yields event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Progress(progress) = event else {
            panic!("expected progress event");
        };
        assert_eq!(progress.session_id.0, "local:test");
        assert_eq!(
            progress.metadata.message.as_deref(),
            Some("indexing workspace")
        );
    }

    #[test]
    fn server_heartbeat_notification_is_ignored() {
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "server/heartbeat",
            "params": {
                "timestamp": "2026-05-13T17:17:00Z"
            }
        })
        .to_string();

        let event = rpc_text_to_app_event(&frame).expect("frame decodes");
        assert!(event.is_none());
    }

    #[test]
    fn websocket_request_includes_bearer_auth_header() {
        let request = websocket_request(
            "wss://example.test/ui-protocol",
            Some(" secret-token "),
            Some(" coding "),
        )
        .expect("request builds");
        let expected_features = format!(
            "{UI_PROTOCOL_FEATURE_APPROVAL_TYPED_V1}, {UI_PROTOCOL_FEATURE_PANE_SNAPSHOTS_V1}, {UI_PROTOCOL_FEATURE_SESSION_WORKSPACE_CWD_V1}, {UI_PROTOCOL_FEATURE_CODING_AUTONOMY_V1}, {UI_PROTOCOL_FEATURE_CODING_AGENT_CONTROL_V1}, {UI_PROTOCOL_FEATURE_CODING_GOAL_RUNTIME_V1}, {UI_PROTOCOL_FEATURE_CODING_LOOP_RUNTIME_V1}, {UI_PROTOCOL_FEATURE_HARNESS_TASK_CONTROL_V1}"
        );

        assert_eq!(
            request
                .headers()
                .get("Authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer secret-token")
        );
        assert_eq!(
            request
                .headers()
                .get("X-Octos-Ui-Features")
                .and_then(|value| value.to_str().ok()),
            Some(expected_features.as_str())
        );
        assert_eq!(
            request
                .headers()
                .get("X-Profile-Id")
                .and_then(|value| value.to_str().ok()),
            Some("coding")
        );
    }

    #[test]
    fn stdio_transport_driver_rejects_empty_command() {
        let err = match StdioTransportDriver::new("   ".into()) {
            Ok(_) => panic!("empty command should be rejected"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("UI protocol stdio command must not be empty")
        );
    }

    #[test]
    fn stdio_transport_driver_shape_is_line_oriented_text() {
        let driver =
            StdioTransportDriver::new("octos serve --stdio".into()).expect("driver builds");

        assert_eq!(driver.label(), "octos serve --stdio");
        assert!(!driver.is_connected());
    }

    #[test]
    fn stdio_transport_rejects_oversized_text_frame_before_decode() {
        let event = stdio_text_frame_event("x".repeat(MAX_TEXT_FRAME_BYTES + 1));

        let TransportEvent::Error {
            code,
            message,
            disconnect,
        } = event
        else {
            panic!("expected oversized stdio frame error");
        };
        assert_eq!(code, "frame_too_large");
        assert!(message.contains("UI protocol stdio frame"));
        assert!(disconnect);
    }

    #[test]
    fn launch_from_cli_uses_stdio_endpoint_when_requested() {
        let cli = Cli {
            config: None,
            mode: crate::cli::Mode::Protocol,
            base_url: None,
            stdio_command: Some("octos serve --stdio".into()),
            session: None,
            profile_id: None,
            cwd: None,
            auth_token: Some("ignored-for-stdio".into()),
            readonly: false,
            theme: crate::cli::ThemeName::Codex,
        };

        let launch = launch_from_cli(&cli);

        assert_eq!(
            launch
                .endpoint
                .as_ref()
                .map(|endpoint| endpoint.label().to_string()),
            Some("octos serve --stdio".into())
        );
    }

    #[test]
    fn protocol_success_response_is_ack_only() {
        let event = rpc_text_to_app_event(r#"{"jsonrpc":"2.0","id":"tui-1","result":{}}"#)
            .expect("response decodes");

        assert!(event.is_none());
    }

    #[test]
    fn protocol_error_response_maps_to_app_error() {
        let event = rpc_text_to_app_event(
            r#"{"jsonrpc":"2.0","id":"tui-1","error":{"code":"unknown_session","message":"missing session"}}"#,
        )
        .expect("response decodes")
        .expect("error yields event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Error(error) = event else {
            panic!("expected app error");
        };
        assert_eq!(error.code, "unknown_session");
        assert_eq!(error.message, "request tui-1 failed: missing session");
    }

    /// M12-F structured policy error: when the server attaches
    /// `data.kind` + `data.message` to a JSON-RPC error response, the
    /// TUI must surface those rather than the numeric JSON-RPC `code`
    /// + the top-level `message`. Otherwise tenant/cloud rejection
    /// renders as a generic "application_error" line, which violates
    /// the M12-F acceptance bar.
    #[test]
    fn rpc_error_prefers_structured_data_kind_over_numeric_code() {
        let event = rpc_text_to_app_event(
            r#"{"jsonrpc":"2.0","id":"tui-set-perms","error":{"code":-32603,"message":"application error","data":{"kind":"policy_rejected","message":"tenant policy forbids danger-full-access"}}}"#,
        )
        .expect("response decodes")
        .expect("error yields event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected app error");
        };
        // `data.kind` wins for both renderer code (so "policy_rejected"
        // shows up structured) and message (so the tenant-specific
        // detail surfaces).
        assert_eq!(error.code, "policy_rejected");
        assert!(
            error.message.contains("tenant policy forbids"),
            "structured detail must reach the user: {}",
            error.message
        );
    }

    /// Guard: when `data` is absent the legacy fallback still kicks
    /// in. Existing AppUI callers that do not emit structured errors
    /// keep working.
    #[test]
    fn rpc_error_falls_back_to_top_level_code_when_data_kind_absent() {
        let event = rpc_text_to_app_event(
            r#"{"jsonrpc":"2.0","id":"tui-1","error":{"code":"unknown_session","message":"missing session","data":{"hint":"check the session id"}}}"#,
        )
        .expect("response decodes")
        .expect("error yields event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected app error");
        };
        // No `data.kind` -> top-level code/message win.
        assert_eq!(error.code, "unknown_session");
        assert!(
            error.message.contains("missing session"),
            "{}",
            error.message
        );
    }

    /// Guard: empty / whitespace `data.kind` does NOT win over the
    /// numeric code. The TUI cannot surface an empty discriminator as
    /// "the policy reason."
    #[test]
    fn rpc_error_ignores_blank_data_kind() {
        let event = rpc_text_to_app_event(
            r#"{"jsonrpc":"2.0","id":"tui-1","error":{"code":"unknown_session","message":"missing session","data":{"kind":"   "}}}"#,
        )
        .expect("response decodes")
        .expect("error yields event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected app error");
        };
        assert_eq!(error.code, "unknown_session");
    }

    #[test]
    fn protocol_malformed_frames_become_recoverable_errors() {
        let event = rpc_text_to_app_event("{")
            .expect("malformed JSON is recoverable")
            .expect("error event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected malformed JSON error");
        };
        assert_eq!(error.code, "malformed_json");

        let event = rpc_text_to_app_event(
            &json!({
                "method": methods::MESSAGE_DELTA,
                "params": {}
            })
            .to_string(),
        )
        .expect("missing jsonrpc is recoverable")
        .expect("error event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected invalid jsonrpc error");
        };
        assert_eq!(error.code, "invalid_jsonrpc");

        let event = rpc_text_to_app_event(
            &json!({
                "jsonrpc": "2.0",
                "method": methods::MESSAGE_DELTA,
                "params": {
                    "session_id": "local:test"
                }
            })
            .to_string(),
        )
        .expect("bad notification params are recoverable")
        .expect("error event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected invalid params error");
        };
        assert_eq!(error.code, "invalid_params");
        assert!(error.message.contains(methods::MESSAGE_DELTA));
    }

    #[test]
    fn protocol_exchange_tracks_requests_without_transport() {
        let mut exchange = ProtocolExchange::default();
        let session_id = SessionKey("local:test".into());
        let request = exchange
            .build_tracked_request(AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
                session_id: session_id.clone(),
            }))
            .expect("request builds");

        assert_eq!(
            exchange.pending_requests.get(&request.id),
            Some(&PendingRequest {
                method: methods::APPROVAL_SCOPES_LIST.into(),
            })
        );

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "scopes": []
            }
        })
        .to_string();
        let event = exchange
            .decode_rpc_text(&frame)
            .expect("response decodes")
            .expect("status event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Status(status) = event else {
            panic!("expected approval scopes status");
        };
        assert_eq!(
            status.message,
            "No persisted approval scopes for this session"
        );
        assert!(exchange.pending_requests.is_empty());
    }

    #[test]
    fn protocol_exchange_replays_session_cursor_without_transport() {
        let mut exchange = ProtocolExchange::default();
        let session_id = SessionKey("local:test".into());
        let cursor = UiCursor {
            stream: "session_events".into(),
            seq: 21,
        };
        let frame = json!({
            "jsonrpc": "2.0",
            "method": methods::SESSION_OPEN,
            "params": {
                "session_id": session_id.clone(),
                "active_profile_id": "coding",
                "cursor": cursor.clone()
            }
        })
        .to_string();

        exchange
            .decode_rpc_text(&frame)
            .expect("session/opened decodes")
            .expect("session opened event");
        let request = exchange
            .build_tracked_request(AppUiCommand::OpenSession(SessionOpenParams {
                session_id: session_id.clone(),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: None,
                after: None,
            }))
            .expect("request builds");

        assert_eq!(request.params["after"]["stream"], json!(cursor.stream));
        assert_eq!(request.params["after"]["seq"], json!(cursor.seq));
    }

    #[test]
    fn protocol_backend_cancels_pending_requests_on_disconnect() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let request = backend
            .build_tracked_request(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                session_id: SessionKey("local:test".into()),
                preview_id: PreviewId::new(),
            }))
            .expect("request builds");
        backend.mark_connected("wss://example.test/ui-protocol");

        backend.mark_disconnected("transport closed for test");

        assert!(backend.protocol.pending_requests.is_empty());
        let status = backend.queue.pop_front().expect("disconnect status");
        let status = unwrap_app_event(status);
        assert!(matches!(status, AppUiEvent::Status(_)));

        let cancelled = backend.queue.pop_front().expect("cancelled request");
        let cancelled = unwrap_app_event(cancelled);
        let AppUiEvent::Error(error) = cancelled else {
            panic!("expected cancellation error");
        };
        assert_eq!(error.code, "request_cancelled");
        assert!(error.message.contains(methods::DIFF_PREVIEW_GET));
        assert!(error.message.contains(&request.id));
        assert!(error.message.contains("transport closed for test"));
    }

    #[test]
    fn protocol_backend_bounds_pending_requests_before_transport_send() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        for index in 0..MAX_PENDING_REQUESTS {
            backend.protocol.pending_requests.insert(
                format!("existing-{index}"),
                PendingRequest {
                    method: methods::APPROVAL_SCOPES_LIST.into(),
                },
            );
        }

        backend
            .send(AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
                session_id: SessionKey("local:test".into()),
            }))
            .expect("pending saturation is reported as an app event");

        assert_eq!(
            backend.protocol.pending_requests.len(),
            MAX_PENDING_REQUESTS
        );
        let event = backend.next_event().expect("poll").expect("queued error");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected pending request limit error");
        };
        assert_eq!(error.code, "too_many_pending_requests");
        assert!(error.message.contains("pending request"));
        // M22-B: the message must include the rejected method name
        // so onboarding (and other callers) can attribute pre-send
        // rejections to the command that lost its slot.
        assert!(
            error.message.contains(methods::APPROVAL_SCOPES_LIST),
            "expected method name in too_many_pending_requests message, got: {}",
            error.message
        );
    }

    #[test]
    fn protocol_backend_maps_malformed_transport_frame_to_error() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch::default());

        let event = backend
            .handle_transport_frame(TransportFrame::Binary(vec![0xff]))
            .expect("malformed frame is recoverable")
            .expect("error event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Error(error) = event else {
            panic!("expected malformed frame error");
        };
        assert_eq!(error.code, "malformed_frame");
        assert!(error.message.contains("not UTF-8 JSON"));
    }

    #[test]
    fn protocol_backend_tracks_requests_and_clears_success_acks() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });

        let request = backend
            .build_tracked_request(AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: SessionKey("local:test".into()),
                turn_id: TurnId::new(),
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }))
            .expect("request builds");

        assert_eq!(
            backend.protocol.pending_requests.get(&request.id),
            Some(&PendingRequest {
                method: methods::TURN_START.into(),
            })
        );

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": { "accepted": true }
        })
        .to_string();
        let event = backend.decode_rpc_text(&frame).expect("ack decodes");

        assert!(event.is_none());
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_diff_preview_success_to_client_event() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let preview_id = PreviewId::new();
        let request = backend
            .build_tracked_request(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                session_id: session_id.clone(),
                preview_id: preview_id.clone(),
            }))
            .expect("request builds");

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "status": "requires_refresh",
                "source": "future_cache",
                "preview": {
                    "session_id": session_id,
                    "preview_id": preview_id,
                    "title": "Preview",
                    "files": [{
                        "path": "src/lib.rs",
                        "status": "copied",
                        "hunks": [{
                            "header": "@@ metadata @@",
                            "lines": [{
                                "kind": "metadata",
                                "content": "mode change"
                            }]
                        }]
                    }]
                }
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("diff response decodes")
            .expect("diff event");

        let ClientEvent::DiffPreview(result) = event else {
            panic!("expected diff preview client event");
        };
        assert_eq!(result.status, "requires_refresh");
        assert_eq!(result.source, "future_cache");
        assert_eq!(result.preview.files[0].status, "copied");
        assert_eq!(result.preview.files[0].hunks[0].lines[0].kind, "metadata");
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_session_open_result_to_opened_notification() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let request = backend
            .build_tracked_request(AppUiCommand::OpenSession(SessionOpenParams {
                session_id: session_id.clone(),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: Some("/repo".into()),
                after: None,
            }))
            .expect("request builds");

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "opened": {
                    "session_id": session_id,
                    "active_profile_id": "coding",
                    "workspace_root": "/repo",
                    "cursor": {
                        "stream": "session_events",
                        "seq": 11
                    }
                }
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("session open response decodes")
            .expect("session opened event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Protocol(UiNotification::SessionOpened(opened)) = event else {
            panic!("expected session opened notification");
        };
        assert_eq!(opened.session_id.0, "local:test");
        assert_eq!(opened.workspace_root.as_deref(), Some("/repo"));
        assert_eq!(opened.cursor.as_ref().map(|cursor| cursor.seq), Some(11));
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_approval_scopes_list_success_to_status() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let request = backend
            .build_tracked_request(AppUiCommand::ListApprovalScopes(ApprovalScopesListParams {
                session_id: session_id.clone(),
            }))
            .expect("request builds");

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "scopes": [{
                    "session_id": session_id,
                    "scope": "session",
                    "scope_match": "cargo test",
                    "decision": "approve"
                }]
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("approval scopes response decodes")
            .expect("status event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Status(status) = event else {
            panic!("expected status event");
        };
        assert_eq!(
            status.message,
            "1 persisted approval scope for this session"
        );
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_permission_profile_success_to_client_state() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let list_request = backend
            .build_tracked_request(AppUiCommand::ListPermissionProfiles(
                PermissionProfileListParams {
                    session_id: session_id.clone(),
                },
            ))
            .expect("list request builds");
        let list_frame = json!({
            "jsonrpc": "2.0",
            "id": list_request.id,
            "result": {
                "session_id": session_id,
                "current": {
                    "mode": "workspace-write",
                    "network": "deny"
                },
                "profiles": []
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&list_frame)
            .expect("permission list response decodes")
            .expect("permission profile event");
        let ClientEvent::PermissionProfile(profile) = event else {
            panic!("expected permission profile event");
        };
        assert_eq!(
            profile.message,
            "Permissions: Workspace Write, network blocked"
        );
        assert_eq!(profile.session_id, SessionKey("local:test".into()));
        assert_eq!(profile.current, PermissionProfileSelection::default());

        let set_request = backend
            .build_tracked_request(AppUiCommand::SetPermissionProfile(
                PermissionProfileSetParams {
                    session_id: SessionKey("local:test".into()),
                    update: PermissionProfileUpdate {
                        mode: Some(PermissionProfileMode::DangerFullAccess),
                        network: Some(PermissionNetworkPolicy::Allow),
                        approval_policy: Some("never".into()),
                    },
                    runtime_mode: None,
                },
            ))
            .expect("set request builds");
        let set_frame = json!({
            "jsonrpc": "2.0",
            "id": set_request.id,
            "result": {
                "session_id": "local:test",
                "current": {
                    "mode": "danger-full-access",
                    "network": "allow"
                },
                "applied": true
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&set_frame)
            .expect("permission set response decodes")
            .expect("permission profile event");
        let ClientEvent::PermissionProfile(profile) = event else {
            panic!("expected permission profile event");
        };
        assert_eq!(
            profile.message,
            "Permissions updated: Full Access, network allowed"
        );
        assert_eq!(
            profile.current,
            PermissionProfileSelection {
                mode: PermissionProfileMode::DangerFullAccess,
                network: PermissionNetworkPolicy::Allow,
            }
        );
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_runtime_cockpit_success_to_client_state_events() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());

        let status_request = backend
            .build_tracked_request(AppUiCommand::ReadSessionStatus(SessionStatusReadParams {
                session_id: session_id.clone(),
            }))
            .expect("status request builds");
        let status_frame = json!({
            "jsonrpc": "2.0",
            "id": status_request.id,
            "result": {
                "session_id": "local:test",
                "profile_id": "coding",
                "runtime_policy_stamp": {
                    "model": "deepseek-v4-pro",
                    "provider": "deepseek",
                    "tool_policy_id": "coding-v3",
                    "tool_contract_id": "codex-compatible-coding-v1",
                    "tool_contract_version": "1",
                    "model_toolset": "coding",
                    "dynamic_tool_discovery": "enabled",
                    "mcp_servers": [{
                        "id": "github",
                        "display_name": "GitHub",
                        "status": "connected",
                        "tool_count": 4
                    }]
                }
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&status_frame)
            .expect("session status response decodes")
            .expect("session status event");
        let ClientEvent::SessionStatus(status) = event else {
            panic!("expected session status event");
        };
        assert_eq!(status.result.session_id, session_id);
        assert_eq!(status.result.profile_id.as_deref(), Some("coding"));
        let stamp = status
            .result
            .runtime_policy_stamp
            .as_ref()
            .expect("runtime policy stamp");
        assert_eq!(
            stamp.tool_contract_id.as_deref(),
            Some("codex-compatible-coding-v1")
        );
        assert_eq!(stamp.mcp_servers[0].label(), "GitHub (connected, 4 tools)");

        let local_profile_request = backend
            .build_tracked_request(AppUiCommand::ProfileLocalCreate(ProfileLocalCreateParams {
                name: "Ada Lovelace".into(),
                username: "ada".into(),
                email: "ada@example.com".into(),
            }))
            .expect("local profile request builds");
        let local_profile_frame = json!({
            "jsonrpc": "2.0",
            "id": local_profile_request.id,
            "result": {
                "profile_id": "ada-server",
                "user_id": "ada-user",
                "name": "Ada Lovelace",
                "username": "ada",
                "email": "ada@example.com",
                "created": true,
                "runtime_mode": "solo"
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&local_profile_frame)
            .expect("local profile response decodes")
            .expect("local profile event");
        let ClientEvent::ProfileLocalCreate(profile) = event else {
            panic!("expected profile/local/create event");
        };
        assert_eq!(profile.result.profile_id, "ada-server");

        let model_request = backend
            .build_tracked_request(AppUiCommand::ListModels(ModelListParams {
                session_id: SessionKey("local:test".into()),
            }))
            .expect("model list request builds");
        let model_frame = json!({
            "jsonrpc": "2.0",
            "id": model_request.id,
            "result": {
                "session_id": "local:test",
                "models": [{
                    "model": "deepseek-v4-pro",
                    "provider": "deepseek",
                    "selected": true,
                    "available": true
                }]
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&model_frame)
            .expect("model list response decodes")
            .expect("model list event");
        let ClientEvent::ModelList(models) = event else {
            panic!("expected model list event");
        };
        assert_eq!(models.result.models[0].model, "deepseek-v4-pro");

        let mcp_request = backend
            .build_tracked_request(AppUiCommand::ListMcpStatus(McpStatusListParams {
                session_id: SessionKey("local:test".into()),
                include_disabled: true,
            }))
            .expect("mcp status request builds");
        let mcp_frame = json!({
            "jsonrpc": "2.0",
            "id": mcp_request.id,
            "result": {
                "session_id": "local:test",
                "servers": [{
                    "server": "github",
                    "status": "connected",
                    "tool_count": 8
                }, {
                    "server": "playwright",
                    "status": "failed",
                    "last_error": "not installed"
                }]
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&mcp_frame)
            .expect("mcp status response decodes")
            .expect("mcp status event");
        let ClientEvent::McpStatus(mcp) = event else {
            panic!("expected mcp status event");
        };
        assert_eq!(mcp.result.servers.len(), 2);
        assert_eq!(
            mcp.result.servers[1].last_error.as_deref(),
            Some("not installed")
        );

        let tool_request = backend
            .build_tracked_request(AppUiCommand::ListToolStatus(ToolStatusListParams {
                session_id: SessionKey("local:test".into()),
                include_denied: true,
            }))
            .expect("tool status request builds");
        let tool_frame = json!({
            "jsonrpc": "2.0",
            "id": tool_request.id,
            "result": {
                "session_id": "local:test",
                "policy_id": "coding-v3",
                "coding_tool_contract": {
                    "id": "codex-compatible-coding-v1",
                    "version": "1",
                    "feature": "coding.tool_contract.v1",
                    "status": "incomplete",
                    "required_tool_names": ["apply_patch", "exec_command"],
                    "missing_required_tools": ["exec_command"],
                    "policy": {
                        "tool_policy_id": "coding-v3",
                        "sandbox_mode": "workspace-write",
                        "approval_policy": "on-request"
                    },
                    "required_tools": [{
                        "name": "exec_command",
                        "category": "runtime",
                        "aliases": ["shell"],
                        "capability": "coding.exec_session.v1",
                        "policy": "approval_gated",
                        "status": "missing",
                        "backend_tool": null
                    }]
                },
                "tools": []
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&tool_frame)
            .expect("tool status response decodes")
            .expect("tool status event");
        let ClientEvent::ToolStatus(tools) = event else {
            panic!("expected tool status event");
        };
        let contract = tools
            .result
            .coding_tool_contract
            .as_ref()
            .expect("coding tool contract");
        assert_eq!(contract.status, "incomplete");
        assert_eq!(
            contract.missing_required_tools,
            vec!["exec_command".to_string()]
        );
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_task_output_read_success_to_output_delta() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let task_id = TaskId::new();
        let request = backend
            .build_tracked_request(AppUiCommand::ReadTaskOutput(TaskOutputReadParams {
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                cursor: Some(OutputCursor { offset: 12 }),
                limit_bytes: Some(4096),
            }))
            .expect("request builds");

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "session_id": session_id,
                "task_id": task_id,
                "source": "runtime_projection",
                "cursor": { "offset": 12 },
                "next_cursor": { "offset": 31 },
                "text": "task output window\n",
                "bytes_read": 19,
                "total_bytes": 31,
                "truncated": false,
                "complete": true,
                "live_tail_supported": false,
                "task_status": "completed",
                "runtime_state": "completed",
                "lifecycle_state": "completed",
                "output_files": [],
                "limitations": []
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("task output response decodes")
            .expect("task output event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Protocol(UiNotification::TaskOutputDelta(event)) = event else {
            panic!("expected task output delta event");
        };
        assert_eq!(event.session_id.0, "local:test");
        assert_eq!(event.cursor, OutputCursor { offset: 31 });
        assert_eq!(event.text, "task output window\n");
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_preserves_task_output_metadata_when_window_omits_it() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());
        let task_id = TaskId::new();
        let request = backend
            .build_tracked_request(AppUiCommand::ReadTaskOutput(TaskOutputReadParams {
                session_id: session_id.clone(),
                task_id: task_id.clone(),
                cursor: Some(OutputCursor { offset: 128 }),
                limit_bytes: Some(128),
            }))
            .expect("request builds");

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": {
                "session_id": session_id,
                "task_id": task_id,
                "source": "runtime_projection",
                "cursor": { "offset": 128 },
                "next_cursor": { "offset": 156 },
                "text": "tail window without metadata\n",
                "bytes_read": 28,
                "total_bytes": 512,
                "truncated": true,
                "complete": false,
                "live_tail_supported": false,
                "task_status": "completed",
                "runtime_state": "completed",
                "lifecycle_state": "completed",
                "output_files": ["/repo/out/report.md"],
                "limitations": [
                    {
                        "code": "runtime_projection",
                        "message": "snapshot projection, not live stdout"
                    }
                ]
            }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("task output response decodes")
            .expect("task output event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Protocol(UiNotification::TaskOutputDelta(event)) = event else {
            panic!("expected task output delta event");
        };
        assert!(event.text.contains("tail window without metadata"));
        assert!(event.text.contains("output_files:\n- /repo/out/report.md"));
        assert!(
            event
                .text
                .contains("limitations:\n- snapshot projection, not live stdout")
        );
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_interrupt_success_to_cancel_status() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let request = backend
            .build_tracked_request(AppUiCommand::InterruptTurn(TurnInterruptParams {
                session_id: SessionKey("local:test".into()),
                turn_id: TurnId::new(),
            }))
            .expect("request builds");
        let frame = json!({
            "jsonrpc": "2.0",
            "id": request.id,
            "result": { "interrupted": true }
        })
        .to_string();

        let event = backend
            .decode_rpc_text(&frame)
            .expect("interrupt response decodes")
            .expect("status event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Status(status) = event else {
            panic!("expected interrupt status event");
        };
        assert_eq!(
            status.message,
            "Interrupt acknowledged; active turn cancelled"
        );
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn protocol_backend_maps_error_responses_with_request_context() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });
        let request = backend
            .build_tracked_request(AppUiCommand::OpenSession(SessionOpenParams {
                session_id: SessionKey("local:test".into()),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: None,
                after: None,
            }))
            .expect("request builds");
        let request_id = request.id.clone();

        let frame = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {
                "code": -32602,
                "message": "missing session"
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&frame)
            .expect("error decodes")
            .expect("error event");
        let event = unwrap_app_event(event);

        let AppUiEvent::Error(error) = event else {
            panic!("expected app error");
        };
        assert_eq!(error.code, "-32602");
        assert!(error.message.contains(methods::SESSION_OPEN));
        assert!(error.message.contains(&request.id));
        assert!(error.message.contains("missing session"));
        assert!(backend.protocol.pending_requests.is_empty());
    }

    #[test]
    fn launch_from_cli_defaults_cwd_to_process_current_dir() {
        let cli = Cli {
            config: None,
            mode: crate::cli::Mode::Protocol,
            base_url: Some("wss://example.test/ui-protocol".into()),
            stdio_command: None,
            session: Some("local:test".into()),
            profile_id: Some("coding".into()),
            cwd: None,
            auth_token: None,
            readonly: false,
            theme: crate::cli::ThemeName::Codex,
        };

        let launch = launch_from_cli(&cli);

        assert_eq!(
            launch.cwd,
            Some(
                std::env::current_dir()
                    .expect("current dir")
                    .to_string_lossy()
                    .to_string()
            )
        );
    }

    #[test]
    fn protocol_session_open_request_includes_cwd() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            cwd: Some("/tmp/project".into()),
            ..AppUiLaunch::default()
        });
        let request = backend
            .build_tracked_request(AppUiCommand::OpenSession(SessionOpenParams {
                session_id: SessionKey("local:test".into()),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: Some("/tmp/project".into()),
                after: None,
            }))
            .expect("request builds");

        assert_eq!(request.params["cwd"], json!("/tmp/project"));
    }

    #[test]
    fn protocol_backend_captures_cursor_and_reuses_it_on_session_open() {
        let session_id = SessionKey("local:test".into());
        let opened_cursor = UiCursor {
            stream: "session_events".into(),
            seq: 7,
        };
        let turn_cursor = UiCursor {
            stream: "session_events".into(),
            seq: 9,
        };
        let turn_id = TurnId::new();
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });

        let session_opened = json!({
            "jsonrpc": "2.0",
            "method": methods::SESSION_OPEN,
            "params": {
                "session_id": session_id.clone(),
                "active_profile_id": "coding",
                "cursor": opened_cursor.clone()
            }
        })
        .to_string();
        let event = backend
            .decode_rpc_text(&session_opened)
            .expect("session/opened decodes")
            .expect("event");
        let event = unwrap_app_event(event);
        assert!(matches!(
            event,
            AppUiEvent::Protocol(UiNotification::SessionOpened(_))
        ));
        assert_eq!(
            backend.protocol.session_cursors.get(&session_id),
            Some(&opened_cursor)
        );

        let turn_completed = json!({
            "jsonrpc": "2.0",
            "method": methods::TURN_COMPLETED,
            "params": {
                "session_id": session_id.clone(),
                "turn_id": turn_id,
                "cursor": turn_cursor.clone()
            }
        })
        .to_string();
        backend
            .decode_rpc_text(&turn_completed)
            .expect("turn/completed decodes")
            .expect("event");
        assert_eq!(
            backend.protocol.session_cursors.get(&session_id),
            Some(&turn_cursor)
        );

        let request = backend
            .build_tracked_request(AppUiCommand::OpenSession(SessionOpenParams {
                session_id: session_id.clone(),
                topic: None,
                profile_id: Some("coding".into()),
                cwd: None,
                after: None,
            }))
            .expect("request builds");

        assert_eq!(
            request.params["after"]["stream"],
            json!(turn_cursor.stream.clone())
        );
        assert_eq!(request.params["after"]["seq"], json!(turn_cursor.seq));
    }

    #[test]
    fn protocol_backend_readonly_bootstrap_connects_opens_and_reads_existing_session() {
        let server = spawn_protocol_capture_server(4, true);
        let session_id = SessionKey("session-123".into());
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(server.endpoint.clone(), None)),
            session_id: Some(session_id.clone()),
            profile_id: Some("coding".into()),
            cwd: Some("/repo".into()),
            readonly: true,
            ..AppUiLaunch::default()
        });

        let snapshot = backend.bootstrap().expect("readonly bootstrap connects");

        assert_eq!(snapshot.sessions[0].id.0, "session-123");
        assert!(snapshot.status.contains("read-only"));
        assert!(
            snapshot.sessions[0].messages[0]
                .content
                .contains("mutating commands disabled")
        );
        assert!(backend.is_connected());
        assert_eq!(backend.connection_state, ProtocolConnectionState::Connected);

        let capabilities_request = server.recv_json();
        assert_eq!(
            capabilities_request["method"],
            crate::model::APPUI_METHOD_CONFIG_CAPABILITIES_LIST
        );

        let llm_request = server.recv_json();
        assert_eq!(llm_request["method"], crate::model::APPUI_METHOD_MODEL_LIST);
        assert_eq!(llm_request["params"]["profile_id"], json!("coding"));

        let open_request = server.recv_json();
        assert_eq!(open_request["method"], methods::SESSION_OPEN);
        assert_eq!(open_request["params"]["session_id"], json!("session-123"));
        assert_eq!(open_request["params"]["cwd"], json!("/repo"));

        let event = next_event_until(&mut backend);
        let ClientEvent::Capabilities(_) = event else {
            panic!("expected capabilities event");
        };

        let event = next_event_until(&mut backend);
        let event = unwrap_app_event(event);
        let AppUiEvent::Protocol(UiNotification::SessionOpened(opened)) = event else {
            panic!("expected session opened notification");
        };
        assert_eq!(opened.session_id, session_id);
        assert_eq!(opened.workspace_root.as_deref(), Some("/repo"));

        let preview_id = PreviewId::new();
        backend
            .send(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                session_id: opened.session_id,
                preview_id: preview_id.clone(),
            }))
            .expect("readonly diff preview request sends");

        let diff_request = server.recv_json();
        assert_eq!(diff_request["method"], methods::DIFF_PREVIEW_GET);
        assert_eq!(diff_request["params"]["preview_id"], json!(preview_id));
        server.join();
    }

    #[test]
    fn protocol_backend_readonly_bootstrap_survives_connection_failure() {
        let session_id = SessionKey("session-123".into());
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            session_id: Some(session_id.clone()),
            profile_id: Some("coding".into()),
            readonly: true,
            ..AppUiLaunch::default()
        });
        backend.runtime = None;
        backend.runtime_error = Some("runtime unavailable for test".into());

        let snapshot = backend
            .bootstrap()
            .expect("readonly bootstrap returns offline snapshot");

        assert!(snapshot.readonly);
        assert_eq!(snapshot.sessions[0].id, session_id);
        assert!(snapshot.status.contains("read-only"));
        assert!(snapshot.status.contains("no network connection opened"));
        assert_eq!(
            backend.connection_state,
            ProtocolConnectionState::Disconnected
        );
        let event = backend.queue.pop_front().expect("offline status event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Status(status) = event else {
            panic!("expected status event");
        };
        assert!(status.message.contains("no network connection opened"));
    }

    #[test]
    fn protocol_backend_records_disconnected_status() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            ..AppUiLaunch::default()
        });

        backend.mark_connected("wss://example.test/ui-protocol");
        backend.mark_disconnected("UI protocol disconnected for test.");

        assert_eq!(
            backend.connection_state,
            ProtocolConnectionState::Disconnected
        );
        let event = backend.queue.pop_front().expect("status event");
        let event = unwrap_app_event(event);
        let AppUiEvent::Status(status) = event else {
            panic!("expected status event");
        };
        assert!(status.message.contains("disconnected"));
    }

    #[test]
    fn reconnect_session_open_command_resumes_from_recorded_cursor() {
        let session_id = SessionKey("local:test".into());
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            profile_id: Some("coding".into()),
            session_id: Some(session_id.clone()),
            cwd: Some("/tmp/workspace".into()),
            ..AppUiLaunch::default()
        });
        backend.protocol.session_cursors.insert(
            session_id.clone(),
            UiCursor {
                stream: session_id.0.clone(),
                seq: 42,
            },
        );

        let command = backend
            .launch_session_open_command()
            .expect("launch session should reopen");
        let request = backend
            .build_tracked_request(command)
            .expect("request builds");

        assert_eq!(request.method, methods::SESSION_OPEN);
        assert_eq!(request.params["session_id"], "local:test");
        assert_eq!(request.params["profile_id"], "coding");
        assert_eq!(request.params["cwd"], "/tmp/workspace");
        assert_eq!(request.params["after"]["stream"], "local:test");
        assert_eq!(request.params["after"]["seq"], 42);
    }

    #[test]
    fn protocol_snapshot_honors_requested_session() {
        let launch = AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            session_id: Some(SessionKey("session-123".into())),
            profile_id: Some("coding".into()),
            readonly: true,
            ..AppUiLaunch::default()
        };

        let snapshot = protocol_snapshot_from_launch(&launch, "wss://example.test/ui-protocol");

        assert_eq!(
            snapshot.target.as_deref(),
            Some("wss://example.test/ui-protocol")
        );
        assert!(snapshot.readonly);
        assert_eq!(snapshot.sessions[0].id.0, "session-123");
        assert_eq!(snapshot.sessions[0].profile_id.as_deref(), Some("coding"));
    }

    #[test]
    fn protocol_backend_readonly_blocks_mutations_without_network() {
        let mut backend = ProtocolAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            readonly: true,
            ..AppUiLaunch::default()
        });
        let session_id = SessionKey("local:test".into());

        backend
            .send(AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: session_id.clone(),
                turn_id: TurnId::new(),
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }))
            .expect("readonly send is local");
        backend
            .send(AppUiCommand::InterruptTurn(TurnInterruptParams {
                session_id: session_id.clone(),
                turn_id: TurnId::new(),
            }))
            .expect("readonly interrupt is local");
        backend
            .send(AppUiCommand::RespondApproval(ApprovalRespondParams::new(
                session_id,
                octos_core::ui_protocol::ApprovalId::new(),
                ApprovalDecision::Deny,
            )))
            .expect("readonly approval response is local");

        let event = backend.next_event().expect("poll").expect("warning");
        let event = unwrap_app_event(event);
        assert!(matches!(
            event,
            AppUiEvent::Protocol(UiNotification::Warning(_))
        ));
        let event = backend
            .next_event()
            .expect("poll")
            .expect("interrupt error");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected interrupt readonly error");
        };
        assert!(error.message.contains(methods::TURN_INTERRUPT));

        let event = backend
            .next_event()
            .expect("poll")
            .expect("approval response error");
        let event = unwrap_app_event(event);
        let AppUiEvent::Error(error) = event else {
            panic!("expected approval readonly error");
        };
        assert!(error.message.contains(methods::APPROVAL_RESPOND));
        assert!(!backend.is_connected());
    }

    #[test]
    fn mock_backend_queues_turn_events() {
        let mut backend = MockAppUiBackend::default();
        backend.bootstrap().expect("bootstrap");
        let session = MockAppUiBackend::mock_session_key("coding", "m9");

        backend
            .send(AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: session,
                turn_id: TurnId::new(),
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }))
            .expect("send");

        let first = backend.next_event().expect("poll");
        assert!(first.is_some());
    }

    #[test]
    fn mock_backend_submit_prompt_does_not_replace_session_before_approval() {
        let mut backend = MockAppUiBackend::default();
        let snapshot = backend.bootstrap().expect("bootstrap");
        let session_id = snapshot.sessions[0].id.clone();

        let opened = backend.next_event().expect("poll").expect("session opened");
        let opened = unwrap_app_event(opened);
        let AppUiEvent::Protocol(UiNotification::SessionOpened(opened)) = opened else {
            panic!("expected bootstrap session/opened");
        };
        assert_eq!(opened.session_id, session_id);

        let turn_id = TurnId::new();
        backend
            .send(AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                input: vec![InputItem::Text {
                    text: "complete m9 contract".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }))
            .expect("submit prompt");

        let mut saw_turn_started = false;
        loop {
            let event = backend
                .next_event()
                .expect("poll")
                .expect("mock turn event before approval");
            let event = unwrap_app_event(event);
            match event {
                AppUiEvent::Snapshot(_) => {
                    panic!("turn/send must not emit a snapshot that can erase optimistic user text")
                }
                AppUiEvent::Protocol(UiNotification::SessionOpened(_)) => {
                    panic!("turn/send must not reopen the session before approval")
                }
                AppUiEvent::Protocol(UiNotification::TurnStarted(event)) => {
                    assert_eq!(event.session_id, session_id);
                    assert_eq!(event.turn_id, turn_id);
                    saw_turn_started = true;
                }
                AppUiEvent::Protocol(UiNotification::ApprovalRequested(event)) => {
                    assert_eq!(event.session_id, session_id);
                    assert_eq!(event.turn_id, turn_id);
                    assert!(
                        saw_turn_started,
                        "approval must follow turn/started so store state is active first"
                    );
                    break;
                }
                _ => {}
            }
        }
    }

    #[test]
    fn mock_approval_event_supports_all_m9_14_kinds() {
        let session_id = SessionKey("local:test".into());
        let turn_id = TurnId::new();
        let cases = [
            (approval_kinds::COMMAND, "command", "cargo test"),
            (approval_kinds::DIFF, "diff", "Update the coding loop"),
            (
                approval_kinds::FILESYSTEM,
                "filesystem",
                "/tmp/octos-mock-approval.txt",
            ),
            (approval_kinds::NETWORK, "network", "example.com"),
            (
                approval_kinds::SANDBOX_ESCALATION,
                "sandbox_escalation",
                "danger-full-access",
            ),
        ];

        for (input, expected_kind, expected_detail) in cases {
            let event = mock_approval_event(session_id.clone(), turn_id.clone(), input);
            assert_eq!(event.approval_kind.as_deref(), Some(expected_kind));
            let payload = serde_json::to_string(&event).expect("approval event serializes");
            assert!(
                payload.contains(expected_detail),
                "missing {expected_detail} in {payload}"
            );
        }
    }

    #[test]
    fn mock_backend_accepts_approval_response_and_diff_preview_requests() {
        let mut backend = MockAppUiBackend::default();
        let session_id = MockAppUiBackend::mock_session_key("coding", "m9");
        let approval_id = octos_core::ui_protocol::ApprovalId::new();
        let preview_id = PreviewId::new();

        backend
            .send(AppUiCommand::RespondApproval(ApprovalRespondParams::new(
                session_id.clone(),
                approval_id,
                ApprovalDecision::Deny,
            )))
            .expect("mock approval response accepted");

        let warning = backend.next_event().expect("poll").expect("warning");
        let warning = unwrap_app_event(warning);
        assert!(matches!(
            warning,
            AppUiEvent::Protocol(UiNotification::Warning(_))
        ));

        backend
            .send(AppUiCommand::GetDiffPreview(DiffPreviewGetParams {
                session_id: session_id.clone(),
                preview_id: preview_id.clone(),
            }))
            .expect("mock diff preview accepted");

        let event = backend.next_event().expect("poll").expect("diff preview");
        let ClientEvent::DiffPreview(result) = event else {
            panic!("expected diff preview event");
        };
        assert_eq!(result.preview.session_id, session_id);
        assert_eq!(result.preview.preview_id, preview_id);
        assert_eq!(result.status, "ready");
        assert_eq!(result.preview.files[0].path, "src/coding_loop.rs");
    }

    #[test]
    fn mock_backend_bootstrap_honors_launch_options() {
        let mut backend = MockAppUiBackend::new(AppUiLaunch {
            endpoint: Some(AppUiEndpoint::websocket(
                "wss://example.test/ui-protocol",
                None,
            )),
            session_id: Some(SessionKey("session-123".into())),
            profile_id: Some("review".into()),
            readonly: true,
            ..AppUiLaunch::default()
        });

        let data = backend.bootstrap().expect("bootstrap");

        assert_eq!(data.selected_session, 0);
        assert_eq!(
            data.target.as_deref(),
            Some("wss://example.test/ui-protocol")
        );
        assert!(data.readonly);
        assert_eq!(data.sessions[0].id.0, "session-123");
        assert_eq!(data.sessions[0].profile_id.as_deref(), Some("review"));
    }

    #[test]
    fn mock_backend_readonly_submit_prompt_emits_warning() {
        let mut backend = MockAppUiBackend::new(AppUiLaunch {
            readonly: true,
            ..AppUiLaunch::default()
        });
        let session_id = MockAppUiBackend::mock_session_key("coding", "m9");
        let turn_id = TurnId::new();

        backend
            .send(AppUiCommand::SubmitPrompt(TurnStartParams {
                session_id,
                turn_id,
                input: vec![InputItem::Text {
                    text: "hello".into(),
                }],
                media: Vec::new(),
                topic: None,
                rewrite_for: None,
            }))
            .expect("send");

        let notification = backend.next_event().expect("poll").expect("warning");
        let notification = unwrap_app_event(notification);
        assert!(matches!(
            notification,
            AppUiEvent::Protocol(UiNotification::Warning(_))
        ));
    }
}
