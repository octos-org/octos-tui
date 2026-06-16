spec: task
name: "渲染几何 helper：先算 Rect 再渲染"
inherits: project
tags: [tui, rendering, geometry, refactor]
depends: []
estimate: 0.5d
---

## 意图

当前 `app.rs` 中 chat、pager、menu、modal 的 `Rect` 计算散落在渲染函数里，
渲染和几何决策耦合在一起。后续要做 pager scrollbar、统一 hint-bar、鼠标
hit-testing 或 `/activity` overlay 时，测试只能通过像素文本间接推断布局，
风险和维护成本都偏高。

本任务先做低风险地基：把主 chat surface 的区域切分抽成纯几何 helper，
让测试可以直接验证 transcript/menu/autonomy/harness/composer/status 的 Rect
关系，同时保持现有视觉、输入、scrollback、mouse-capture 行为完全不变。

## 已定决策

- 第一阶段只抽主 chat layout：`render_chat_layout` 中的 transcript、menu、
  autonomy、harness、composer、status 区域由 `chat_layout_areas` 统一计算。
- 几何 helper 是纯函数：输入为 `AppState` 与 terminal `Rect`，输出为
  `ChatLayoutAreas`；不渲染、不发 AppUI command、不修改状态。
- 渲染继续复用既有 `FrameLike` 路径，`render_chat_layout` 只消费 helper
  返回的区域，不改变任何 widget、style、copy/selection 或 scrollback 逻辑。
- menu 高度预算继续沿用现有规则：先算 desired menu height，再受
  `min_transcript_height + composer + autonomy + harness + status` 预算限制。
- 本任务不引入 herdr 的默认鼠标捕获、常驻 alt-screen 或 PTY/multiplexer 架构；
  它只借鉴"先算几何再渲染"这个局部模式。

## 边界

### Allowed Changes

- src/app.rs
- tests/**
- specs/**

### Forbidden

- 不改变 inline viewport 与 native scrollback 模型。
- 不改变 transcript pager 的 alt-screen 进入/退出条件。
- 不改变 menu/provider/onboarding/approval 的可见行为或 command dispatch。
- 不引入新 crate 依赖。

## 排除范围

- pager 图形 scrollbar 或拖拽交互。
- 统一 hint-bar 组件。
- `/activity` 或 `/sessions` navigator overlay。
- 输入 raw-byte parser 或全量可配置 keybind。

## 完成条件

场景: chat layout 几何保持 composer 和 status 钉底
  测试: chat_layout_areas_keep_composer_and_status_at_bottom
  假设 普通 chat surface 在 80x24 终端中渲染
  当 计算 ChatLayoutAreas
  那么 status 位于最后一行
  并且 composer 紧贴 status 上方

场景: menu 区域受 transcript 最小高度预算限制
  测试: chat_layout_areas_clamp_menu_to_transcript_budget
  假设 短终端中打开 slash/menu surface
  当 计算 ChatLayoutAreas
  那么 menu 高度不会挤掉最小 transcript 区域
  并且 composer/status 仍保留可用区域

场景: 渲染路径消费同一份几何结果
  测试: render_chat_layout_matches_chat_layout_areas
  假设 pager 激活且会话含历史
  当 渲染 full chat layout
  那么 composer 文本出现在 ChatLayoutAreas.composer 内
  并且 transcript 文本不会进入 composer 区域

场景: 全仓渲染契约不回归
  测试: cargo test --test transcript_pager_contract --test pager_visual_continuity_contract --test slash_popup_contract
  假设 本任务只做几何抽取
  当 运行 pager/menu 相关契约测试
  那么 既有 pinned composer、pager 背景、slash popup 行为保持通过
