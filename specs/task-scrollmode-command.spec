spec: task
name: "/scrollmode 运行时切换滚动模式"
inherits: project
tags: [tui, command, scroll-mode]
estimate: 0.5d
---

## 意图

scroll-mode（native/pinned）目前只能启动时配置，切换要重启。本任务新增
`/scrollmode` 斜杠命令：无参数切换（toggle），带参数显式设置；鼠标捕获
随状态在下一帧自动同步（draw 已按 `wants_mouse_capture` 每帧对齐），
即时生效无需重启。

## 已定决策

- 新 `LocalAction::SetScrollMode`，registry 注册 `scrollmode`
  （别名 `scroll-mode`），`InlineArgMode::Optional`——与 `/thinking`
  同构：空参 toggle、`native`/`pinned` 显式设置、未知参数报状态行错误。
- 只改 `AppState.pinned_scroll` 运行时值，不写回 config 文件（启动配置
  仍是默认来源；持久化属配置管理范畴，不在本任务）。
- 状态行确认文案双语（locales en/zh 同步新增）。

## 边界

### Allowed Changes
- src/menu/registry.rs
- src/menu/types.rs
- src/menu/providers.rs
- src/event_loop.rs
- src/store.rs
- locales/en.yml
- locales/zh.yml
- tests/scrollmode_command_contract.rs
- specs/**

### Forbidden
- 不改变启动配置解析与默认值。
- 不在命令中写文件。

## 排除范围

- 选择菜单 UI（仅命令行式切换）。
- 持久化到 config 文件。

## 完成条件

场景: 无参数切换滚动模式
  测试: bare_scrollmode_toggles
  假设 当前为 native 模式
  当 用户执行 /scrollmode
  那么 切换为 pinned 且状态行确认；再次执行切回 native

场景: 显式参数设置模式
  测试: explicit_argument_sets_mode
  假设 当前为 native 模式
  当 用户执行 /scrollmode pinned
  那么 pinned_scroll 为真且鼠标捕获策略立即返回真

场景: 未知参数不改状态
  测试: unknown_argument_keeps_mode
  假设 当前为 native 模式
  当 用户执行 /scrollmode banana
  那么 模式不变且状态行提示未知参数

场景: 弹窗回车补全命令到输入框（带参命令两段式）
  测试: popup_enter_completes_argful_command
  假设 用户输入 /sc 且弹窗过滤出 scrollmode
  当 用户回车选中
  那么 输入框补全为完整的 /scrollmode 前缀（不立即执行）
  并且 续输参数后再回车，设置生效且弹窗关闭

场景: 弹窗条目展示当前滚动模式
  测试: popup_entry_shows_current_mode
  假设 斜杠弹窗过滤出 scrollmode 条目
  当 构建该条目描述
  那么 描述包含当前模式（native/pinned），切换后重开弹窗随之更新

场景: 命令在帮助注册表中可发现
  测试: scrollmode_registered_in_command_registry
  假设 核心命令注册表构建完成
  当 解析 /scrollmode
  那么 命令被解析为 SetScrollMode 本地动作
