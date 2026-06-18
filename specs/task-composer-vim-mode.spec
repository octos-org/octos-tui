spec: task
name: "Composer Vim 模式：Normal/Insert 务实子集"
inherits: project
tags: [tui, composer, input, editing, vim]
depends: [task-composer-multiline]
estimate: 1d
---

## 意图

多行输入落地后（[[task-composer-multiline]]），为习惯 Vim 的用户提供模态编辑。本期做
**务实子集**：Normal/Insert 两态 + 常用 motion/operator，覆盖约 90% 日常编辑。默认关闭，
显式 opt-in（config `vim-mode` + 运行时 `/vimmode` 切换），开启后不改变非 Vim 用户的任何
现有行为。Visual 选区、寄存器 yank/paste、数字计数前缀（`3dd`）不在本期。

## 已定决策

- **开关**：CLI/config 复用既有 scroll-mode 模式——config 键 `vim-mode`（bool，默认
  false，含 `vim_mode` alias），运行时 `/vimmode`（别名 `vim-mode`）切换；状态承载
  `AppState.vim_mode: bool`，事件循环启动时从 `Cli` 播种（与 `pinned_scroll` 同模式）。
  并入 `/saveconfig` 持久化（与 theme/lang/scroll-mode 并列）。
- **模式状态**：`AppState.composer_mode: ComposerMode`（`Insert` | `Normal`，默认 `Insert`）。
  仅当 `vim_mode` 为真时生效；为假时 composer 行为与现状完全一致（等同恒在 Insert）。
  开启 vim 时进入 `Insert`，让打字立即可用；`Esc` 从 Insert 切 Normal。
- **Normal 态按键**（仅 `vim_mode && Normal && 聚焦 composer` 时拦截，先于普通插入）：
  - 移动：`h`/`l` 左右、`j`/`k` 上下行（复用 `move_composer_cursor_*`）、`0`/`$` 行首尾、
    `w`/`b` 词移、`e` 词尾（新增 `move_composer_cursor_word_end`）、`gg` 缓冲区首、`G` 缓冲区尾。
  - 编辑：`x` 删字符、`dd` 删整逻辑行、`dw` 删词、`cc` 改行（清空该行并进 Insert）。
  - 进 Insert：`i` 原地、`a` 右移一格、`A` 行尾、`I` 行首、`o` 下方开新行、`O` 上方开新行。
  - 多键序列用 `composer_vim_pending: Option<char>` 承载（`g`/`d`/`c` 前缀，下一键决议或清空）。
- **Enter 语义**：Normal 与 Insert 下普通 `Enter` 都发送（一致、不丢手感）；Insert 下
  `Shift+Enter`/`Ctrl+J` 换行（沿用 Phase 1）；Normal 下换行用 `o`/`O`。
- **模式指示**：`vim_mode` 开启时在 composer 提示串显示 `NORMAL`/`INSERT`（en/zh 双语齐备）。

## 边界

### Allowed Changes
- src/model.rs
- src/event_loop.rs
- src/cli.rs
- src/store.rs
- src/menu/registry.rs
- src/menu/types.rs
- src/app.rs
- locales/en.yml
- locales/zh.yml
- tests/composer_vim_contract.rs
- specs/**

### Forbidden
- `vim_mode` 关闭（默认）时不改变 composer 的任何现有行为或键位。
- 不改变普通 `Enter` 发送语义。
- 不引入新 crate 依赖；不启用鼠标捕获、不破坏 inline-scrollback 模型。

## 排除范围

- Visual 选区模式、寄存器 yank/paste/`p`、数字计数前缀（`3dd`、`2w`）。
- `.` 重复、宏、搜索 `/`、ex 命令 `:`。
- 跨视觉折行的移动（沿用 Phase 1 的逻辑行口径）。

## 完成条件

Rule: toggle-safety — 开关与默认安全

场景: 默认不启用 vim、行为零变化
  测试: vim_disabled_by_default_types_normally
  假设 未配置 vim-mode（默认关闭）
  当 composer 聚焦、用户输入可见字符 "h"
  那么 "h" 被插入文本（不被当作 motion）
  并且 composer_mode 不影响输入

场景: /vimmode 运行时切换
  测试: vimmode_slash_toggles_enabled
  假设 vim-mode 当前关闭
  当 执行 /vimmode
  那么 vim_mode 变为开启、composer_mode 为 Insert

场景: config 文件可解析 vim-mode
  测试: vim_mode_parses_from_config_file
  假设 JSON config 含 "vim-mode": true
  当 load_config_file 解析该文件
  那么 解析出 vim_mode 为真（未设置时为 None、最终默认 false）

Rule: mode-switch — 模式切换

场景: Esc 从 Insert 切到 Normal
  测试: esc_enters_normal_mode
  假设 vim 开启且处于 Insert
  当 用户按下 Esc
  那么 composer_mode 变为 Normal
  并且 此后可见字符不再插入文本

场景: i 从 Normal 进入 Insert
  测试: i_enters_insert_mode
  假设 vim 开启且处于 Normal
  当 用户按下 i
  那么 composer_mode 变为 Insert
  并且 此后可见字符正常插入

Rule: normal-motions — Normal 态移动

场景: hjkl 移动光标且不改文本
  测试: normal_hjkl_moves_cursor_without_editing
  假设 vim 开启、Normal、composer 含 "abc\nde"
  当 依次按 l 与 j
  那么 光标按字符/逻辑行移动
  并且 文本保持 "abc\nde" 不变

场景: 0 和 $ 跳到行首行尾
  测试: normal_line_start_and_end
  假设 vim 开启、Normal、光标在某行中间
  当 按 0 再按 $
  那么 光标先到行首、后到行尾

场景: w/b/e 词移动
  测试: normal_word_motions
  假设 vim 开启、Normal、composer 含 "foo bar baz"
  当 按 w、b、e
  那么 光标分别落到下一词首、上一词首、当前/下一词尾

场景: gg 到缓冲区首、G 到缓冲区尾
  测试: normal_gg_and_g_buffer_bounds
  假设 vim 开启、Normal、多行文本、光标在中间
  当 按 g g 再按 G
  那么 光标先到文本开头、后到文本结尾

Rule: normal-edits — Normal 态编辑

场景: x 删除光标处字符
  测试: normal_x_deletes_char
  假设 vim 开启、Normal、composer 含 "abc"、光标在首
  当 按 x
  那么 文本变为 "bc"

场景: dd 删除整逻辑行
  测试: normal_dd_deletes_line
  假设 vim 开启、Normal、composer 含 "one\ntwo\nthree"、光标在第二行
  当 按 d d
  那么 "two" 行被删除、文本为 "one\nthree"

场景: dw 删除一个词
  测试: normal_dw_deletes_word
  假设 vim 开启、Normal、composer 含 "foo bar"、光标在首
  当 按 d w
  那么 "foo " 被删除

场景: cc 改行并进入 Insert
  测试: normal_cc_changes_line
  假设 vim 开启、Normal、composer 含 "one\ntwo"、光标在第一行
  当 按 c c
  那么 第一行内容清空、composer_mode 变为 Insert

Rule: insert-entry — 进入 Insert 的变体

场景: a/A/I/o/O 各自定位并进入 Insert
  测试: normal_insert_entry_variants_position_cursor
  假设 vim 开启、Normal、composer 含 "abc"
  当 分别按 a、A、I、o、O
  那么 光标按语义定位（右移/行尾/行首/下方新行/上方新行）且进入 Insert

Rule: submit-no-regress — 发送语义不回归

场景: Normal 态 Enter 仍发送
  测试: normal_enter_still_submits
  假设 vim 开启、Normal、composer 含非空文本
  当 按下无修饰 Enter
  那么 触发发送（与现状一致）

场景: 空文本按 x 不 panic
  测试: normal_x_on_empty_is_safe
  假设 vim 开启、Normal、composer 为空
  当 按 x
  那么 不发生 panic、文本仍为空
