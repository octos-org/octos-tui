# 《octos-tui 现状分析 + agent-spec 接手方案》

> 交付对象:AlexZ。目的:据此决定如何用 agent-spec 接手 octos-tui 后续开发。
> 仓库:`/Users/zhangalex/Work/Projects/FW/octos-tui`,HEAD `840ee2e`(2026-06-06),~64k 行 Rust。
> **校正一处贯穿全文的事实**:研究材料里出现的 "octos HEAD 840ee2e" 实为 **octos-tui 的 HEAD**;后端主仓 octos 的 HEAD 是 **`13e4623b`**。本文中 840ee2e = octos-tui,13e4623b = octos 后端,二者不同。

---

## 1. 执行摘要

octos-tui 是 octos 后端的**纯终端客户端**:spawn `octos serve --stdio`(或连 WebSocket)后走 JSON-RPC 的 "Octos UI Protocol",自身不跑 agent、不执行 tool、不拥有 provider/sandbox/permission 真相。代码质量信号强(780 个测试、10 个里程碑契约测试文件、`unsafe_code="deny"` 且零 unsafe、全树仅 4 处 TODO),架构是干净的单向数据流 reducer。**当前焦点是 M22 solo onboarding**(June 2026 提交簇:onboarding 向导 #191/#195、菜单打磨 #185-#192、i18n EN→ZH #197、inline viewport+scrollback+OSC52),M12–M16 已在 2026-05-20 一次性落地,**无 M23+**——前路是 "已记录的 AppUI follow-ups + 被推迟的 live-soak 门"。**接手建议核心**:octos-tui 已经在实践 agent-spec 所形式化的那套工作流(里程碑契约文档 → `tests/m*_*_contract.rs` 契约测试 → codex review),且后端 octos 已先行接入 agent-spec(`specs/task-matrix-media-support.spec.md` 驱动了最新 Matrix 媒体提交)——接手就是把每个 gap 写成一个 `specs/*.spec.md`、把现有契约测试函数绑成 `Test:` selector、用 `agent-spec guard` 补上**目前完全缺失的 PR 质量门禁**。最大的两个结构性风险是 `store.rs` 的 ~7000 行 "上帝 reducer" 和**跨仓协议契约的手动 pin(无编译门禁,silent-drift)**。

---

## 2. 架构速览

### 2.1 模块职责与单向数据流

octos-tui 是经典的 **单向数据流 + reducer**:键盘/后端事件 → `Store` 归约进唯一真值 `AppState` → `app::render*` 纯函数渲染。

```
键盘/鼠标/粘贴 ──┐
后端 ClientEvent ─┤→ event_loop.rs  (终端 raw mode + draw/poll/send 主循环)
                  │     ├─ store.rs    Store::compose_command / apply_*   (reducer,唯一写状态处)
                  │     │      └→ AppState (model.rs:3029)   唯一真值
                  │     ├─ app.rs      render*(&AppState, Palette)         纯渲染,不持状态/不发命令
                  │     ├─ viewport.rs ScrollbackTracker::sync  把已定稿历史增量刷进原生 scrollback
                  │     └─ transport.rs AppUiBackend.send/next_event
                  │            │ AppUiCommand → JSON-RPC
                  │            ▼
                  └──── ClientEvent ← UiNotification/RPC result ← octos serve --stdio | ws
```

三段流转:① **输入→命令**:`event_loop::run` 主循环(空闲 25ms / turn 活跃 120ms poll)把按键转成 `KeyAction::{Continue,Quit,Send(AppUiCommand)}`;Enter 触发 `Store::compose_command`(store.rs:241),生成 `turn_id`、乐观追加用户消息、返回 `AppUiCommand::SubmitPrompt`。② **命令→后端→事件**:backend 发 `turn/start` 等 JSON-RPC,服务器回流 `ClientEvent`。③ **事件→状态→渲染**:`Store::apply_client_event`(store.rs:3807)归约,可返回 follow-up 命令;`dirty=true` 后下一帧重画。**关键不变量:redraw on change 而非定频**,空闲不写终端,以保住用户的原生终端选区。

各模块行数与职责(行数即维护/导航成本信号):

| 模块 | 行数 | 职责 |
|---|---|---|
| `store.rs` | **17.9k** | reducer。`struct Store{ state: AppState }` 薄包装(:184);`impl Store` 容纳全部 ~40 个 `dispatch_*` 与 ~35 个 `apply_*`。**单点最大技术债** |
| `app.rs` | 10.5k | 纯渲染。chat/inspector/onboarding/transcript/tasks/artifacts 各 `render_*`,viewport 用的 fingerprint 纯函数 |
| `menu/providers.rs` | 7.3k | 22 个 `MenuProvider` 实现(theme/model/permissions/mcp/skills…) |
| `transport.rs` | 7.3k | `AppUiBackend` trait + mock/protocol 双后端 + WS/stdio 双 driver + JSON-RPC 帧 |
| `model.rs` | 7.1k | `AppUiCommand` 枚举(67 变体)、`AppState`(~50 字段)、各视图模型、param 结构 |
| `event_loop.rs` | 2.9k | 终端 raw mode、键盘分发、后端轮询、OSC52 flush、capabilities 握手 |
| `viewport.rs` | 392 | `ScrollbackTracker`,inline viewport / scrollback 增量同步 |
| `autonomy.rs` | 908 | M15-E `/agents //goal //loop` 语法解析(只解析,dispatch 在 store.rs) |
| `clipboard.rs` | 259 | OSC52 复制(含 tmux DCS passthrough),SSH-safe |
| `cmd/{doctor,update,install_method}.rs` | — | 非交互子命令 |
| `keymap.rs` | **1 行** | 仅一个 `HELP` 常量——名实不符(见 §3) |

### 2.2 关键扩展点

1. **出站命令面 `AppUiCommand`(model.rs:591,67 变体)** —— 每个变体 `.method()` 返回 `&'static str`,**多数指向 `octos_core::ui_protocol::methods::*` 共享常量**(如 `SESSION_OPEN`/`TURN_START`)。注意:**枚举本身是 TUI 本地定义,不是从 octos-core 共享**(见 §6 协议契约风险)。
2. **入站事件面 `ClientEvent`(client_event.rs:24)** —— `App(AppUiEvent)` + ~25 个 typed 结果变体,store 精确 pattern-match。
3. **菜单 registry(`src/menu/`)** —— `CommandRegistry`(name/alias 唯一)解析 slash → 能力门控 → 分发为 `CommandEntry::{OpenMenu,LocalAction,AppUiAction,PromptTemplate}`。**新菜单 = 实现 `MenuProvider` + 在 `core_menu_registry` 注册**。
4. **capabilities 门控(`menu/availability.rs`)** —— `CapabilitySet` 由服务器握手 `UiProtocolCapabilities` 构造;未广告的 method/feature 使命令渲染为 **Unsupported/Disabled 而非被探测**。这是 TUI "渲染服务器真相、不自作主张" 不变量的机器实现。
5. **i18n(`locales/en.yml` 1227 行 / `zh.yml` 1186 行)** —— `rust_i18n::i18n!("locales", fallback="en")`。**新文案 = en.yml 加 key + zh.yml 加翻译**;`/lang` 运行时切换并重建菜单。
6. **Transport driver** —— `AppUiBackend` 3 方法 trait(`bootstrap/send/next_event`);protocol 后端用 `ProtocolTransportDriver` 枚举统一 WS 与 stdio,跑同一 `ProtocolExchange`。

---

## 3. 里程碑现状与未竟工作(接手的工作来源)

### 3.1 里程碑时间线

里程碑号**非时序**;M9.x 是 TUI 内部轨,M12–M22 是 octos 跨仓里程碑的 TUI 切片。下表 "真实状态" 由代码/测试/commit 推导,**不信任各 doc 过期的 `Status:` 头**。

| 里程碑 | 目标 | 真实状态(doc 头) | 契约测试 |
|---|---|---|---|
| **M9.31** 上下文附件 | `turn/start` 携带结构化 diff-hunk/file/selection/review-comment | **部分**:仅 v1 文本桥(`[`/`]`+`c`)落地;结构化 UPCR 仍 proposed(doc 头 `proposed` ✓) | 由 `appui_ux_fixture.rs` 的 diff_context 覆盖 |
| **M9.33** 视觉对等 harness | tmux 7-state 矩阵 + 严格模式发布门 | **可执行 harness 在父仓 octos**;本仓只拥有断言(doc `accepted`) | 断言喂给父仓 harness |
| **M9.34** 菜单框架 | Codex 风格可复用 registry + selection/multi-select/nested 菜单 | **已完成**(doc 头 `proposed`,严重滞后)。`src/menu/` 全套 + 28 命令注册 | inline `src/menu/*` 单测 |
| **M12** Solo + 权限 | 无 OTP 本地 solo(`profile/local/create`)、cwd 门控、`/permissions` 服务器确认 | **完成(E/F 切片)**(doc `draft contract`) | `m12_solo_permissions`(2)、`m12_workspace_cwd_gating`(6) |
| **M13** 受监管任务巡检 | `task/artifact/list`+`read` + 受监管元数据 | **完成** | `m13_supervised_task_inspection`(4)、`review_start`(3) |
| **M15** 编码自治 | `/agents //goal //loop` over AppUI,门控 `coding.autonomy.v1` | **完成**(含 live tmux #113) | `m15_autonomy_constants`(4)、`m15_autonomy_dispatch`(7) |
| **M16** 上下文生命周期 | `context/compaction_completed`+`normalization_reported` 折进 per-session ledger | **完成**(#62/#65) | `m16_context_lifecycle`(4) |
| **M18** stdio live flake 预算 | 重复跑 tmux 打真实 `octos serve --stdio`,flake-budget 记账 | **完成(scaffold)**;不合成 live 证据 | shell `self-test` lane |
| **M22** Solo onboarding | 首启向导**扩展现有 onboarding 面**(禁止并行向导) | **进行中(当前焦点)**:基线已落,June 向导重设计 #191/#195;live UX soak(#55)推迟为人工 | 复用映射到 `store.rs::tests` + `app.rs::tests`(inline) |

无映射但通过的契约测试:`session_hydrate`(3,UPCR-2026-009)、`turn_state`(4,UPCR-2026-011)。**10 个契约测试文件全过,共 37 个断言用例**。**无 M23+**(grep docs/tests/commits 仅命中已存在的 M19 UX-run-bundle lane)。

### 3.2 Gaps / TODO 清单(按可执行性三类分组——这是 AlexZ 的决策维度)

#### A. TUI 本地、现在就能开工

- **[A1] god-reducer 拆分(最大可维护性债)**:`impl Store` 容纳全部 ~40 `dispatch_*` + ~35 `apply_*`,无子模块切分。`store.rs` 17.9k 行是全仓最大文件(第二是 `app.rs` 10.5k)。可行切法:按域拆 `store/{autonomy,onboarding,permission,mcp,tool,task}.rs`,`impl Store` 分散到各文件(Rust 允许同一 `impl` 块跨文件以自由函数/子 impl 形式拆)。
- **[A2] 大文件导航成本**:`store.rs`/`app.rs`/`providers.rs`/`model.rs`/`transport.rs` 均 7k–18k 行。接 [A1] 一起做。
- **[A3] `keymap.rs` 名实不符 / SaveKeymap 空壳**:`keymap.rs` 仅 1 行 `HELP` 常量;`LocalAction::SaveKeymap` + `MENU_KEYMAP` provider 是 "保存 keymap" 扩展点的**空壳**——无独立 keymap 配置/持久化层,无冲突检测(M9_34 doc:329-340 要求保存前检测绑定冲突、确认后才持久化)。
- **[A4] `doctor` live-WS 探测仍是 TODO**(`cmd/doctor.rs:756-919):doctor 不真正握手 WebSocket,只记录配置端点。是全仓 4 个 TODO 的集中地。
- **[A5] 错误处理收敛**:536 处 `.expect(`、68 处非测试 `.unwrap()`,部分在启动期 "不变量" 路径(如 `CommandRegistry::register` 的 `.expect("unique")`)。缺统一收敛策略。
- **[A6] 命令面膨胀**:`AppUiCommand` 已 67 变体,远超 ARCHITECTURE.md:107-117 宣称的 7 个 "稳定核心";契约表面增长快,缺收口纪律。
- **[A7] 陷阱:`autonomy.rs` stale 注释**(已核实):autonomy.rs:5-6 称 `agent/list`/`session/goal/set`/`loop/create` 的 dispatch "wired in a later PR",但 `store.rs:368-388` **早已解析并 dispatch** `/agents //goal //loop`。读注释会误判实现状态——接手前先清理这类 stale 注释。
- **[A8] composer 随原生滚动消失(活跃用户痛点,方案已定,spec 已落)**:inline + 原生 scrollback 模型下,用户用终端滚轮/滚动条回看历史时,底部 composer 必然随整屏滚出可视区——终端查看自身 scrollback 时**没有任何转义序列能固定屏幕区域**,inline 模式架构上不可修(DECSTBM 只影响程序输出时的滚动)。已定方案:**alt-screen 全屏 transcript pager**(codex CLI Ctrl+T 同款),复用现成的 `wants_fullscreen_overlay`(app.rs:83)切换机制 + `render_chat_layout`(app.rs:736,composer 本就钉底)+ `render_transcript` 内部滚动(app.rs:1408);完整 transcript 数据一直在 `AppState`(scrollback 只是渲染去向,不是数据去向)。**spec:`specs/task-transcript-pager.spec`(8 场景,lint 100%);已于 2026-06-10 按 spec 实现并通过全部门禁(lifecycle 9/9、全量 cargo test 零回归)**。刻意不选 "普通 chat 改常驻 alt-screen 内部滚动"——那会撤销 inline+scrollback 这笔刻意投资(event_loop.rs:38-41 记录了旧 alt-screen 模型 "定频重绘抹选区" 的病根),且滚轮要 `EnableMouseCapture` 会杀掉原生选择/复制。

#### B. 服务器阻塞(等后端广告 method/feature 才能解门)

- **[B1] 权限**变更**仍被禁用**(最主要的 server-backed gap):`/permissions` 渲染 Codex 风格行(Default/Read Only/Workspace Write/Full Access/network)但**只读**;profile 切换与 scope 清除被 `permission_action_disabled_reason`(providers.rs:3673-3683)以 "typed command missing for profile/set / profile/list" 门住,直到服务器广告 `permission/profile/set`、`permission/profile/list`、`approval/scopes/clear`。
- **[B2] `config/capabilities/list`** 无显式 AppUiCommand 变体;capabilities 现从 feature/method token 推断。follow-up 是加该 method 并把 map 存进 menu context。
- **[B3] `/status` 权威刷新**:`session/status/read`(当前 model/provider、usage、server version/build、连接/replay 健康)仍是 follow-up;今天 `/status` 只渲染快照。
- **[B4] M9.31 结构化附件**:完整 UPCR(diff_hunk/file_path/text_selection/review_comment)proposed,非 protocol 1.0;需协议版本广告 + 不支持类型的 typed reject + transcript 持久化决策。

#### C. 跨仓 / 手动流程(进程性,非纯编码)

- **[C1] core-pin 升级 + 契约 diff**(见 §4、§6):`Cargo.toml:21` 手动 pin `rev="2afff187"`;**已核实** octos HEAD `13e4623b` 比 pin 领先 21 commit,但其中 **0 个触及 `crates/octos-core`**——所以协议契约**今天没漂移**,但靠手动 pin + PR 不编译维系,fragile by construction。接手须含 "bump pin → diff octos-core → 本地跑契约测试" 步骤。
- **[C2] CI 质量门禁缺口**(headline,见 §4):PR 上不编译/不 test/不 clippy/不 fmt。
- **[C3] M9.33 父仓 harness 补丁未完**:`octos/scripts/compare-tui-coding-ux-tmux.sh` + `octos/e2e/tmux/run.sh` 需打补丁断言 7-state 矩阵、保存 captures、噪声 trace 即失败、加 strict/non-strict 模式。octos-tui 只拥有断言。
- **[C4] live-soak 门全推迟为人工**:M22 live UX soak(#55)、1 小时 live AppUI soak(WS+stdio 全矩阵:reconnect-after-replay-loss/interrupt/typed approval+denial/窄终端重叠)、M12-G 交互式 tmux soak(strict 模式被后端 `profile/local/create`/`permission/profile/*`/`mcp/config/*`/`tool/config/set_enabled` 阻塞)。本地 PTY lane "仍不能替代完整 live soak"。
- **[C5] update/doctor P4**(DESIGN_update_doctor.md:143-151):抽共享检查进 `octos-core::diagnostics`、重构 `octos-cli/src/updater.rs`(现 macOS-aarch64-only、无 install-method 感知、会覆盖 brew/distro 二进制)、给 server 二进制加 `octos doctor`/`octos update`。**协议-skew 检查(pinned core vs live server capabilities)是承重差异点**。
- **[C6] 无 release-please/changelog 自动化**:版本 bump、core-rev bump、Cargo.lock 同步全手动多步,易漏。

> **服务器端契约说明(非 TUI gap,但接手须知)**:`.octos-workspace.toml`(**已核实存在于 octos-tui**)定义了 `[spawn_tasks.deep_search/...]` 的 `on_completion` 验证契约(file_exists/magic_bytes/audio_non_silent/http_probe 等)。这是 **session-workspace 验证契约,由后端 runtime 执行**——TUI 只把它作为 workspace 工件携带,自身不跑这些校验。写 TUI 的 spec 时**不要**把它当 TUI 待办;它属于后端 octos 的 spawn-task 验证面。

---

## 4. 开发工作流与工程约定

**分支/PR/merge**:feature 分支 → 编号 PR → merge。前缀 `feat/ fix/ chore/ docs/ codex/`;commit 带 `(#NNN)`,常见双编号 `(#issue) (#PR)`。

**spec-first(里程碑文档 → 契约测试 → 实现)**——三层证据:
- README.md:353-358 明文:协议变更须走 **formal UI Protocol change request**,顺序为 shared types → server tests → golden tests → TUI reducer/rendering tests。
- 里程碑/设计文档先行(如 `docs: design for octos-tui update+doctor` 先于实现)。
- 契约测试头注释 pin issue/UPCR 表面契约(`turn_state_contract.rs:1-6` = UPCR-2026-011;`session_hydrate_contract.rs:1-6` = UPCR-2026-009/#154),并断言常量字面值匹配 feature-flag(`assert_eq!(APPUI_FEATURE_TURN_STATE_GET_V1, "state.turn_state_get.v1")`)。

**UPCR 编号体系**:`UPCR-YYYY-NNN`,主仓 octos 持有。已见 -009/-011/-016/-021/-023。

**codex review gating**:由名为 "codex" 的 reviewer 审查,findings 按 `codex P1/P2/review` 标注并以单独 commit 回填(如 `fix(tui): … (codex P2)`);有专门 `codex/verify-*` 分支做活体产物核对。**注意:codex review 是流程约定,不是 CI/branch-protection 强制门——无机器可验证的 "codex 已通过" 状态闸。**

**测试金字塔(5 层,快→慢)**:
1. **Rust 契约测试**(`tests/*.rs`,mock-backed,无需 server):6 个直接 import `octos_core::ui_protocol` 类型,断言 reducer/dispatch + capability gating + 常量对齐 UPCR。
2. **JSON fixture UX-parity**(`appui_ux_fixture.rs` + `fixtures/`):断言 WS 与 stdio 归一到相同语义。
3. **PTY 捕获 + validate 脚本**(`capture-appui-ux-pty.sh` ~24 markers;`validate-tmux-ux-capture.sh`)。
4. **tmux 活体 soak + flake-budget**(`run-m18-stdio-live-tmux-soak.sh`;`run-onboarding-tmux-soak.sh` 209KB)。
5. **fake-openai-server.py mock**(132 行,无真 LLM 时驱动后端)。
   harness 自带**负向自检**:`validate-appui-ux-fixture.sh:10-18` 断言已知坏 fixture **必须**失败,否则 exit 1。

**CI 缺口(headline)**:`.github/workflows/` 下**只有 `release.yml`**(cargo-dist 生成,纯发布:tag push → 4-target 二进制 → GitHub Release + npm + Homebrew)。**PR 上 `pr-run-mode="plan"` + build-job if-guard ⇒ 只跑 `dist plan`,不编译/不 test/不 clippy/不 fmt**。无 `ci.yml`。结论:**任何 PR 都能在零自动化验证下 merge**;契约测试/clippy/fmt 全靠开发者本地自觉。**这正是 agent-spec `guard` 要填的洞。**

**跨仓依赖**:`octos-core = { git=…, rev="2afff187" }`(手动 pin);本地 live dev 用 gitignored `.cargo/config.toml`(`paths=["../octos/crates/octos-core"]`)覆盖。

**构建约定**:edition 2024 / rust 1.85.0;`unsafe_code="deny"`;纯 rustls 无 OpenSSL;`eyre`/`color-eyre`;`update` feature 默认开(distro 可关);dist `lto="thin"`。**display 重命名 #180**:用户可见 "AppUI" → "Octos UI",但**内部代码标识符仍 `APPUI_*`/`AppUiCommand`**。

---

## 5. agent-spec 接入方案

### 5.1 agent-spec 是什么 + 真实结构 + CLI 生命周期(以已验证事实为准)

**是什么**:`agent-spec --help` 自述为 "AI-Native BDD/Spec verification tool"。人写**结构化 Task Contract**(`.spec.md`),agent 照着实现,机器**确定性地**验证——每个 BDD 场景经 `Test:` selector 绑一个 cargo 测试。核心价值是 "review-point displacement":人审 50–80 行契约,而非 500 行 diff。**已装 CLI 是 v0.2.7**(`~/.cargo/bin/agent-spec`,已核实);源码仓是 v0.3.0(`~/Work/Projects/FW/rust-agents/agent-spec`)。**以下行为以 0.2.7 实测为准。**

**`.spec.md` 真实结构 = YAML frontmatter + 章节头(中英双语,一行一种语言)**:

- frontmatter:`spec:`(`org|project|task`,必填)、`name:`(必填)、`inherits:`(父 spec,org→project→task 三层继承)、`tags:`、`depends:`(列表,驱动 `graph` DAG)、`estimate:`(`0.5d`/`1d`/`2d`/`1w`/`4h`,驱动关键路径权重)。
- 真实章节头(**硬规则:一行一种语言,`## Intent` 或 `## 意图`,绝不 `## Intent / 意图`**):
  - `## Intent` / `## 意图` —— 2–4 句:做什么 + 为什么。
  - `## Constraints` / `## 约束`(带 `### 必须做`/Must、`### 禁止做`/Must-NOT 子头,多用于 org/project 层)。
  - `## Decisions` / `## 已定决策` —— 已固定的技术选择;**每条 Decision 应有 ≥1 个场景覆盖**(`decision-coverage` linter)。
  - `## Boundaries` / `## 边界`:`### Allowed Changes`(path glob,**BoundariesVerifier 机械强制**)、`### Forbidden`(自然语言,仅 lint 检查)、Out-of-scope。
  - `## Acceptance Criteria` / `## Completion Criteria` / `## 验收标准` / `## 完成条件` —— BDD 场景体(parser 在 `parse` 输出里统一归一为 "Acceptance Criteria")。
  - `## Out of Scope` / `## 排除范围`。
  - **不要发明 `## Architecture`/`## Milestones`/`## Quality` 等顶层节——parser 会拒。**
- **BDD 场景语法**:`Scenario:`/`场景:`、`Test:`/`测试:`(selector)、`Given`/`假设`、`When`/`当`、`Then`/`那么`、`And`/`并且`、`But`/`但是`;支持 step 表格。
- **Test selector 两形**:简单 `Test: test_name`;或结构块 `Test:` → `Package: <crate>` + `Filter: <name>`。
- **场景扩展**:`Tags: critical`/`标签: critical`(置 `gate_blocked` JSON 字段)、`Review: human`/`审核: human`(→ `pending_review`)、`Mode: optimize`、`Depends:`(执行排序)。
- **核心作者原则**:**异常场景 ≥ happy-path 场景**(`error-path` linter 强制);每个 spec **3–8 个场景**(更大的拆成多个用 `depends:` 串)。

**CLI 生命周期(7 步 + 实测退出码)**:
`init → 填契约 → lint(质量门)→ contract(agent 读计划)→ 实现 → lifecycle(验证,重试环)→ guard(CI)→ explain(人验收)→ stamp(git 溯源)`。各命令:

| 命令 | 作用 | 默认/关键 |
|---|---|---|
| `parse <spec>` | 显 AST,`Acceptance Criteria: N scenarios` | 非零场景数才合格 |
| `lint <files> [--min-score X]` | 质量 smells:vague-verb/testability/coverage/determinism/decision-coverage/error-path… | lifecycle/guard 阈值 0.6;authoring 自检 0.7 |
| `contract <spec>` | 渲染 Task Contract 给 agent | 优于 legacy `brief` |
| `verify <spec> --code <dir>` | 原始验证,**无 lint 门** | `--change-scope` 默认 `none` |
| `lifecycle <spec> --code <dir>` | **主门**:lint → StructuralVerifier → BoundariesVerifier → TestVerifier(跑绑定测试) | `--change-scope none/staged/worktree`,`--format` 默认 json |
| `guard [--spec-dir specs] [--code .]` | 仓级:lint+verify 全部 spec,**给 pre-commit/CI** | `--change-scope` 默认 `staged` |
| `explain <spec> [--format markdown]` | 人读的契约验收摘要(贴进 PR) | 替代 "读全 diff" 的 review 工件 |
| `stamp <spec> --dry-run` | 预览 git trailer `Spec-Name/Spec-Passing/Spec-Summary` | **仅支持 `--dry-run`** |
| `graph --spec-dir specs [--format dot/svg]` | 从 `depends`/`estimate` 出 DAG,关键路径红线 | — |

**实测退出码(0.2.7,已验证,修正 SKILL 文档)**:
- `parse` 缺文件 → **1**。
- `lint` 低于 `--min-score` → **1**;**任何 error-level 问题(如场景缺 `Test:` selector)→ 1,即便 `--min-score 0.0`**;干净且达阈值 → 0。
- `lifecycle` 通过 → 0;**未通过 → 1**(含 critical-tagged 失败也是 **1,不是 2**)。
- `guard` 全过 → 0;任一失败 → 1。
- **关键修正**:agent-spec 的 `main()` 只映射 `Ok→0 / Err→1`,**自身逻辑无 `process::exit(2)`**。SKILL 文档里的 "critical → exit 2" 指的是 **`gate_blocked` JSON 字段,不是进程退出码**(clap 自身对 CLI 用法错误仍按默认 exit 2,与验证无关)。
- **Test selector 绑定**(读自 `test_verifier.rs:193-201` 的 `build_cargo_test_args`):`Test: test_foo` → `cargo test -q test_foo`;结构块 `Package: octos-tui` + `Filter: test_bar` → `cargo test -q -p octos-tui test_bar`。`Filter` 是 cargo 子串名匹配。

> **6 个 verdict,皆不同**:pass / fail / skip / uncertain / pending_review / gate_blocked。**`skip != pass`**——未绑定/跳过的场景会让 lifecycle 退 1。

### 5.2 与现有 "里程碑文档 + 契约测试 + codex review" 工作流的逐项映射

octos-tui **已经在跑** agent-spec 形式化的那套(已核实:`docs/M*.md` 里程碑契约 + `tests/m*_*_contract.rs`),后端 octos 也已先行接入。映射:

| octos-tui 现有 | agent-spec 对应 | 说明 |
|---|---|---|
| 里程碑 doc `## Goal`/概述散文 | `## Intent` | M12 的 "Goal" 已是这形状 |
| doc 的固定选择(AppUI method 列表、请求字段、`-a never` 语义) | `## Decisions` | 每条 Decision 给一个覆盖场景(decision-coverage) |
| doc 的 UX 原则 / "must not"(如 "TUI 不是 runtime-policy 权威"、"不得要求 auth/send_code") | `## Constraints`(Must/Must-NOT) + `### Forbidden` | 自然语言 → lint 检查 |
| 里程碑可触碰哪些文件 | `## Boundaries` → `### Allowed Changes`(`src/**`、`tests/m12_*`) | **BoundariesVerifier 机械比对改动文件** |
| 契约测试断言(`tests/m12_solo_permissions_contract.rs::*`) | BDD `Scenario:` + `Test: <fn>` selector | 一行为一场景,按测试函数名绑 → `cargo test -q <fn>` |
| 跑 `cargo test`/脚本验证里程碑 | `agent-spec lifecycle <spec> --code . --change-scope worktree` | 一次门内含 lint+boundary+test,失败 exit 1 |
| **codex review / 人审 diff** | `agent-spec guard`(机械门)+ `explain --format markdown`(人验收工件) | **guard = 自动门;explain = review 工件** |
| 里程碑 "done" 签收 | `agent-spec stamp --dry-run` 的 `Spec-Passing: true` git trailer | 契约→commit 溯源 |

**诚实地标出非 1:1 处**:agent-spec 的 TestVerifier **只检查绑定测试是否通过,不评判代码质量**(不像 codex review 读 diff 的写法)。所以 **codex/人审 diff 仍互补**——agent-spec 替换的是 "契约行为是否被验证",**不是** "代码写得好不好"。这二者叠加才完整:guard 管 "契约达成"(并补上目前缺失的 PR 门禁),codex review 继续管 "代码质量"。

### 5.3 落地步骤

**目录布局**(对齐 octos 先例 + agent-spec 自托管规则):
```
octos-tui/
  specs/
    project.spec.md          # octos-tui 全仓规则,写一次,task spec inherits: project
    task-<gap>.spec.md        # 一个 gap 一个 spec
    roadmap/                  # 未激活的未来里程碑,激活时提升进 specs/(默认 guard 不扫)
  .agent-spec/runs/           # --run-log-dir,gitignore
```
`project.spec.md` 一次性编码:**"TUI 渲染服务器真相、永不作 policy 权威"**;"无正当理由不加新 crate 依赖";"每个里程碑场景必须绑显式 `Test:` selector";model 选择事实(profile 驱动、`_main` 被拒、`turn_id` 必须 UUID,来自项目记忆)。task spec 全部 `inherits: project`。

> **状态更新(2026-06-10,全链路已实跑)**:`specs/` 已落地——`project.spec`(全仓不变量 + 3 个绑既有测试的不变量冒烟场景,lint 76%)+ 首个试点 `specs/task-transcript-pager.spec`([A8],lint 100%、lifecycle 9/9 含 worktree 边界检查、guard 2 specs passed、stamp `Spec-Passing: true`,**已按 spec 实现并落地 8 个契约测试**);`.agent-spec/runs/` 已入 `.gitignore`。**0.2.7 实测勘误与陷阱(读自 `~/.cargo/registry/.../agent-spec-0.2.7` 源码并实跑验证,部分修正本文上方 §5.1/§5.3 的 `.spec.md` 表述)**:
> 1. **扩展名契约:`guard` 只扫描 `*.spec` 裸后缀**(`main.rs:661` 按 `extension == "spec"` 过滤),`inherits:` 也只按 `{name}.spec` 文件名解析(`resolver.rs`)。`.spec.md` 文件对 parse/lint/lifecycle 直接调用有效,但**对 guard 不可见、也无法被继承**——本仓 specs 因此用 `.spec` 后缀(octos 先例的 `.spec.md` 是孤立 spec,不走 guard/继承,故未暴露)。
> 2. **project 级 spec 会被 guard 正常 lint+verify,无级别豁免**:零场景 → lint 0 分 + "0 passed" 双挂。解法 = 给 project spec 加少量绑定**既有**确定性测试的不变量冒烟场景(本仓绑了 m12 权限真相、i18n 双语解析、inline 鼠标捕获三条)。
> 3. **BoundariesVerifier 两个坑**:`--code` 必须传**绝对路径**(传 `.` 时 worktree 改动会整组误判 "not covered");不含 `/` 或 `*` 的条目(如 `.gitignore`)被 `looks_like_path_boundary` 整行忽略,要写成 `**/.gitignore`。Allowed Changes 应包含 `specs/**` 与本文档自身——spec-first 工作流的 diff 天然含 spec/文档。
> 4. frontmatter 可不带开头 `---` 围栏(以 `spec:` 开行、仅尾部 `---`,parse 正常)。
> 5. 测注意:管道后 `$?` 取到的是 tail/grep 的退出码——验证 agent-spec 退出码时不要接管道。

**粒度:一个 gap → 一个 spec**(3–8 场景;更大的拆多个用 `depends:` 串)。octos-tui 的 `m<NN>` 里程碑天然映射:如 `specs/task-solo-permissions.spec.md` 绑 `tests/m12_solo_permissions_contract.rs` 的测试函数。`### Allowed Changes` 保持收紧(octos 的 matrix spec 用了单文件 Allowed Changes)以让边界机械可验。

**每个 gap 的配方**:
1. `agent-spec init --level task --lang zh --name "<里程碑>"`(parity/port 类如 M9.33 用 `--template rewrite-parity`)。
2. 填 Intent(取自里程碑 `## Goal`)、Decisions(AppUI method/字段)、Boundaries(`src/**`、`tests/m<NN>_*`)、Acceptance Criteria(每个现有/计划契约测试函数一个 `Scenario:` + `Test: <fn>`;**异常场景 ≥ happy-path**)。
3. `agent-spec parse <spec>`(非零场景)→ `agent-spec lint <spec> --min-score 0.7`(**须 exit 0**:补 vague verb、补 `Test:` selector、覆盖 Decision)。
4. 让 agent 照 `agent-spec contract <spec>` 实现。
5. `agent-spec lifecycle <spec> --code . --change-scope worktree --run-log-dir .agent-spec/runs` —— exit 1 = 绑定测试失败/跳过,**改代码不改 spec**(重试协议)。注意 lifecycle 会跑 `cargo test`,**octos-tui 工作区必须能编译**。

**git/PR/CI 集成**:
- pre-commit:`agent-spec guard --spec-dir specs --code . --change-scope staged`(默认 scope)。可 `agent-spec install-hooks` 自动接线。
- **CI(给 octos-tui 新建 `ci.yml`,填 §4 的 headline 缺口)**:`agent-spec guard --spec-dir specs --code . --change-scope worktree`,exit 1 即 fail job。同一 job 里建议**先 `cargo test --workspace` / `cargo clippy` / `cargo fmt --check`**(这些 octos-tui 现在 PR 上完全没跑),再跑 guard——guard 是里程碑契约门,基础门也得补。可对齐 octos 的 `scripts/milestone-ci.sh` 模式。
- PR body:贴 `agent-spec explain <spec> --code . --format markdown` 输出(契约验收工件,替代读全 diff)。
- commit trailer:`agent-spec stamp <spec> --dry-run` → 贴 `Spec-Name/Spec-Passing/Spec-Summary`。

**estimate 用法**:frontmatter 加 `estimate:` + `depends:`;sprint 前 `agent-spec graph --spec-dir specs --format svg` 看关键路径。effort 模型:每场景 ≈ 1 模块(1–15 轮),乘风险系数(决策已固定=1.0,vague/缺失=1.3–1.5),加集成(~10–15%)+ 验证(`ceil(场景数/3)`)轮,再 轮×~3min。事后用 `agent-spec explain <spec> --history`(预测 vs 实际重试)校准。**octos-tui 里程碑因 AppUI 决策紧 + 已有契约测试,会估出 high-confidence / 低轮数。**

**版本决策点(需在 §5.3 拍板)**:CI 用**已装的 0.2.7** 还是 `cargo install --path` 源码 **0.3.0**?0.3.0 可能新增子命令/flag(`plan` 在 0.2.7 已有,但 0.2.7↔0.3.0 行为差异**未逐一 diff,需核实**)。建议 CI pin 一个明确版本以保可复现。

### 5.4 示例:把一个真实 gap 写成 `.spec.md` 骨架

选 **[B1] 权限变更门控**(§3.2)。它满足三个判别条件:① 真实开放 gap;② 绑**确定性 fixture/契约测试**(非 M18 flaky stdio-live);③ TUI 侧可执行(门控逻辑在 TUI,即便 mutation 等后端,**"未广告时禁用并给出原因" 这条今天就能验**)。下面用**已核实存在**的两个测试函数作 `Test:` selector(`tests/m12_solo_permissions_contract.rs::solo_onboarding_fixture_uses_local_profile_create_without_otp`、`::permissions_fixture_requires_server_confirmed_dangerous_status`);新增的 "禁用/解禁" 场景需对应**新写**确定性测试函数(下方标注 `<需新增测试>`)。

```markdown
---
spec: task
name: 权限菜单能力门控
inherits: project
tags: [m12, permissions, capability-gating]
depends: []
estimate: 1d
---

## 意图

`/permissions` 菜单按服务器广告的能力门控权限变更动作:服务器未广告
`permission/profile/set` / `permission/profile/list` / `approval/scopes/clear`
时,profile 切换与 scope 清除必须渲染为禁用并给出明确原因(只读巡检);
广告后才解禁。TUI 永不本地伪造权限真相,只渲染服务器确认的状态。

## 已定决策

- 权限变更走 AppUI 方法:`permission/profile/set`、`permission/profile/list`、
  `approval/scopes/clear`;均由服务器 capabilities 广告门控。
- 危险状态行(Full Access / network)必须由服务器确认(server-confirmed
  dangerous status),不得本地推断。
- solo onboarding 用 `profile/local/create`,无 OTP。

## 约束

### 禁止做
- 禁止在服务器未广告对应 method 时本地探测或本地切换权限 profile。
- 禁止把 `-a never` 与 full-access 混同——二者语义必须区分。

## 边界

### Allowed Changes
- src/menu/providers.rs
- tests/m12_solo_permissions_contract.rs

### Forbidden
- 修改 transport/JSON-RPC 帧逻辑。
- 引入新的 crate 依赖。

## 验收标准

场景: solo onboarding 使用本地 profile 创建且无 OTP
  测试: solo_onboarding_fixture_uses_local_profile_create_without_otp
  假设 fixture 描述一次 solo onboarding 流程
  当 客户端归约该 fixture
  那么 它使用 `profile/local/create` 且不触发 auth/send_code

场景: 危险权限状态要求服务器确认
  测试: permissions_fixture_requires_server_confirmed_dangerous_status
  假设 服务器返回包含危险状态的权限 fixture
  当 客户端渲染权限菜单
  那么 危险行只有在服务器确认时才标记为 dangerous

场景: 服务器未广告 profile/set 时禁用 profile 切换   # <需新增测试>
  测试: permission_profile_switch_disabled_when_capability_absent
  假设 capabilities 不包含 `permission/profile/set`
  当 客户端构建权限菜单
  那么 profile 切换项渲染为 disabled
  并且 禁用原因为 "typed command missing for profile/set"

场景: 服务器未广告 approval/scopes/clear 时禁用 scope 清除   # <需新增测试>
  测试: scope_clear_disabled_when_capability_absent
  假设 capabilities 不包含 `approval/scopes/clear`
  当 客户端构建权限菜单
  那么 scope 清除项渲染为 disabled
  但是 只读巡检项仍可用

场景: 服务器广告全部权限能力时解禁变更   # <需新增测试>
  测试: permission_mutations_enabled_when_all_capabilities_present
  假设 capabilities 包含 profile/set、profile/list、approval/scopes/clear
  当 客户端构建权限菜单
  那么 profile 切换与 scope 清除均为 enabled
```

> 该骨架:5 个场景(3 异常/边界 ≥ 2 happy-path,过 `error-path` linter);3 条 Decision 均有覆盖场景(过 `decision-coverage`);`### Allowed Changes` 收紧到两文件(BoundariesVerifier 可机械验)。落地前先 `agent-spec lint --min-score 0.7` 跑过,再补齐 3 个 `<需新增测试>` 函数,然后 `lifecycle … --change-scope worktree`。

---

## 6. 风险与建议(含完整性批判发现的遗漏点)

**R1 — 协议契约 forked,非 shared(最大风险,修正 ARCHITECTURE.md 的声称)**。ARCHITECTURE.md 称 "client 消费 octos-core 共享类型、新特性应先落 octos-core AppUI 类型" ——**部分不成立**。已核实的精确三类画面:
- **method 名*字符串*是共享常量、编译期强制**:`AppUiCommand::OpenSession(_) => octos_core::ui_protocol::methods::SESSION_OPEN`(model.rs:665+)——这些 wire 方法名来自 octos-core。
- **但 `AppUiCommand` 枚举本身是 TUI 本地定义**(model.rs:591,**67 变体**);octos-core 有它**自己的** `AppUiCommand`(app_ui.rs:113,变体少得多)。transport 用的是 `crate::…AppUiCommand`(本地),**不是** `octos_core::app_ui::AppUiCommand`。
- **param 结构三类并存**:(a) 从 `octos_core::ui_protocol` import = 编译强制;(b) **TUI-local-only、无 octos-core 对偶**(如 `ConfigCapabilitiesListParams`/`SessionStatusReadParams`/`ModelListParams`/`McpStatusListParams`/`AuthVerifyParams`)= 纯 raw-wire,零跨仓编译保护;(c) **重复定义**——`ProfileLocalCreateParams` **同时**存在于 `src/model.rs:1703` 和 `octos/.../ui_protocol.rs:2014`,**仅靠 serde 约定兼容,无共享真相**。**类 (c) 是最尖锐风险**:任一侧改字段名都编译通过、wire 静默 break。**建议**:把 [C1] 升级为强制流程——bump pin 后 `diff` octos-core 协议头、本地跑全部契约测试;长期考虑把 TUI-local param 结构往 octos-core 收(或反向),消灭类 (b)/(c)。

**R2 — pin 漂移机制脆弱(今天干净,构造上 fragile)**。已核实:pin `2afff187` 比 octos HEAD `13e4623b` 落后 21 commit,但**其中 0 个触及 `crates/octos-core`**——**契约今天没漂移**。但靠 "手动 bump pin + PR 不编译" 维系:协议在主仓演进后,要等有人手动 bump rev 并本地跑测试才会发现 drift。**无任何自动化(CI matrix / dependabot 式 bump)保证 pinned rev 与主仓协议头对齐**。建议在 CI 加一个 "core-rev 落后告警" + 契约测试针对最新 octos-core 跑一次的 lane。

**R3 — CI 零质量门禁**(§4 headline)。PR 不编译/不 test/不 clippy/不 fmt。这是 agent-spec 接入的**首要收益点**,但也意味着接入 agent-spec **必须同时新建 `ci.yml`** 把基础门(cargo test/clippy/fmt)+ `agent-spec guard` 一起补上,否则只是把契约门加在一个连 "能否编译" 都不验的流水线旁。

**R4 — 完整性批判发现的遗漏点(已逐一核实)**:
- **stale-comment 陷阱**([A7]):`autonomy.rs:5-6` 称 dispatch "wired in a later PR",但 `store.rs:368-388` 早已 dispatch ——读注释会误判实现状态。**接手前清理此类 stale 注释**,避免基于过期注释的误工。
- **最活跃子系统是终端渲染、非协议**:近期 churn 集中在 inline viewport + scrollback(`viewport.rs`/`insert_history.rs`/`tui_terminal.rs`,纯终端关切:native-bg scrollback、cursor 重放、OSC52),容易被 "协议/里程碑" 视角漏掉——若接手要碰渲染,这是真实热点。
- **inline "滚动已接好" 是误读**([A8] 配套陷阱,已核实):`MouseEventKind::ScrollUp/Down` 的处理(event_loop.rs:479)在 inline 模式是**死代码**——全仓无任何 `EnableMouseCapture` 调用,crossterm 根本收不到鼠标事件,只有测试在走它;且 inline 渲染路径的 `transcript_scroll` 只作用于 live tail(app.rs:260),已提交历史不在渲染树里,**键盘 PageUp 也滚不到历史**。读这两处代码易误判 "App 内滚动已可用"。
- **`.octos-workspace.toml` 是后端契约**:已核实存在于 octos-tui,带 `[spawn_tasks.*]`/`[validation]`/`[artifacts]` 验证面,但**由后端 runtime 执行**,**不是 TUI 待办**——写 spec 时勿误纳。

**R5 — agent-spec 自身风险**:
- **版本 skew**:已装 0.2.7 vs 源码 0.3.0;退出码/字段以 **0.2.7 实测为准**(本文 §5.1)。**修正(2026-06-10 核实):`~/Work/Projects/FW/rust-agents/agent-spec` 源码路径已不存在,本机仅余已装的 0.2.7 二进制**——CI 应直接 pin 0.2.7,或先找回 0.3.0 源码仓再决定升级。
- **未亲手验证项(需核实)**:未在 octos-tui 实跑过 `explain --format markdown`/`stamp --dry-run`/`graph`(specs/ 已存在,现在可跑);`--ai-mode caller`/`resolve-ai`/`--resume` 仅据 SKILL 文档;**未确认 octos-tui 全量 `cargo test`(lifecycle 会调)在 agent-spec 验证环境干净编译/运行**。**第一步落地动作已完成**(见 §5.3 状态更新):[A8] spec 已实现、`tests/transcript_pager_contract.rs` 8 个测试函数已落地,完整 lifecycle(含 worktree 边界检查)9/9 通过,`Test:` selector → cargo 测试函数的绑定链路已验证可用。
- **flaky 测试不可绑场景**:M18 stdio-live 测试有 flake budget,**绝不**把 `Scenario:` 绑到 flaky 测试(否则 lifecycle 非确定),只绑确定性 fixture/契约测试(`appui_ux_fixture.rs`、`turn_state_contract.rs`、`m1*_*_contract.rs`)。
- **codex review 不可被 agent-spec 替换**:guard 只验 "契约行为被测",不评代码质量;codex/人审 diff 仍是承重环节,二者叠加。

**建议落地顺序**:① **已完成(2026-06-10)**:`specs/project.spec` + 试点 spec 已落地并实现,实际试点选了 **[A8] transcript pager** 而非 [B1]——纯 TUI 本地、零服务器依赖、活跃用户痛点;spec→实现→契约测试→lifecycle/guard 全链路已打通(见 §5.3 状态更新,含 0.2.7 勘误)。[B1] 按 §5.4 骨架作第二个 spec;② 新建 `ci.yml` 把 cargo test/clippy/fmt + `agent-spec guard` 一起补上(填 R3);③ 把 [C1]/R2 的 "bump pin → diff 协议头 → 跑契约测试" 固化为流程并加 CI 告警;④ 再按 §3.2 的 A/B/C 分组,A 类(god-reducer 拆分、keymap 持久化、doctor live-WS、错误收敛)逐个写 spec 推进,B 类等后端解门,C 类排进跨仓协调。