spec: project
name: octos-tui
tags: [tui, appui-client]
---

## 意图

octos-tui 是 octos 后端的纯终端客户端：通过 JSON-RPC 的 Octos UI Protocol 与
`octos serve`（stdio/WebSocket）交互，自身不跑 agent、不执行 tool、不拥有
provider/sandbox/permission 真相。本 project spec 编码全仓不变量，所有 task
spec 以 `inherits: project` 继承。

## 约束

### 必须做

- TUI 只渲染服务器确认的真相：能力门控一律以服务器握手广告的
  capabilities 为准，未广告的 method/feature 渲染为 Disabled/Unsupported，
  绝不本地探测或本地伪造状态。
- 新增用户可见文案必须同时落 `locales/en.yml` 与 `locales/zh.yml`。
- 每个验收场景必须绑定显式 `测试:` selector，且只绑确定性契约测试
  （mock-backed / fixture），禁止绑定 flaky 的 stdio-live soak 测试。
- 状态变更只经 `Store` reducer（单向数据流）；`app::render*` 保持纯函数，
  不持状态、不发命令。

### 禁止做

- 禁止无正当理由新增 crate 依赖。
- 禁止 unsafe 代码（全仓 `unsafe_code = "deny"`）。
- 禁止破坏 inline scrollback 模型的三条设计不变量：普通 chat 不常驻
  alternate screen；普通 chat 在默认 `native` scroll-mode 下不启用鼠标捕获（保住原生选择/复制；`--scroll-mode pinned` 为用户显式 opt-in 豁免）；
  空闲时不重绘终端（redraw on change，保住用户选区）。

## 验收标准

以下场景把上述全仓不变量绑到既有的确定性测试上，作为仓级 guard 的
冒烟门；逐任务的行为契约在各 task spec 中另行覆盖。

场景: 危险权限状态只渲染服务器确认的真相
  测试: permissions_fixture_requires_server_confirmed_dangerous_status
  假设 服务器返回包含危险状态的权限 fixture
  当 客户端渲染权限菜单
  那么 危险行只在服务器确认时标记，TUI 不本地伪造状态

场景: 用户可见文案在 en 与 zh 语言包同时可解析
  测试: resolves_keys_in_en_and_zh
  假设 locales/en.yml 与 locales/zh.yml 均已加载
  当 以 en 与 zh 两个 locale 解析同一文案 key
  那么 两个语言包都返回译文而非回退到 key 本身

场景: inline 模式不启用鼠标捕获且历史留在原生 scrollback
  测试: inline_mode_never_enables_mouse_capture
  假设 普通 chat inline 模式
  当 事件循环渲染与轮询
  那么 不启用鼠标捕获，已提交历史不进入 inline viewport
