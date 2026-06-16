spec: task
name: "pager 纯函数 scrollbar model + 可视位置指示"
inherits: project
tags: [tui, rendering, pager, scrollbar]
depends: [task-geometry-helper, task-pager-visual-continuity]
estimate: 0.5d
---

## 意图

herdr 的 scrollbar 值得借鉴的是"先用纯函数算可视模型，再渲染"这个局部模式，
不是它的 PTY/multiplexer 架构。octos-tui 的 transcript pager 运行在
alt-screen，终端原生 scrollback 不可见；已有状态行能提示"回看中"，但缺少
直观的位置感。本任务给 pager 增加一个轻量可视位置条，并把滚动条 thumb 的
几何计算抽成纯函数，便于契约测试和后续鼠标 hit-testing。

## 已定决策

- 只在 `transcript_pager_active` 的全屏 chat layout 渲染位置条；inline
  scrollback、live tail、inspector/detail 背景不受影响。
- 不使用 ratatui `StatefulWidget`：当前 `FrameLike` 已暴露 buffer，位置条作为
  pager transcript 的右侧 overlay 绘制即可，不扩展 trait 或引入依赖。
- scrollbar model 是纯函数：输入 transcript scroll metrics 与 track rect，
  输出 thumb 的 top/height；没有 overflow 或 track 不可用时返回 `None`。
- 位置条只表达可视位置，不新增拖拽、不改变滚轮/键盘滚动语义，也不默认捕获
  inline 鼠标。
- 状态行"Reviewing"提示保留；它表达模式/回到底部路径，scrollbar 表达相对
  位置，两者职责不同。

## 边界

### Allowed Changes
- src/app.rs
- tests/pager_visual_continuity_contract.rs
- specs/**

### Forbidden
- 不改变 transcript 行内容、wrap 宽度、滚动步进或 `transcript_scroll` 语义。
- 不引入 herdr 的 async Notify/AtomicBool/redraw loop、PTY 字节解析、默认鼠标
  捕获或常驻 alt-screen。
- 不新增 crate 依赖。

## 排除范围

- 鼠标拖拽 scrollbar、hover hit-testing、scrollbar 点击跳转。
- 用户可配置 scrollbar 样式。
- inline 模式的原生 scrollback 替代实现。

## 完成条件

场景: 无 overflow 时不显示 pager scrollbar
  测试: scrollbar_thumb_hidden_without_overflow
  假设 transcript 总行数不超过可视行数
  当 计算 scrollbar thumb
  那么 返回 None

场景: pager 底部时 thumb 位于 track 底部
  测试: scrollbar_thumb_places_bottom_at_track_end
  假设 transcript overflow 且 `scroll_from_bottom == 0`
  当 计算 scrollbar thumb
  那么 thumb bottom 与 track bottom 对齐

场景: pager 上滚后 thumb 向 track 顶部移动
  测试: scrollbar_thumb_moves_toward_top_when_scrolled_up
  假设 transcript overflow 且 scroll_from_bottom 增大
  当 计算 scrollbar thumb
  那么 thumb top 小于底部状态的 thumb top

场景: pager overflow 时渲染可视位置条
  测试: pager_scrollbar_renders_when_transcript_overflows
  假设 pager 已激活且 transcript 高度超过可视高度
  当 渲染全屏 chat 布局
  那么 transcript 右侧 lane 中出现 scrollbar track/thumb

场景: pager scroll 变化会移动可视 thumb
  测试: pager_scrollbar_thumb_moves_when_scrolled_up
  假设 pager 已激活且 transcript 可滚动
  当 用户 PageUp 上滚
  那么 thumb 行相对于底部状态向上移动

场景: 无 overflow 的 pager 不绘制位置条
  测试: pager_scrollbar_hidden_without_overflow
  假设 pager 已激活但 transcript 不可滚动
  当 渲染全屏 chat 布局
  那么 scrollbar lane 中没有 thumb/track 符号
