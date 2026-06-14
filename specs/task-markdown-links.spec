spec: task
name: "Markdown 链接与行级标记渲染"
inherits: project
tags: [tui, rendering, markdown]
estimate: 0.5d
---

## 意图

行内 markdown 渲染支持 `**`/`` ` ``/`*`，但 `[text](url)` 原样显示、最影响阅读。
本任务为链接加视觉区分渲染，并顺带补水平分割线与删除线。三处渲染路径
（live tail / scrollback / pager）共用同一行内函数，自动全生效。

## 已定决策

- 链接 `[text](url)`：在行内扫描循环加分支，链接文字用 accent/selected 色，其后
  追加 ` (url)` 用 muted 色。不走真 OSC8 超链接——ratatui 按 Cell 渲染，原始转义
  会被计入宽度破坏布局。过长 url 用既有 `truncate_terminal_line` 截断。
- 删除线 `~~text~~`：行内加 CROSSED_OUT modifier。
- 水平分割线：整行 `---` / `***` / `___`（≥3 个、去空白后纯重复符号）在块级
  渲染为一条 muted 横线（仿 `markdown_heading` 加 `markdown_hr` 判定）。
- 嵌套列表 / 嵌套引用本期不做（成本高、价值低）。
- 不引入新依赖；不给链接/正文加背景色（保持无背景不变量）。

## 边界

### Allowed Changes
- src/app.rs
- tests/markdown_links_contract.rs
- specs/**

### Forbidden
- 不发出真 OSC8 超链接转义（破坏 Cell 布局）。
- 不引入新 crate 依赖。
- 不改变代码块 / 表格 / 标题的既有渲染。

## 排除范围

- 嵌套列表、嵌套引用、HTML、图片。
- 可点击/可跟随的真超链接。

## 完成条件

场景: 链接渲染为高亮文字加灰色 URL
  测试: link_renders_text_and_muted_url
  假设 正文含 [Octos](https://example.com)
  当 渲染行内 span
  那么 出现 accent/selected 色的链接文字 span 与 muted 色的 URL span

场景: 链接不发出 OSC8 转义
  测试: link_emits_no_osc8_escape
  假设 正文含一个 markdown 链接
  当 渲染行内 span
  那么 span 文本不含 OSC8 超链接转义序列（\x1b]8）

场景: 删除线加删除修饰
  测试: strikethrough_adds_crossed_out
  假设 正文含 ~~obsolete~~
  当 渲染行内 span
  那么 对应文字 span 带 CROSSED_OUT 修饰

场景: 整行分割线渲染为横线
  测试: horizontal_rule_renders_divider
  假设 一整行为 ---
  当 渲染该段落
  那么 产出一条 muted 横线行而非字面三连字符

场景: 普通方括号文本不被误判为链接
  测试: non_link_brackets_render_plain
  假设 正文含 [just brackets] 但其后无 (url)
  当 渲染行内 span
  那么 按普通文本渲染、不产生链接样式
