spec: task
name: "scrollback 空白疤痕缓解"
inherits: project
tags: [tui, rendering, scrollback, viewport]
estimate: 1d
---

## 意图

turn 活跃时 inline live tail 撑高，turn 结束内容提交进只追加的原生 scrollback、
实时区收缩，在历史里留下高水位的空白行（"疤痕"）。本任务通过裁剪 live tail
尾部空行、收紧高度上限、统一两条高度路径，最小化遗留空白行。终端 scrollback
只追加，疤痕无法根治，本任务为缓解。

## 已定决策

- 裁剪尾部空行：live tail 行集尾部的 spacer 空行在测高与渲染前一并 trim，使
  inline viewport 紧贴实际内容高度，降低高水位。
- 收紧 max_tail：`live_tail_height_with_finalization` 的上限不再固定 18，改为随
  终端高度按比例（如 height 的一半上限），避免大终端也吃满固定值。
- 统一高度口径：`live_ui_height_with_finalization` 计入的 tail 与实际渲染使用的
  tail 行集来自同一计算，消除 off-by 错位。
- 严守既有不变量：不破坏 pager / pinned / "空闲不重绘" / 原生选区；committed 历史
  仍只在 scrollback、不进 inline viewport。

## 边界

### Allowed Changes
- src/app.rs
- tests/scrollback_scar_contract.rs
- specs/**

### Forbidden
- 不修改 insert_history / viewport.rs 的写入机制。
- 不改变 pager 与 pinned 模式的既有行为。
- 不把 committed 历史移进 inline viewport。

## 排除范围

- 彻底消除疤痕（终端只追加，物理不可行）。
- 改写终端 scrollback 已有内容。

## 完成条件

场景: live tail 尾部空行被裁剪
  测试: live_tail_trims_trailing_blank_rows
  假设 live tail 内容块之后带有尾部 spacer 空行
  当 计算 live tail 行集
  那么 末尾不含空白行（最后一行为实际内容）

场景: tail 高度上限随终端高度收紧
  测试: tail_height_cap_scales_with_terminal
  假设 一个高终端与一个矮终端、live tail 内容超过两者上限
  当 分别计算 tail 高度
  那么 高终端的上限不超过其高度的一半，且不再是固定 18

场景: 两条高度路径口径一致
  测试: live_ui_height_matches_rendered_tail
  假设 同一活跃 turn 状态
  当 比较 live_ui_height 计入的 tail 行数与实际渲染 tail 行数
  那么 两者相等（无 off-by 错位）

场景: turn 结束后遗留空白行不超阈值
  测试: settled_turn_leaves_bounded_blank_rows
  层级: 集成
  替身: RecordingBackend draw-bytes（无真实 tty）
  假设 一个 turn 活跃后结束、内容提交进 scrollback
  当 收缩后重绘
  那么 viewport 之上连续空白行数不超过既定阈值

场景: committed 历史不回流进 inline viewport
  测试: committed_history_stays_in_scrollback
  假设 会话含已提交历史且非 pager 模式
  当 渲染 inline viewport
  那么 已提交历史不出现在 viewport（既有不变量不回归）
