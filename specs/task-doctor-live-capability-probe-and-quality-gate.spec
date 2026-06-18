spec: task
name: "doctor live capability probe 与严格质量门"
inherits: project
tags: [doctor, appui, websocket, protocol, quality-gate]
depends: []
estimate: 1d
---

## 意图

`octos-tui doctor --endpoint ...` 原先只记录 WS endpoint 已配置，并继续跑本地
结构性 protocol-skew 检查；这无法发现真实 server 是否可连、是否支持
`config/capabilities/list`、以及 live capability set 是否满足 TUI 运行所需。
本任务补齐 doctor 的 live AppUI capability probe，并把本轮发现的严格质量门
问题纳入 spec：测试在受限环境中不能因为后台线程 bind 失败而 panic，
`cargo clippy --all-targets --all-features -- -D warnings` 必须可作为合并门。

## 已定决策

- WS endpoint 模式优先执行 live probe：建立 WebSocket 连接，携带
  `X-Octos-Ui-Features`，发送 JSON-RPC
  `config/capabilities/list`，并把返回的 `UiProtocolCapabilities` 交给
  `compare_against_server` 做 protocol/schema/feature 兼容性判断。
- live probe 成功时，不再追加本地 structural fallback 的 protocol-skew 检查，
  避免同一 backend category 输出两份语义相近的结果；live probe 失败、无 endpoint
  或 stdio 模式仍保留 structural fallback。
- WS probe 使用短超时、当前线程 Tokio runtime、明确的 JSON-RPC id，并同时接受
  text/binary JSON frame；ping/pong 忽略，close/timeout/JSON-RPC error 转为
  doctor warning，而不是让 doctor 崩溃。
- response loop 必须跳过 unrelated JSON-RPC notification 或 id 不匹配的 frame，
  直到收到当前 doctor request id 对应的 capabilities response 或超时/关闭。
- 本轮 probe 是快速健康检查：单步连接/发送/接收 timeout 固定为 2 秒；受保护
  endpoint、慢代理或需要额外认证 header 的 endpoint 先报告 warning，不在本任务中
  做 token 发现或交互式认证。
- protocol 测试服务在测试主线程先用 `std::net::TcpListener::bind("127.0.0.1:0")`
  取得端口，再交给 Tokio listener；如果当前环境禁止 loopback bind，则测试
  早退跳过该网络路径，避免后台线程 panic 或等待地址 channel 超时。
- 为通过 `-D warnings`，只做语义保持型 Rust cleanup：大 enum payload 改为
  `Box<AppUiCommand>`/`Box<LlmSelectionConfig>`，增加小构造 helper，能 derive
  `Default` 的类型改用 derive，测试初始化改为 struct literal，重复复杂类型抽出
  alias。不得借 clippy cleanup 改变菜单、onboarding、pager 或 AppUI 命令语义。
- AppUI capability 文档必须反映当前实现：菜单 provider 使用服务器广告的
  capability map，doctor 也使用同一个 `config/capabilities/list` contract；
  剩余 gap 只记录尚未拥有的服务端字段或可选方法。

## 边界

### Allowed Changes

- src/cmd/doctor.rs
- src/transport.rs
- src/event_loop.rs
- src/menu/types.rs
- src/menu/providers.rs
- src/menu/availability.rs
- src/model.rs
- src/store.rs
- src/app.rs
- tests/onboarding_saved_provider_contract.rs
- tests/transcript_pager_contract.rs
- docs/AppUI_MENU_CAPABILITY_GAPS.md
- specs/**

### Forbidden

- 不新增 AppUI 方法名；doctor 只能调用既有 `config/capabilities/list`。
- 不在 doctor 中读取 server 内部文件、profile JSON、MCP 配置或 agent internals。
- 不因为 live probe 失败返回进程 hard failure；doctor backend connectivity 问题应
  以 warning/fail check 表达，由 renderer 决定最终报告。
- 不引入新的 crate 依赖；只能使用项目已有的 async/WebSocket/serde 依赖。
- 不把受限环境下的 loopback bind 失败算作 contract regression。

## 排除范围

- stdio transport 的 live capability handshake。当前 stdio 仍只验证命令解析与
  结构性 protocol-skew fallback。
- 认证握手、token 刷新、server 自动启动、endpoint 自动发现或 probe timeout 配置。
- `octos-core::diagnostics` 抽取与 `octos doctor` 共享实现。
- MCP refresh/reload、approval scopes clear、模型 reasoning effort 等后续 AppUI
  contract 扩展。

## 完成条件

场景: doctor WS endpoint 跳过无关 frame 并成功拉取 live capabilities
  测试: ws_probe_ignores_unrelated_frames_and_fetches_live_capabilities
  假设 本地 mock WebSocket server 接受连接、先推送 unrelated notification、
       再响应 config/capabilities/list
  当 doctor probe 连接该 endpoint
  那么 请求 method 为 config/capabilities/list
  并且 返回的 UiProtocolCapabilities 被成功解码

场景: JSON-RPC unrelated frame 被忽略
  测试: decode_matching_capabilities_response_ignores_unrelated_frame
  假设 server 推送一个无 id 的 notification
  当 doctor 解码该 frame
  那么 返回 None 而不是 capabilities error

场景: JSON-RPC result 可解码为 capability payload
  测试: decode_capabilities_response_accepts_result
  假设 server 返回 id 匹配且包含 result.capabilities
  当 doctor 解码该 frame
  那么 得到 UiProtocolCapabilities 且 protocol version 与 TUI 预期一致

场景: JSON-RPC error 被转成可读诊断
  测试: decode_capabilities_response_reports_jsonrpc_error
  假设 server 对 config/capabilities/list 返回 JSON-RPC error
  当 doctor 解码该 frame
  那么 返回包含 server error message 的错误文本

场景: protocol test server 在受限环境中不造成测试 panic
  测试: protocol_backend_readonly_bootstrap_connects_opens_and_reads_existing_session
  假设 当前环境允许 loopback bind
  当 readonly bootstrap 测试启动 mock protocol server
  那么 listener 已在主线程绑定并交给 Tokio 接受连接
  并且 backend 完成 connect/open/read bootstrap

场景: capabilities 连接被取消后 backend 会重试且不冒泡错误
  测试: protocol_backend_retries_cancelled_capabilities_without_surfacing_error
  假设 mock server 第一次 capabilities 请求断连、第二次返回成功
  当 ProtocolAppUiBackend 连接该 endpoint
  那么 capabilities 请求会重试
  并且 不向 UI surface 临时取消错误

场景: Box 化 action 不改变 onboarding hydration 契约
  测试: onboard_open_with_profile_hydrates_saved_provider
  假设 profile 已解析且当前 profile 的 llm state 尚未 hydrate
  当 用户打开 /onboard 或执行 /onboard profile <id>
  那么 仍发出 profile/llm/list 请求
  并且 请求参数中的 profile_id 不因 Box<AppUiCommand> 包装而改变

场景: Box 化 action 不改变 pager 内 composer 提交契约
  测试: pager_with_empty_transcript_renders_safely
  假设 transcript pager 激活但 transcript 为空
  当 用户在 pinned composer 中提交输入
  那么 仍发出 SubmitPrompt AppUI command
  并且 pager 空状态不 panic

场景: 全仓严格 clippy 门通过
  测试: cargo clippy --all-targets --all-features -- -D warnings
  假设 本轮只做语义保持型 cleanup
  当 在 worktree 根目录运行严格 clippy
  那么 不出现 warning 或 lint error

场景: 全仓测试门通过
  测试: cargo test --all-targets
  假设 mock-backed contract tests 与 unit tests 均可运行
  当 在 worktree 根目录运行全量测试
  那么 所有确定性测试通过，允许既有 ignored 测试保持 ignored
