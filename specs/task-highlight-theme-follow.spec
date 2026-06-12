spec: task
name: "代码高亮主题跟随 /theme"
inherits: project
tags: [tui, highlight, theme]
estimate: 0.5d
---

## 意图

代码高亮固定一套 syntect 主题，与 `/theme` 选择的界面主题无关。本任务为
每个界面主题映射一套 syntect 配色（仍 fg-only），`/theme` 切换后代码块
配色随之变化，缓存按主题隔离不串色。

## 已定决策

- `Palette` 新增 `code_theme: &'static str`（syntect 默认主题集内的名字）：
  Terminal/Codex → base16-eighties.dark，Slate → base16-ocean.dark，
  Claude → base16-mocha.dark，Solarized → Solarized (dark)。
- `highlight.rs` 主题按名懒加载（`ThemeSet` 进程内一份，按名取引用）；
  未知主题名回退 base16-eighties.dark，不 panic。
- 块缓存 key 纳入主题名：`/theme` 切换后不会命中旧主题的渲染结果。
- 仍只取前景色（无背景不变量不变）。

## 边界

### Allowed Changes
- src/theme.rs
- src/highlight.rs
- src/app.rs
- tests/highlight_theme_contract.rs
- specs/**

### Forbidden
- 不给代码块引入背景色。
- 不新增依赖。

## 排除范围

- 自定义/用户自配 syntect 主题。
- 浅色终端自动检测。

## 完成条件

场景: 不同界面主题产生不同代码配色
  测试: themes_yield_distinct_token_colors
  假设 同一段 rust 代码
  当 分别以两个映射不同 syntect 主题的界面主题渲染
  那么 至少一个 token 的前景色不同

场景: 缓存按主题隔离
  测试: cache_isolated_per_theme
  假设 同一代码块先后以两个主题渲染
  当 第二个主题渲染时
  那么 输出为该主题配色（不命中第一个主题的缓存）

场景: 未知主题名安全回退
  测试: unknown_theme_falls_back_safely
  假设 请求一个不存在的 syntect 主题名
  当 渲染代码块
  那么 使用回退主题正常着色，不 panic

场景: 高亮仍不携带背景色
  测试: themed_highlight_keeps_no_background
  假设 任一主题下渲染代码块
  当 检查高亮 span
  那么 无任何背景色（既有不变量不回归）
