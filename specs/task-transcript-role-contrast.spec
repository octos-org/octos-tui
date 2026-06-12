spec: task
name: "transcript 角色视觉分层：用户竖条锚点 + 日志降权"
inherits: project
tags: [tui, rendering, transcript, ux]
depends: []
estimate: 0.5d
---

## 意图

输出面板里用户输入、agent 回复正文、agent 运行时/工具日志三类内容混在
一起难以区分：区分手段只有 `› `/`• ` 两个小前缀和背景色（背景色在 pager
已剥除、terminal 主题下本为透明、scrollback 中不可靠），而活动日志的
label 反而用 bold 亮色，视觉权重高过正文。本任务建立三层视觉梯度：用户
输入升权（accent 竖条 gutter + 加粗，全局唯一锚点）、日志整体降权
（统一 muted、去 bold）、回复正文保持基线零装饰。

## 已定决策

- 用户输入改走专用渲染：每个逻辑行前缀 `▌ `（`palette.accent` 色）+
  正文 `palette.text()` 加粗；内容按原文逐行显示（verbatim echo，不再走
  markdown 渲染——用户输入是引用不是排版对象）；不再设置背景色
  （`diff_context_bg` 退役于 user 消息），inline/scrollback/pager 三处
  外观统一。`push_recent_user_context` 同步走该路径。
- 活动日志降权：组标题色从 `title` 降为 `muted`（保留 BOLD 与状态图标的
  状态色——spinner/✓/✗ 的颜色有信息量）；子行的 action label / 工具名 /
  调用文本全部 `muted` 并去 BOLD；"preview ready" 等可操作提示保留
  `selected` 高亮。
- agent 回复正文零改动：`• ` 前缀、`text` 色、markdown 渲染均保持，
  作为三层梯度的基线。
- 不依赖背景色实现任何区分（项目内已验证 bg 在三个显示场景均不可靠）。

## 边界

### Allowed Changes
- src/app.rs
- tests/transcript_role_contrast_contract.rs
- specs/**

### Forbidden
- 不改变 composer / 状态行 / 菜单的样式。
- 不恢复任何消息背景色块。
- 不引入新 crate 依赖。

## 排除范围

- 日志组默认折叠（Ctrl+O 展开机制已有，默认态另起任务）。
- `Palette` 新增 `user_accent` 字段（本期复用 `accent`）。
- inspector 布局的同等改造。

## 完成条件

场景: 用户消息每行带 accent 竖条且正文加粗
  测试: user_message_renders_accent_gutter_bold
  假设 会话含一条多行用户消息
  当 渲染 transcript
  那么 每个逻辑行以 accent 色的竖条 gutter 开头
  并且 正文单元格带加粗修饰

场景: 用户消息不再携带背景色
  测试: user_message_has_no_background
  假设 会话含用户消息
  当 渲染 transcript
  那么 用户消息行所有单元格背景为默认色（与 pager/scrollback 一致）

场景: 活动日志子行统一弱化且无加粗
  测试: activity_rows_are_muted_without_bold
  假设 存在含工具调用的活动日志组
  当 渲染 transcript
  那么 子行的工具 label 与标题单元格为 muted 色且无 BOLD 修饰

场景: agent 回复正文样式不回归
  测试: assistant_body_style_unchanged
  假设 会话含 agent 回复
  当 渲染 transcript
  那么 回复正文仍以 • 前缀、text 色渲染且无竖条

场景: 三处渲染语言一致
  测试: pager_and_inline_share_role_styling
  假设 同一会话分别在 pager 与 inline live tail 渲染
  当 比较用户消息行
  那么 两处均为 accent 竖条 + 加粗正文（无背景）
