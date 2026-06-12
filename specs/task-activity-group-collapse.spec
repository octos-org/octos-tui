spec: task
name: "已完成活动组默认折叠为单行摘要"
inherits: project
tags: [tui, transcript, activity, ux]
estimate: 0.5d
---

## 意图

已完成 turn 的活动日志组（工具调用流水）默认展开全部子行，长会话里灰色
流水占据大量屏幕。本任务让**已完成**组默认折叠为单行标题摘要（标题本就
含操作计数），Ctrl+O 既有全局展开开关原样复用；**进行中**的组始终展开
（实时反馈不可藏）。

## 已定决策

- 折叠只作用于重绘视图（pager / live tail）：scrollback flush 路径始终
  完整写入子行——终端历史是只追加的存档，折叠它会永久丢失信息。
- 复用 `expanded_tool_outputs`（Ctrl+O）作为唯一开关：展开时所有组完整
  显示，无新键位、无新状态字段。
- 活跃判定复用既有 `is_active_group`：进行中（含子代理运行中）的组不折叠。

## 边界

### Allowed Changes
- src/app.rs
- tests/activity_collapse_contract.rs
- tests/slash_popup_contract.rs
- specs/**

### Forbidden
- 不改变 scrollback flush 内容的完整性。
- 不新增键位或状态字段。

## 排除范围

- 按组独立折叠/展开（仅全局开关）。
- inspector 布局的同等处理。

## 完成条件

场景: 已完成组默认只渲染标题摘要行
  测试: settled_group_collapses_to_header
  假设 存在已完成 turn 的活动组且未开启展开
  当 渲染 pager transcript
  那么 该组只出现标题行（含操作计数），子行不渲染

场景: Ctrl+O 展开后子行完整显示
  测试: expanded_group_shows_children
  假设 同一活动组且 expanded_tool_outputs 为真
  当 渲染 pager transcript
  那么 子行（工具调用明细）完整出现

场景: 进行中的组不折叠
  测试: active_group_never_collapses
  假设 turn 进行中且其活动组含运行中条目
  当 渲染 live tail
  那么 子行照常显示（实时反馈不受折叠影响）

场景: 斜杠弹窗在折叠后的矮视口中仍渲染
  测试: slash_popup_renders_in_short_viewport
  假设 折叠使 inline viewport 变矮且用户键入斜杠命令前缀
  当 渲染 inline viewport
  那么 弹窗内容可见（高度启发式不得把已预留的菜单空间渲染为空白）

场景: scrollback flush 始终完整
  测试: scrollback_flush_keeps_children
  假设 已完成组经 scrollback flush 路径写入
  当 生成 flush 行
  那么 子行完整包含在内（历史存档不折叠）
