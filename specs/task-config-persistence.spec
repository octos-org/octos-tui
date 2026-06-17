spec: task
name: "运行时配置持久化（/saveconfig）"
inherits: project
tags: [tui, config, command]
estimate: 1d
---

## 意图

`/theme`、`/lang`、`/scrollmode` 的运行时改动只存内存、退出即丢——cli.rs 只在
启动读 config、无回写。新增 `/saveconfig` 命令，把当前的 theme/lang/scroll-mode
安全合并写回 config 文件，使下次启动沿用，消除"重启重配"痛点。

## 已定决策

- 新增 `LocalAction::SaveConfig`，registry 注册 `saveconfig`（别名 `save-config`），
  `InlineArgMode::None`——与既有本地命令同构。
- 运行时保留 config 路径：事件循环启动时把 `Cli.config` 播种进
  `AppState.config_path: Option<PathBuf>`（与 theme/pinned_scroll 同模式，启动 seed、
  运行只读）。
- 安全合并写回：读现有 config JSON 为 `serde_json::Value`、仅 patch
  `theme`/`lang`/`scroll-mode` 三个键、写回——不整体序列化，保留
  stdio-command/profile-id/session/endpoint 等运输配置不被清除。文件不存在或无
  `--config` 时写默认路径并在状态行提示。
- 仅持久化全局启动设置（theme/lang/scroll-mode）。不含 thinking——它是 per-session
  且由服务器侧持久化，纳入会语义混乱。
- 写值用各枚举既有的 kebab 序列化（与启动解析对称，round-trip 一致）。
- 状态行确认/错误文案双语（locales en/zh）。

## 边界

### Allowed Changes
- src/cli.rs
- src/model.rs
- src/event_loop.rs
- src/menu/types.rs
- src/menu/registry.rs
- src/store.rs
- locales/en.yml
- locales/zh.yml
- tests/save_config_contract.rs
- specs/**

### Forbidden
- 不改变启动时 config 的解析与优先级（CLI 覆盖 config）。
- 不整体序列化覆盖 config（必须保留未知/运输键）。
- 不持久化 per-session 的 thinking。
- 不引入新 crate 依赖（serde_json 已在依赖中）。

## 排除范围

- 自动持久化（退出/改动即存）——本期仅显式 `/saveconfig`。
- keymap / statusline / title 的持久化。
- 多 config profile 切换。

## 完成条件

场景: 保存把运行时设置写回 config 文件
  测试: saveconfig_writes_runtime_settings
  层级: 集成
  替身: 临时 config 文件（tempdir，无真实 tty）
  假设 store 持有一个指向已存在 config 的路径，且运行时已改 theme 与 scroll-mode
  当 触发 SaveConfig
  那么 config 文件中 theme 与 scroll-mode 键更新为当前值

场景: 保存保留运输与未知键
  测试: saveconfig_preserves_transport_keys
  层级: 集成
  替身: 含 stdio-command/profile-id 的临时 config 文件
  假设 config 文件含 stdio-command 与 profile-id
  当 触发 SaveConfig
  那么 stdio-command 与 profile-id 原样保留，仅 UI 设置键被改写

场景: 写回的值能被启动解析回相同设置
  测试: saved_config_roundtrips_through_loader
  假设 SaveConfig 已写出 theme/lang/scroll-mode
  当 用 config 加载器重新解析该文件
  那么 解析出的 theme/lang/scroll-mode 等于保存时的运行时值

场景: 无 config 路径时解析出默认路径
  测试: default_config_path_resolves_under_config_dir
  假设 未带 --config 启动（无运行时 config 路径）
  当 解析保存回退路径
  那么 路径位于 ~/.config/octos-tui/config.json（保存写入器据此落盘）

场景: thinking 不被持久化
  测试: saveconfig_excludes_thinking
  假设 运行时设置了 per-session thinking 强度
  当 触发 SaveConfig 写回
  那么 config 文件不含 thinking 键（仅 theme/lang/scroll-mode）
