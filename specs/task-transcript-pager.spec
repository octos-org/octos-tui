spec: task
name: "全屏 transcript pager（输入框钉底的会话滚动视图）"
inherits: project
tags: [tui, rendering, pager, scrollback, m23-candidate]
depends: []
estimate: 1d
---

## 意图

普通 chat 采用 inline viewport + 终端原生 scrollback 模型（`event_loop.rs:63`、
`insert_history.rs`）：已提交历史被物理写进终端自己的滚动缓冲区，用户用滚轮/
滚动条回看时，底部 composer 必然随整屏一起滚出可视区——这是终端能力边界，
inline 模式下无法固定。本任务新增一个 alt-screen 全屏 transcript pager 模式：
pager 内完整会话在上方独立滚动，composer 始终钉在终端最底端；inline 模式的
原生 scrollback / 鼠标选择 / 复制体验保持完全不变。

## 已定决策

- 采用 alt-screen transcript pager 方案（codex CLI 同款 Ctrl+T 模型），
  **不**把普通 chat 改成常驻 alt-screen 内部滚动——inline + scrollback
  模型是已落地的设计不变量（见 project spec 禁止项），改回全屏模型会
  重新引入"定频重绘抹选区"问题（`event_loop.rs:38-41` 记录的旧病）。
- 复用现有 alt-screen 切换机制：pager 激活状态并入
  `app::wants_fullscreen_overlay`（`src/app.rs:83`），由 `TerminalGuard`
  统一进出 alternate screen；渲染复用 `render_chat_layout`/`render_transcript`
  （`src/app.rs:736`、`src/app.rs:1408`，composer 已是钉底布局）。
- 键位：Ctrl+T 切换进出 pager；Esc 退出；pager 内 PageUp/PageDown 与
  上下方向键滚动 transcript。chat 模式下按 PageUp 自动进入 pager
  （当前 inline 下 PageUp 只能滚 live tail，够不到已提交历史）。
- 鼠标捕获只在 pager 内启用：进入时 `EnableMouseCapture`、退出时关闭，
  让滚轮在 pager 内可用；inline 模式永不捕获鼠标。
- 退出 pager 时 `transcript_scroll` 归零，inline 视口回到底部跟随最新输出。

## 边界

### Allowed Changes
- src/app.rs
- src/event_loop.rs
- src/model.rs
- src/store.rs
- locales/en.yml
- locales/zh.yml
- tests/transcript_pager_contract.rs
- specs/**
- **/.gitignore

### Forbidden
- 不修改 `src/insert_history.rs` 与 `src/viewport.rs` 的 scrollback 写入机制。
- 不在 inline（非 pager）模式启用鼠标捕获（默认 native 模式；pinned 模式的显式 opt-in 豁免见 task-pinned-scroll-mode spec）。
- 不把普通 chat 渲染迁入常驻 alternate screen。
- 不引入新 crate 依赖。

## 排除范围

- inline 模式下固定 composer（终端查看自身 scrollback 时无转义序列可固定
  屏幕区域，架构上不可行——本任务的 pager 即是对此的替代答案）。
- pager 内的文本选择/复制增强（原生选择回 inline 模式做；OSC52 复制已有）。
- transcript 内搜索、过滤、跳转。
- 鼠标点击定位光标或链接点击。

## 完成条件

场景: Ctrl+T 进入 pager 并切换到 alt-screen
  测试: ctrl_t_enters_transcript_pager_fullscreen
  假设 普通 chat inline 模式且会话含已提交历史
  当 用户按下 Ctrl+T
  那么 pager 状态激活
  并且 wants_fullscreen_overlay 返回真（事件循环据此进入 alternate screen）

场景: pager 内滚动时 composer 仍渲染在最底行
  测试: pager_scroll_keeps_composer_pinned_at_bottom
  假设 pager 已激活且 transcript 总行数超过可视高度
  当 用户按 PageUp 向上滚动
  那么 transcript 视口上移且更早的已提交消息可见
  并且 composer 与 status 行仍渲染在终端区域的最底部

场景: 退出 pager 回 inline 且滚动位置复位
  测试: pager_exit_restores_inline_and_resets_scroll
  层级: 集成
  替身: mock 后端 + 测试终端（TerminalGuard 状态断言，无真实 tty）
  假设 pager 激活且 transcript_scroll 大于 0
  当 用户按 Esc 或再次按 Ctrl+T
  那么 回到 inline viewport 渲染路径
  并且 transcript_scroll 归零、鼠标捕获已关闭

场景: chat 模式按 PageUp 自动进入 pager
  测试: pageup_in_chat_auto_enters_pager
  假设 普通 chat inline 模式且存在已提交历史
  当 用户按 PageUp
  那么 pager 激活且初始位置在底部（最近消息可见）

场景: 已有模态 overlay 激活时 Ctrl+T 不进入 pager
  测试: ctrl_t_ignored_when_modal_overlay_active
  假设 task_output 或 approval 等模态正在显示
  当 用户按下 Ctrl+T
  那么 pager 不激活
  并且 当前模态保持原状、按键不被吞掉语义

场景: inline 模式永不启用鼠标捕获
  测试: inline_mode_never_enables_mouse_capture
  层级: 集成
  替身: mock 后端 + 测试终端（TerminalGuard 状态断言，无真实 tty）
  假设 普通 chat inline 模式（pager 未激活）
  当 事件循环正常渲染与轮询
  那么 不发出 EnableMouseCapture
  并且 已提交历史不进入 inline viewport（原生选择/复制不回归）

场景: turn 流式输出期间进出 pager 不丢内容
  测试: pager_during_active_turn_streams_and_returns_to_tail
  假设 一个 turn 正在流式输出回复
  当 用户进入 pager 随后退出
  那么 pager 内可见最新流式内容
  并且 退出后 inline live tail 继续跟随最新输出且 finalization 水位不错乱

场景: 空会话进入 pager 安全渲染
  测试: pager_with_empty_transcript_renders_safely
  假设 无活动会话或 transcript 为空
  当 用户按下 Ctrl+T
  那么 pager 渲染空 transcript 而不 panic
  但是 composer 仍可正常输入与提交
