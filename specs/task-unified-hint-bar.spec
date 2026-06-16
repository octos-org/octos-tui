spec: task
name: "统一 compact hint-bar 模型"
inherits: project
tags: [tui, rendering, hints, statusbar]
depends: [task-geometry-helper, task-pager-scrollbar-model]
estimate: 0.5d
---

## 意图

octos-tui 已有状态行、pager、menu、onboarding、approval 等提示文本，但模式判断散落
在具体渲染函数里。herdr 可借鉴的是"每个模式都有稳定、紧凑的提示条"这个局部
UI 模式，而不是可配置 keybind 或全局 TUI 架构。本任务先把状态行末尾的 key hint
收敛成一个纯模型，作为后续 approval/onboarding/menu 接入同一 hint-bar 组件的地基。

## 已定决策

- 第一阶段只抽取模型，不改变现有状态行文案、顺序、颜色或快捷键语义。
- `HintBarModel` 只表达当前 compact hint 的模式与文本 key：默认状态行、
  pager 底部、pager 回看中。approval/onboarding/menu 后续按同一模型扩展。
- pager scrollbar 表达相对位置；hint-bar 继续表达模式与可用操作，两者并存。
- 不引入全量可配置 keybind，不把 herdr 的 1814 行 keybind 表迁入本项目。

## 边界

### Allowed Changes
- src/app.rs
- tests/pager_visual_continuity_contract.rs
- specs/**

### Forbidden
- 不改变任何 key handling。
- 不新增 crate 依赖。
- 不改变 inline scrollback、alt-screen 进入/退出或鼠标捕获策略。

## 排除范围

- 可配置 keybind 系统。
- approval/onboarding/menu 的完整提示条重排。
- 新增图标或多行 help overlay。

## 完成条件

场景: 默认 chat 状态使用默认 hint mode
  测试: hint_bar_model_defaults_to_statusbar_keys
  假设 pager 未激活
  当 构建 hint-bar model
  那么 mode 为 `StatusbarKeys`

场景: pager 底部使用 pager key hint
  测试: hint_bar_model_uses_pager_keys_at_bottom
  假设 pager 激活且 `transcript_scroll == 0`
  当 构建 hint-bar model
  那么 mode 为 `PagerKeys`

场景: pager 回看时使用 reviewing hint
  测试: hint_bar_model_uses_reviewing_when_pager_scrolled
  假设 pager 激活且 `transcript_scroll > 0`
  当 构建 hint-bar model
  那么 mode 为 `PagerReviewing`

场景: 旧 pager 状态行文案保持不变
  测试: pager_status_shows_reviewing_indicator / pager_status_hides_indicator_at_bottom
  假设 pager 激活
  当 渲染状态行
  那么 既有 Reviewing 与 PgUp/PgDn 契约继续通过
