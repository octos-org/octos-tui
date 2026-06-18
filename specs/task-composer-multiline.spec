spec: task
name: "Composer 多行输入：手动换行键 + 上下行光标移动"
inherits: project
tags: [tui, composer, input, editing]
estimate: 0.5d
---

## 意图

composer 已能存储含 `\n` 的多行文本、随内容长高并自动折行、多行光标定位也已就绪，
但**用户无法用按键手动插入换行**——普通 Enter 直接发送，只有"粘贴多行文本"的启
发式会插 `\n`；且 Up/Down 在 composer 里只滚动 transcript，无法在多行间移动光标。
本任务补齐这两点，让多行输入真正可用（对标 Claude Code / codex 的输入体验）。
Vim 模式是独立的后续任务，不在本期。

## 已定决策

- **换行键**：`Alt+Enter` 与 `Ctrl+J` 在 composer 聚焦时于光标处插入 `\n`，不发送。
  二者并存：`Ctrl+J` 在所有终端都可靠（本就是 LF），`Alt(Option)+Enter` 多数终端可用。
  不依赖 `Shift+Enter`（macOS Terminal.app 在无 Kitty/modifyOtherKeys 时收不到独立的
  Shift+Enter）。插入逻辑复用既有 `insert_composer_text`。
- **Enter 语义不变**：普通 `Enter` 仍发送（`handle_composer_enter`），保留现有习惯；
  "粘贴多行"启发式 `should_insert_unbracketed_paste_newline` 不回归。
- **上下行光标移动**：新增 `move_composer_cursor_up` / `move_composer_cursor_down`
  （模型层、与渲染宽度无关），按**逻辑行**（`\n` 分隔）移动并保留目标列（典型 composer
  行短于终端宽度，逻辑行≈视觉行）。视觉折行级别的上下移动属精修，本期不做。
- **边界回退**：在 composer 聚焦时，Up 当光标不在首逻辑行→上移光标；在首逻辑行（或
  单行/空）→沿用现有 `scroll_transcript_up`。Down 对称。如此既得多行光标移动，又不丢
  "方向键滚动 transcript" 的既有手感。
- **换行键位提示**：在 composer 底部提示串里加入换行键说明（en/zh 双语齐备）。
- **输入框随换行长高（关键体验）**：composer 高度上限按**完整终端高度**计算，渲染
  路径（`render_viewport`）必须与预留路径（`live_ui_height`）用同一基准——此前渲染
  误用 inline 视口区域高度算上限、被钳到 3 行，导致多行输入丢前面的行。并且 inline
  live-tail 用 `Min(1)` 布局，预留高度须把这 1 行算进去，否则空 tail 会少预留一行、
  从 composer 偷走一行。

## 边界

### Allowed Changes
- src/event_loop.rs
- src/model.rs
- src/app.rs
- locales/en.yml
- locales/zh.yml
- tests/composer_multiline_contract.rs
- specs/**

### Forbidden
- 不改变普通 `Enter` 发送的语义。
- 不回归粘贴多行换行启发式（`should_insert_unbracketed_paste_newline`）。
- 不引入新 crate 依赖。
- 不启用鼠标捕获、不破坏 inline-scrollback 渲染模型。

## 排除范围

- Vim / 模态编辑（独立后续任务）。
- 视觉折行级别的上下光标移动（本期按逻辑行）。
- 软换行重排、自动缩进、括号配对等富编辑能力。

## 完成条件

场景: Alt+Enter 在光标处插入换行而不发送
  测试: alt_enter_inserts_newline_without_submitting
  假设 composer 聚焦且含文本 "ab"、光标在末尾
  当 用户按下 Alt+Enter
  那么 composer 文本变为 "ab\n"
  并且 不产生发送动作（不调用 turn/start）

场景: Ctrl+J 同样插入换行
  测试: ctrl_j_inserts_newline
  假设 composer 聚焦且含文本 "ab"
  当 用户按下 Ctrl+J
  那么 composer 文本含一个新插入的 "\n"
  并且 不产生发送动作

场景: 普通 Enter 仍发送（不回归）
  测试: plain_enter_still_submits
  假设 composer 聚焦且含非空文本
  当 用户按下无修饰的 Enter
  那么 触发发送（与现状一致）

场景: 多行时 Down 在逻辑行间下移光标并保留列
  测试: arrow_down_moves_cursor_to_next_line
  假设 composer 含 "abc\nde"、光标在第一行第 3 列
  当 用户按下 Down
  那么 光标移到第二行、列被钳到行尾（第 2 列）
  并且 transcript 不滚动

场景: 首行按 Up 回退为滚动 transcript（不回归既有手感）
  测试: arrow_up_at_first_line_scrolls_transcript
  假设 composer 含 "abc\nde"、光标在第一行
  当 用户按下 Up
  那么 光标不移动
  但是 transcript 上滚一行（沿用现有行为）

场景: composer 随换行长高
  测试: composer_height_grows_with_newlines
  假设 composer 文本含两个 "\n"（三行）
  当 计算 composer 高度
  那么 高度大于单行时的高度

场景: 末行按 Down 不越界
  测试: arrow_down_at_last_line_does_not_panic
  假设 composer 含 "abc\nde"、光标在最后一行末尾
  当 用户按下 Down
  那么 不发生 panic、光标停在合法位置（回退为滚动 transcript）

场景: 多行输入在 inline 视口里不被截断
  测试: multiline_composer_not_capped_in_inline_viewport
  假设 composer 含 6 个逻辑行、终端高 40
  当 渲染 inline 视口
  那么 首行与末行都可见（高度上限按完整终端高度、非视口区域高度）
  并且 不出现"earlier lines hidden"截断
