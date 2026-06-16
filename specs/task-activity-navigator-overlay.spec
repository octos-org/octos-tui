spec: task
name: "/activity navigator overlay：搜索 + 状态过滤 + detail"
inherits: project
tags: [tui, rendering, navigator, activity, appui]
depends: [task-geometry-helper, task-unified-hint-bar]
estimate: 1.5d
---

## 意图

herdr 的 Navigator 对 octos-tui 最有价值的点不是 server/client multiplexer，
而是一个可搜索、可过滤、可看 detail 的活动导航面。我们现有 inspector 是固定
三列静态列表：sessions/tasks/artifacts + transcript + plan/workspace/git；它能
查看状态，但不能按 `running/blocked/failed/done` 聚合，也不能跨 session 搜索。

本任务新增一个 `/activity` overlay，把 AppUI 已有真相聚合成 navigator：
sessions、session tasks、turn activity logs、live activity、orchestration、
session usage。它是只读视图，服务于定位"现在谁在跑、谁卡住、哪个 turn 出错"。

## 已定决策

- 数据源只来自现有 `AppState`，不新增后端 RPC：第一版使用
  `sessions[*].tasks`、`activity`、`turn_activity_logs`、`orchestration`、
  `session_usage`、`run_state`。
- 新增 overlay 不替换 inspector：inspector 继续做当前 3 列静态诊断；
  `/activity` 做搜索/过滤/详情。
- 入口优先使用 slash command `/activity`；键盘全局快捷键另行讨论，避免抢占
  composer 输入或现有 inspector Tab 流。
- 模型先行：`ActivityNavigatorModel` 是纯函数，输入 `AppState`、query、filter、
  selected index，输出 rows、counts、detail。渲染层只消费 model。
- 状态过滤第一版支持 `all/running/blocked/failed/done`。其中：
  - running: `TaskRuntimeState::Running/Pending`、running activity、
    active orchestration、`SessionRunState::InProgress`
  - blocked: `SessionRunState::Blocked`、approval/user question visible
  - failed: failed/cancelled task、error activity、`SessionRunState::Error`
  - done: completed task、completed activity、`SessionRunState::Success`
- 搜索第一版做大小写不敏感 substring，字段包括 session title/profile、task
  title/detail/output tail、activity title/status/detail/tool_call_id、turn id。
- detail 面只读：显示所选 row 的来源、session/turn/task id、状态、detail、尾部
  输出预览；不做 cancel/restart/interrupt 操作。
- 不搬 herdr 的鼠标默认捕获、常驻 alt-screen、PTY session tree 或可配置 keybind。

## 边界

### Allowed Changes
- src/model.rs
- src/app.rs
- src/event_loop.rs
- src/menu/** 或 slash command registry（若 `/activity` 入口在菜单层）
- locales/en.yml
- locales/zh.yml
- tests/activity_navigator_contract.rs
- specs/**

### Forbidden
- 不新增 AppUI protocol 方法或后端依赖。
- 不把 `activity` 当作唯一真相覆盖 session/task 真相；只能聚合展示。
- 不改变现有 inspector、pager、inline scrollback 行为。
- 不默认启用鼠标捕获。
- 不引入新 crate 依赖。

## 最小实现计划

1. **状态模型**
   - 在 `model.rs` 增加 `ActivityNavigatorState { active, query, filter, selected }`。
   - 增加 `ActivityNavigatorFilter` enum：`All/Running/Blocked/Failed/Done`。
   - `AppState` 增加字段并在 `AppState::new` 初始化。

2. **纯聚合模型**
   - 在 `app.rs` 或独立小模块增加 `ActivityNavigatorModel`、
     `ActivityNavigatorRow`、`ActivityNavigatorRowKind`。
   - 实现 `activity_navigator_model(app, state) -> ActivityNavigatorModel`。
   - 聚合顺序保持稳定：active/current session 优先，其余按 `sessions` 顺序；
     同 session 内按 live orchestration、tasks、live activity、archived logs。

3. **渲染**
   - 当 `activity_navigator.active` 时走 fullscreen overlay，布局为：
     top toolbar（query + filter counts）/ left result list / right detail /
     bottom compact hint-bar。
   - 复用 #6 几何 helper 风格：先算 `ActivityNavigatorAreas`，再渲染。
   - 复用 #4 hint-bar 模型扩展一个 `ActivityNavigator` mode。

4. **输入**
   - `/activity` 打开 overlay；Esc 关闭。
   - `j/k` 或上下键移动 selection；`/` 聚焦搜索；Backspace/文本键编辑 query；
     `Tab` 或 `f` 循环 filter。
   - Enter 第一版只跳转/选择对应 session（如果 row 有 session id），不触发 task 操作。

5. **测试**
   - 先测纯模型，再测 overlay render 和输入状态转移。

## 排除范围

- 鼠标点击/拖拽。
- task cancel/restart、agent interrupt 等写操作。
- fuzzy search、排序配置、用户自定义 keybind。
- 后端新增 activity index API。

## 完成条件

场景: navigator 聚合 running/blocked/failed/done counts
  测试: activity_navigator_model_counts_statuses
  假设 AppState 同时包含 running task、blocked run_state、failed activity、completed task
  当 构建 ActivityNavigatorModel
  那么 counts 分别反映各状态数量

场景: navigator 搜索跨 session/task/activity 字段
  测试: activity_navigator_search_matches_task_and_activity_detail
  假设 task detail 与 activity detail 各包含不同关键字
  当 query 匹配其中一个关键字
  那么 结果只包含匹配 row

场景: navigator 状态过滤只保留目标状态
  测试: activity_navigator_filter_running_only
  假设 rows 同时有 running 与 completed
  当 filter 为 Running
  那么 结果不包含 completed row

场景: /activity 打开 fullscreen overlay
  测试: slash_activity_opens_activity_navigator_overlay
  假设 用户在 composer 输入 `/activity`
  当 提交命令
  那么 `activity_navigator.active == true` 且 `wants_fullscreen_overlay` 为 true

场景: overlay 渲染结果列表和 detail
  测试: activity_navigator_overlay_renders_results_and_detail
  假设 navigator active 且至少有一个 running task
  当 渲染 overlay
  那么 左侧包含 task title/status，右侧包含 detail 或 output tail

场景: overlay 输入不影响 transcript pager
  测试: activity_navigator_escape_closes_without_touching_pager_state
  假设 navigator active 且 transcript_scroll 已设置
  当 按 Esc 关闭 overlay
  那么 navigator 关闭，transcript_scroll 保持不变
