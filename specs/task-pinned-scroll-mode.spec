spec: task
name: "scroll-mode 配置（pinned：滚轮滚动时输入框钉底）"
inherits: project
tags: [tui, rendering, pager, scroll-mode, config]
depends: []
estimate: 0.5d
---

## 意图

transcript pager 落地后，滚轮上滚仍走终端原生 scrollback，composer 随屏滚走，
用户必须主动按 Ctrl+T/PageUp 才能获得钉底体验。本任务新增 `scroll-mode` 启动
配置：`native`（默认，行为与现状完全一致）/ `pinned`（chat inline 也捕获鼠标，
滚轮上滚自动进入 transcript pager、在 pager 底部继续下滚自动退回 inline——
体感为"无论怎么滚，输入框都钉在终端底部"）。pinned 是显式 opt-in：代价是
终端原生鼠标选择需改用 Shift+拖选，由用户自行权衡。

## 已定决策

- 配置面：CLI `--scroll-mode <native|pinned>` + config 文件 key
  `scroll-mode`（含 `scroll_mode` alias），CLI 优先于 config，默认 `native`。
  解析模式与既有 `theme`/`lang` 完全一致（`ScrollMode` ValueEnum + Deserialize）。
- `native` 模式行为零变化：inline 不捕获鼠标、滚轮走终端原生 scrollback、
  pager 仍由 Ctrl+T/PageUp 进入。project spec 的 "inline 不启用鼠标捕获"
  不变量在 native 模式下继续成立；pinned 模式是该不变量的**显式 opt-in 豁免**。
- `pinned` 模式下鼠标捕获常开（`wants_mouse_capture` 返回真），滚轮事件
  进 App：chat 内上滚自动 `enter_transcript_pager`；pager 内已在底部
  （`transcript_scroll == 0`）时继续下滚自动 `exit_transcript_pager`。
  自动退出仅在 pinned 模式生效，native 模式手动进入的 pager 不受影响。
- 状态承载：`AppState.pinned_scroll: bool`，由事件循环启动时从 `Cli` 播种
  （与 `theme` 同模式）；渲染/事件路径只读该字段。

## 边界

### Allowed Changes
- src/cli.rs
- src/app.rs
- src/event_loop.rs
- src/model.rs
- src/transport.rs
- tests/pinned_scroll_mode_contract.rs
- specs/**
- TUI-使用指南.md

### Forbidden
- 不修改 `src/insert_history.rs` 与 `src/viewport.rs` 的 scrollback 写入机制。
- 不改变 `native` 模式（默认）的任何现有行为或键位。
- 不引入新 crate 依赖。

## 排除范围

- 运行时切换 scroll-mode（`/scrollmode` 之类的 slash 命令）——本期仅启动配置。
- pinned 模式下的鼠标点击定位、拖选、链接点击。
- 把 pinned 设为默认值。

## 完成条件

场景: 默认 native 模式行为零变化
  测试: native_mode_default_keeps_mouse_capture_off
  假设 未配置 scroll-mode（默认 native）
  当 chat inline 模式渲染与轮询
  那么 pinned_scroll 为假且不请求鼠标捕获
  并且 已提交历史仍只在终端原生 scrollback 中

场景: native 模式滚轮事件不改变 pager 状态
  测试: native_mode_wheel_does_not_enter_pager
  假设 native 模式下收到滚轮上滚事件
  当 事件分发处理该事件
  那么 pager 不激活
  但是 live tail 滚动行为与现状一致

场景: pinned 模式 inline 即请求鼠标捕获
  测试: pinned_mode_requests_mouse_capture_inline
  假设 scroll-mode 配置为 pinned
  当 chat inline 模式渲染
  那么 wants_mouse_capture 返回真（滚轮事件路由进 App）

场景: pinned 模式滚轮上滚自动进入 pager 且 composer 钉底
  测试: pinned_mode_wheel_up_enters_pager
  假设 pinned 模式 chat inline 含已提交历史
  当 用户滚轮上滚
  那么 pager 自动激活且初始位置在底部
  并且 composer 仍渲染在终端区域最底部

场景: pinned 模式 pager 底部继续下滚自动退回 inline
  测试: pinned_mode_wheel_down_at_bottom_exits_pager
  假设 pinned 模式 pager 激活且 transcript_scroll 为 0
  当 用户滚轮下滚
  那么 pager 退出回到 inline 跟随模式
  但是 native 模式手动进入的 pager 在底部下滚不自动退出

场景: config 文件可解析 scroll-mode
  测试: scroll_mode_parses_from_config_file
  假设 JSON config 文件含 "scroll-mode": "pinned"
  当 load_config_file 解析该文件
  那么 解析出 ScrollMode::Pinned
  并且 未设置该 key 时为 None（最终默认 native）
