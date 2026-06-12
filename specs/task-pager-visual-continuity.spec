spec: task
name: "pager 视觉连续性：默认背景 + 回看指示"
inherits: project
tags: [tui, rendering, pager, theme]
depends: []
estimate: 0.5d
---

## 意图

pinned 滚动模式让滚轮无缝进入 pager，但 pager 的 transcript 渲染显式刷
`surface_alt` 主题深色背景，而 inline live tail 刻意用终端默认背景——滚轮
一滚整屏从终端配色跳变成主题深色，用户感知为"界面变黑了"。同时 alt-screen
没有终端 scrollback，原生滚动条消失且无任何替代指示。本任务让 pager 的
transcript 改用默认背景（与 inline 视觉连续），并在 pager 回看（已上滚）时
在状态行给出明确指示。

## 已定决策

- 背景改动仅 gate 在 `transcript_pager_active`：pager 激活时 transcript
  Block 不再设置 `bg(surface_alt)`，只保留前景色——与 inline live tail 的
  渲染策略一致（app.rs 的 native-bg 注释同理）。其他全屏面（inspector、
  detail 模态底下的 chat layout）保持 `surface_alt` 不变。
- span 级背景同样剥除：消息块"气泡"底色在原生 scrollback 中本就不存在，
  pager 内保留会在终端背景上画出文字形状的主题色条纹（实测反馈"多了很多
  黑色背景"）。pager 激活时把 transcript 各行/span 的 bg 清空，与
  scrollback 视觉完全一致；非 pager 面不受影响。
- 滚动指示用文字不用数字：`transcript_scroll` 是饱和累加的原始值，渲染时
  才按内容高度钳制，直接显示行数会在过滚时虚高误导。pager 内已上滚
  （`transcript_scroll > 0`）时状态行显示"回看中"提示（含回到底部的键位），
  回到底部即消失。
- 不在 alt-screen 里模拟原生滚动条（FrameLike 无 StatefulWidget 通道，
  且 pager 滚动有键位/滚轮，文字指示已闭环）。

## 边界

### Allowed Changes
- src/app.rs
- locales/en.yml
- locales/zh.yml
- tests/pager_visual_continuity_contract.rs
- specs/**

### Forbidden
- 不改变 inline viewport 与 scrollback 写入路径的渲染策略。
- 不改动 inspector / detail 模态的 `surface_alt` 背景。
- 不引入新 crate 依赖。

## 排除范围

- alt-screen 内的图形滚动条。
- 主题系统改造或新主题。
- inline 模式的任何视觉变化。

## 完成条件

场景: pager 激活时 transcript 用默认背景渲染
  测试: pager_transcript_uses_default_background
  假设 pager 已激活且会话含已提交历史
  当 渲染全屏 chat 布局
  那么 transcript 区域单元格背景为终端默认色（不再是 surface_alt）

场景: pager 内消息块不带主题底色
  测试: pager_message_blocks_have_no_span_background
  假设 pager 激活且可视区域含助手消息块
  当 渲染全屏 chat 布局
  那么 transcript 区域所有单元格背景均为终端默认色（无文字形状色条）

场景: pager 回看时状态行显示回看指示
  测试: pager_status_shows_reviewing_indicator
  假设 pager 激活且用户已向上滚动
  当 渲染状态行
  那么 状态行包含回看提示文本

场景: pager 在底部时不显示回看指示
  测试: pager_status_hides_indicator_at_bottom
  假设 pager 激活且 transcript_scroll 为 0
  当 渲染状态行
  那么 状态行不包含回看提示
  但是 仍显示 pager 键位提示

场景: 非 pager 的全屏面背景不受影响
  测试: non_pager_fullscreen_keeps_surface_background
  假设 pager 未激活（如 detail 模态下的 chat 布局）
  当 渲染全屏 chat 布局
  那么 transcript 区域保持 surface_alt 背景（既有视觉不回归）
