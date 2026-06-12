spec: task
name: "流式 markdown 块级水位：scrollback 增量按完成块边界切割"
inherits: project
tags: [tui, rendering, markdown, streaming, scrollback]
depends: []
estimate: 1d
---

## 意图

流式回复的稳定前缀按"最后一个换行"切割后增量写入只追加的终端 scrollback，
每个增量批次被当作独立 markdown 文档渲染——代码块/表格/段落被批次边界切碎
（开栏闭栏分家、表格永不对齐、段落断裂、每批多注入一个 • 弹点），且错误随
scrollback 永久固化，观感即"markdown 没被渲染"。本任务引入 codex 谱系的
块级水位状态机：flush 切割点只落在**已完成块**的边界上，完成块自包含、
渲染输出固定，增量批次独立渲染即正确。

## 已定决策

- 切割函数 `stable_live_reply_prefix_len`（最后换行）升级为块级状态机
  `stable_markdown_prefix_len`：逐完整行扫描，跟踪代码围栏开闭与
  块连续性；水位只推进到"围栏闭合处"或"空行边界（段落/表格/列表块结束）"，
  未闭合围栏、进行中的表格/段落不切割（滞留在 live tail 继续全量重渲）。
- 不自动补闭合、不事后修正：scrollback 只追加，未完成块一律 hold——
  宁可 flush 滞后，不写入错误渲染（业界 web 端的"补闭合重绘"路线在终端
  scrollback 约束下不可行，明确否决）。
- `• ` prose 弹点只属于回复的第一个增量批次：后续增量与 turn 结束提交的
  尾部都按"续写"渲染（无弹点），一条回复在 scrollback 中只有一个弹点。
- 水位语义不变：`LiveTurnFinalization.reply_flushed_text` 仍是已 flush 的
  文本前缀，turn 结束提交时尾部从同一水位起渲染（拼接无重复无丢失）。

## 边界

### Allowed Changes
- src/app.rs
- tests/markdown_stream_flush_contract.rs
- specs/**

### Forbidden
- 不修改 `insert_history.rs`/`viewport.rs` 的写入机制与行核算。
- 不引入新 crate 依赖（状态机自研，不引 streamdown-parser）。
- 不改变已提交消息（非流式路径）的渲染。

## 排除范围

- 代码块语法高亮、链接/引用块等渲染器语法扩展（另起任务）。
- 已固化在用户终端 scrollback 里的历史错误渲染（物理不可修正）。
- pager / live tail 视口渲染路径（全量重渲，本就正确）。

## 完成条件

场景: 未闭合代码块不被切割进 scrollback
  测试: unclosed_fence_holds_flush_watermark
  假设 流式文本停在代码围栏开启之后、闭合之前
  当 计算下一个 flush 水位
  那么 水位停在围栏块开始之前的块边界（围栏内容不进入增量）

场景: 围栏闭合后整块一次性 flush 且渲染完整
  测试: closed_fence_flushes_as_complete_block
  假设 流式文本中代码围栏已闭合且其后有空行
  当 渲染该增量批次
  那么 增量行包含成对的代码块边框（开栏带语言标签、闭栏）

场景: 进行中的段落不被切割
  测试: open_paragraph_holds_flush_watermark
  假设 流式文本以未遇空行的段落文字结尾
  当 计算下一个 flush 水位
  那么 该段落整体不进入增量（留在 live tail）

场景: 仅首个增量批次携带回复弹点
  测试: only_first_batch_carries_prose_marker
  假设 一条回复分多个增量批次 flush
  当 分别渲染第一批与后续批次
  那么 仅第一批含 • 弹点，后续批次无弹点

场景: turn 结束提交的尾部与已 flush 前缀无缝拼接
  测试: commit_suffix_joins_flushed_prefix_without_marker
  假设 回复已有非空 flush 水位、turn 结束提交完整消息
  当 渲染提交尾部
  那么 尾部从水位处起渲染且不再注入弹点（拼接无重复内容）

场景: 全部为完成块时水位推进到文末
  测试: fully_settled_text_flushes_to_end
  假设 流式文本以空行结尾且无未闭合结构
  当 计算 flush 水位
  那么 水位等于全文长度（行为与旧实现一致，flush 不滞留）
