spec: task
name: "onboarding 向导显示服务器已保存的 provider 配置"
inherits: project
tags: [tui, onboarding, menu, server-truth]
depends: []
estimate: 0.5d
---

## 意图

profile 已配置过 LLM provider（如 alex 的 moonshot/kimi-k2.6）的用户打开
`/onboard` 向导时，provider setup 步骤把模型系列/模型/线路/密钥全部显示为
"未设置"，测试/保存行也被"选择不完整"误禁用——因为这些行只读本次启动的本地
草稿，而服务器已保存配置需要 `profile/llm/list` 拉取，向导却从不自动发起。
本任务让向导在 profile 已解析时自动 hydrate 服务器真相，并在草稿为空时回退
显示已保存值，消除"配置丢了"的错觉。

## 已定决策

- 自动拉取走既有 `profile/llm/list`（`APPUI_METHOD_MODEL_LIST`），且仅当
  能力已广告、profile 已解析、`profile_llm_state` 缺失或属于其他 profile 时
  发起（幂等，不重复请求）。三个触发点：`/onboard` 打开向导、用户设置已有
  profile id、capabilities 首启自动弹出向导。
- 显示规则"草稿优先、已保存回退"：模型系列/模型/线路行在本地草稿为空时显示
  `profile_llm_state.primary_provider()` 的已保存值并标注"（已保存）"；
  API 密钥行在草稿为空且 `has_api_key` 为真时显示"已保存于档案"。草稿一旦
  有值即覆盖显示（编辑流程不变）。
- 不本地伪造：显示的已保存值全部来自服务器返回的 `profile_llm_state`，
  TUI 不读 profile JSON、不推断。解禁逻辑复用既有
  `onboarding_has_saved_primary_provider`（hydrate 后自然生效，不另写）。
- 引导一致性（#203 追加）：footer 提示与步骤进度必须和行显示同源——草稿
  为空且已保存 primary 有密钥时（即行显示"（已保存）"的同一条件），
  Provider/Connect/Save 步骤视为已由服务器真相满足，引导跳到 workspace/
  activate；草稿一旦有值回到草稿优先路径（编辑流程不变）。

## 边界

### Allowed Changes
- src/store.rs
- src/menu/providers.rs
- src/menu/wizard.rs
- src/model.rs
- locales/en.yml
- locales/zh.yml
- tests/onboarding_saved_provider_contract.rs
- specs/**

### Forbidden
- 不新增 AppUI 协议方法；不本地读取 server data-dir 或 profile JSON。
- 不改变草稿编辑、保存（`profile/llm/upsert`）的既有流程。
- 不引入新 crate 依赖。

## 排除范围

- provider 列表/多 profile 切换 UI。
- 把已保存值自动写回草稿（仅显示回退，不改草稿语义）。
- fallback providers 的同等显示改造。

## 完成条件

场景: 打开向导且 profile 已解析时自动拉取已保存配置
  测试: onboard_open_with_profile_hydrates_saved_provider
  假设 后端广告 profile/llm/list 且 profile 已解析、尚无 llm 状态
  当 用户执行 /onboard 打开向导
  那么 发出 profile/llm/list 请求（携带该 profile id）

场景: llm 状态已是当前 profile 时不重复拉取
  测试: hydrate_is_idempotent_for_current_profile
  假设 profile_llm_state 已属于当前 profile
  当 用户再次打开向导
  那么 不发出新的 profile/llm/list 请求

场景: 已保存的 provider 值回退显示在向导行上
  测试: provider_rows_fall_back_to_saved_values
  假设 服务器返回的 llm 状态含已保存的 primary（moonshot/kimi-k2.6 且有密钥）
  当 构建 provider setup 菜单
  那么 模型系列与模型行显示已保存值并带"已保存"标注
  并且 API 密钥行显示"已保存于档案"而非"未设置"

场景: 本地草稿优先于已保存值
  测试: draft_values_override_saved_display
  假设 llm 状态含已保存 primary 且用户已在草稿中选择其他系列
  当 构建 provider setup 菜单
  那么 模型系列行显示草稿值而非已保存值

场景: 无已保存配置时仍显示未设置
  测试: rows_show_not_set_without_saved_provider
  假设 llm 状态为空或无 primary
  当 构建 provider setup 菜单
  那么 模型系列/模型行仍显示"未设置"（不伪造服务器状态）

场景: 已保存配置满足向导引导（footer/进度与行显示一致，#203）
  测试: saved_provider_with_untouched_draft_skips_provider_guidance
  假设 llm 状态含已保存 primary（有密钥）且草稿为空
  当 构建 provider setup 菜单
  那么 footer 不再要求选择模型系列，转而指向 workspace 步骤
  并且 进度把 Provider/Connect/Save 标记为已完成（当前步 = Workspace）

场景: 草稿一旦有值引导回到草稿优先路径
  测试: draft_input_overrides_saved_guidance
  假设 llm 状态含已保存 primary 且用户已在草稿中选择其他系列
  当 构建 provider setup 菜单
  那么 footer 回到草稿路径的第一个未满足前置（不再指向 workspace）

场景: 已保存 primary 无密钥时不跳过引导
  测试: saved_provider_without_key_keeps_draft_guidance
  假设 llm 状态含已保存 primary 但 has_api_key 为假
  当 构建 provider setup 菜单
  那么 footer 仍走草稿路径（无密钥不能视为 Connect 已满足）

场景: 无已保存配置时引导不变
  测试: no_saved_provider_keeps_draft_guidance
  假设 llm 状态为空或无 primary
  当 构建 provider setup 菜单
  那么 footer 仍走草稿路径（provider 配置步骤继续把守引导）
