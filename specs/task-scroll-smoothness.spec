spec: task
name: "pager 滚动平滑度：滚轮单行步进 + 输入事件批处理"
inherits: project
tags: [tui, pager, scrolling, performance]
depends: []
estimate: 0.5d
---

## 意图

pinned 模式下 pager 滚动"不丝滑"有两个叠加原因：滚轮每个事件滚 4 行
（macOS 触控板惯性滚动每秒派发几十个细粒度事件，×4 后内容跳跃式移动）；
事件循环每处理一个输入事件就整屏重绘一次（惯性滚动期间每秒几十次全屏
repaint，终端写入排队产生迟滞感）。本任务把 pager 内滚轮改为 1 行步进，
并把同一帧内已排队的输入事件批量处理后只重绘一次。

## 已定决策

- 滚轮步进按面区分：transcript pager 内 1 行/事件（触控板惯性滚动平滑）；
  其余面（模态、workspace/git pane、inline live tail）保持 4 行/事件
  （适配点击式滚轮鼠标的既有手感）。
- 输入事件批处理在事件循环主体实现：读到首个事件后，把所有已排队事件
  （`event::poll(0)` 非空期间）在同一帧内全部处理，统一置一次 dirty 再
  重绘；批大小上限 64 防止病态事件流饿死渲染。Quit 在批内即时生效。
- 不改变"redraw on change"不变量：空闲时依旧零终端写入，批处理只是把
  N 个事件的 N 次重绘合并为 1 次。

## 边界

### Allowed Changes
- src/event_loop.rs
- tests/scroll_smoothness_contract.rs
- specs/**

### Forbidden
- 不改变键盘 PageUp/PageDown 的 8 行步进。
- 不引入定频重绘（保住原生选区的既有不变量）。
- 不引入新 crate 依赖。

## 排除范围

- 像素级/半行平滑滚动（终端单元格粒度之外）。
- 滚动动画或缓动。
- 后端事件（`drain_backend_events`）的批处理策略（已有，不动）。

## 完成条件

场景: pager 内滚轮以单行步进
  测试: pager_wheel_scrolls_one_line_per_event
  假设 pager 已激活且在底部
  当 收到一个滚轮上滚事件
  那么 transcript_scroll 恰好增加 1 行

场景: 非 pager 面滚轮保持粗粒度步进
  测试: non_pager_wheel_keeps_coarse_step
  假设 inline 模式焦点在 workspace pane
  当 收到一个滚轮上滚事件
  那么 该 pane 滚动 4 行（既有手感不回归）

场景: 键盘翻页步进不受影响
  测试: keyboard_paging_step_unchanged
  假设 pager 已激活
  当 用户按 PageUp
  那么 transcript_scroll 增加 8 行（键盘翻页粒度不变）
