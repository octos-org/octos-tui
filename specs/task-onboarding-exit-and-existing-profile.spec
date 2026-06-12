spec: task
name: "onboarding 逃生门：退出项 + 使用已有 profile"
inherits: project
tags: [tui, onboarding, menu, ux]
depends: []
estimate: 0.5d
---

## 意图

首启 onboarding 向导有两个 UX 死角：① 没有任何可见的退出方式（Esc 被刻意
吞掉以防把用户卡在空界面，Ctrl+Q 不可发现）；② data-dir 里已有 profile 的
用户若启动时忘传 `--profile-id`，会被迫面对"创建新 profile"表单——尽管
`/onboard profile <id>` 命令早已支持指定已有 profile 并跳过创建，但界面上
完全不可发现。本任务在创建 profile 表单中补两行：**"使用已有 profile
（输入 ID）"** 编辑项与**"退出 octos-tui"**项，把既有能力变成可见选项。

## 已定决策

- 零新机制，纯复用：已有 profile 行复用 `onboarding_edit_item` +
  `/onboard profile ` composer 模板（落到既有 `OnboardingAction::SetProfileId`，
  设置非空 id 后 `effective_profile_id` 命中、向导自动切到 provider setup）；
  退出行复用 `MenuAction::Local(LocalAction::Exit)`（落到既有
  `exit_requested` 退出链路）。
- 不放宽 Esc：根 onboarding 菜单继续吞 Esc（防误触把首启用户丢进空界面，
  issue #5 的既有决策）；显式退出项是它的可发现替代。
- 不做 profile 列表选择：UI 协议没有 `profile/local/list` 方法，TUI 不拥有
  server 的 data-dir、不得自行读盘（project spec"渲染服务器真相"不变量）。
  列表选择待后端广告新方法后另起 task（B 类跨仓 gap）。
- 新文案落 `locales/en.yml` + `locales/zh.yml`，退出项描述里提示
  `--profile-id <id>` 启动参数（治本路径）。

## 边界

### Allowed Changes
- src/menu/providers.rs
- locales/en.yml
- locales/zh.yml
- tests/onboarding_escape_hatch_contract.rs
- specs/**
- TUI-使用指南.md

### Forbidden
- 不修改 `handle_menu_escape` 的 Esc 吞没语义。
- 不新增 AppUI 协议方法或本地读取 server data-dir。
- 不改动 `/onboard` 命令语法与 `SetProfileId` 既有行为。

## 排除范围

- profile 列表枚举与选择 UI（等待后端 `profile/local/list`）。
- 删除/重命名 profile。
- legacy email-OTP onboarding 分支的同等改造。

## 完成条件

场景: 创建表单提供"使用已有 profile"编辑行
  测试: onboarding_create_menu_offers_existing_profile_row
  假设 后端广告 profile/local/create 且尚无 profile
  当 首启自动打开 onboarding 向导
  那么 创建表单包含 id 为 onboard.local.profile_id 的编辑行

场景: 输入已有 profile id 后向导跳过创建步骤
  测试: existing_profile_id_skips_creation_step
  假设 onboarding 创建表单已打开
  当 用户通过 /onboard profile 输入一个已有 profile id
  那么 向导切换到 provider setup 步骤
  并且 创建 profile 的行不再出现

场景: 创建表单提供退出行且触发后请求退出
  测试: onboarding_exit_row_requests_app_exit
  假设 onboarding 创建表单已打开
  当 用户选中退出行并确认
  那么 exit_requested 置位（事件循环据此退出进程）

场景: 空 profile id 不改变向导步骤
  测试: empty_profile_id_keeps_creation_step
  假设 onboarding 创建表单已打开
  当 用户输入空白的 profile id
  那么 向导仍停留在创建表单
  但是 不产生任何后端请求

场景: provider setup 步骤同样提供退出行
  测试: provider_setup_step_offers_exit_row
  假设 profile 已解析、向导处于 provider setup 步骤
  当 构建该步骤的菜单
  那么 菜单包含退出行（该步骤同样挂在根菜单 id 下、Esc 被吞，无退出行即被困）

场景: Esc 仍不关闭根 onboarding 向导
  测试: escape_still_keeps_onboarding_open
  假设 onboarding 向导在首启自动打开
  当 用户按下 Esc
  那么 向导保持打开（防误触卡死的既有决策不回归）
