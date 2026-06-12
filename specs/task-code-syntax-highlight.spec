spec: task
name: "代码块语法高亮（syntect，fg-only）"
inherits: project
tags: [tui, rendering, markdown, highlight]
depends: []
estimate: 1d
---

## 意图

transcript 中的代码块目前以统一的 muted 单色渲染，只有围栏边框和语言标签，
没有语法高亮——这是 markdown 渲染最后一个显著缺口。本任务引入 syntect
按围栏语言标签做逐行语法高亮，输出仅取前景色（延续"无背景色"原则，
与任意终端配色兼容），在 live tail / scrollback / pager 三处同时生效。

## 已定决策

- 新依赖 syntect（`default-fancy` 特性：纯 Rust 正则，无 onig C 依赖）。
  这是 project spec"无正当理由不加依赖"的显式豁免：Sublime 语法定义集
  覆盖数百种语言，自研关键字表达不到可用质量且维护负担不可控；syntect
  是 bat/delta 等终端工具的事实标准。
- 仅取前景色：syntect 主题（深色系）样式只映射 fg 到 Span，不设置任何
  背景色——保持代码块与终端默认背景融合（pager/scrollback 不变量）。
- `SyntaxSet`/`Theme` 进程内懒加载一次（`OnceLock`）；语言按围栏标签
  `find_syntax_by_token` 解析，未识别语言与超过 300 行的超长块回退为
  现状的 muted 单色（流式每帧重渲的帧预算保护）。
- 高亮器按块持有逐行状态（`HighlightLines`），在渲染循环内跨行推进——
  多行结构（块注释、字符串延续）着色正确。
- 性能两件套（实测 debug 构建 237ms/帧 → 13.6ms/帧）：① 已闭合块按
  `(语言, 内容, 主题色)` 整块 memoize（pager 每帧重渲全部历史，缓存把
  高亮成本降为 O(流式新增)；未闭合的流式块每帧变化，不入缓存）；
  ② dev profile 对 syntect/fancy-regex 等热点依赖开 opt-level 3
  （未优化的正则引擎慢 10-50 倍，本仓代码保持快速增量编译）。
  流式/提交双路径一致性场景同时锁定缓存输出与直渲输出一致。

## 边界

### Allowed Changes
- **/Cargo.toml
- **/Cargo.lock
- src/lib.rs
- src/highlight.rs
- src/app.rs
- tests/code_highlight_contract.rs
- specs/**

### Forbidden
- 不修改 scrollback 写入机制与块级水位逻辑。
- 不引入需要 C 工具链的依赖（onig 明确排除）。
- 不给代码块设置背景色。

## 排除范围

- 高亮主题跟随 `/theme` 切换（本期固定一套深色友好主题）。
- diff 预览面板、工具输出预览的高亮（另起任务）。
- 行内 `code` span 的语法高亮（保持现状单色）。

## 完成条件

场景: 已知语言的代码块按语法着色
  测试: known_language_block_gets_colored_tokens
  假设 transcript 含 rust 围栏代码块
  当 渲染该代码块
  那么 代码行内出现多种不同前景色的 token（非单一 muted 色）

场景: 高亮不引入任何背景色
  测试: highlighted_code_has_no_background
  假设 pager 中渲染高亮代码块
  当 检查代码区域单元格
  那么 所有单元格背景仍为终端默认色

场景: 未知语言回退单色渲染
  测试: unknown_language_falls_back_to_muted
  假设 代码块语言标签无法被语法集识别
  当 渲染该代码块
  那么 代码行以现状 muted 单色渲染（不 panic、不乱猜语言）

场景: 无语言标签回退单色渲染
  测试: missing_language_falls_back_to_muted
  假设 围栏没有语言标签
  当 渲染该代码块
  那么 代码行以 muted 单色渲染

场景: 流式与提交路径高亮一致
  测试: streaming_flush_and_pager_highlight_consistently
  假设 同一代码块经流式水位 flush 与 pager 全量渲染
  当 比较两处的代码行 span 颜色
  那么 两处 token 着色一致（共用同一高亮器）
