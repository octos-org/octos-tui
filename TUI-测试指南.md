# 如何重新开始测试 octos-tui

> 适用环境:macOS,octos-tui 仓库在 `/Users/zhangalex/Work/Projects/FW/octos-tui`(HEAD `840ee2e`,与本方案研究基线一致);后端 `octos` 在 `~/.cargo/bin/octos`(已是新版,已在 PATH)。`MOONSHOT_API_KEY` 已 export。

## 0. 现状核对(已实测,先看结论)

| 项 | 实测结果 | 对你的影响 |
|---|---|---|
| 旧 TUI | PID **48675**,cwd=`…/octos-tui`,启动参数 `--mode protocol … --data-dir ./octos-data` | `./octos-data` 解析为绝对路径 `…/octos-tui/octos-data` |
| 旧 serve | PID **48699**(48675 的子进程),**正持有 redb 锁** `octos-data/profiles/alexz/data/episodes.redb`(lsof 已确认) | 不清掉它,新 serve 打开 alexz 会撞 `DatabaseAlreadyOpen` → 该 profile `/api/chat` 返回 503 |
| **rebuild 其实已经完成** | **没有任何 octos-tui 的 cargo 进程在跑**;二进制 `target/release/octos-tui` mtime `14:47` > 源码 `14:44`;HEAD=`840ee2e` | 磁盘上的二进制**就是** `840ee2e` 新构建。只有运行中的 48675 还钉在旧 inode 上。**不需要"等 rebuild",杀掉旧进程即可直接用新二进制** |
| launch json | `/Users/zhangalex/octos-tui-launch.json`:`mode=protocol`,`stdio-command="octos serve --stdio --solo --data-dir <绝对octos-data>"`,`profile-id=alexz`,`session=fd1a6150-6304-416b-bf3e-572367dbc85d` | 见路径 B;注意它指向**同一个** octos-data,所以必须先清掉 48699 |
| 会话文件 | `octos-data/sessions/` **是空的**,该 session UUID 在磁盘上不存在 | 路径 B 会**新建一个空会话**(复用 profile/模型,但不是"恢复历史") |
| PID 529 | `~/.octos/bin/octos serve --port 8080 --host 0.0.0.0`,用默认 `~/.octos`,不碰 octos-data | **与 TUI 测试无关,不要动**。仅安全提示:它对外暴露 8080(带 `--auth-token`),不需要可单独 `kill 529` |

backstop 自检(随时可跑):
```bash
pgrep -fl 'cargo' | grep -i octos-tui          # 空 = 没有 rebuild 在跑
ls -la /Users/zhangalex/Work/Projects/FW/octos-tui/target/release/octos-tui   # mtime 应 ≥ 源码改动时间
```

---

## 1. 重启前置:干净退出旧 TUI(头号坑在这里)

octos-tui **没有任何信号处理器**。优雅退出时,它通过 tokio `kill_on_drop(true)` 级联 SIGKILL 掉自己 spawn 的 serve(48699);但用 `kill`/`kill -9`/关终端窗口(SIGHUP)从外部杀 TUI **都不会跑析构**,48699 会被孤儿化、继续锁住 redb——这正是"新 TUI 连不上/无可用 profile"的根因。

### ① 首选:在 TUI 里优雅退出
- **`Ctrl+Q`**:任意焦点/任意界面都能退(在 `handle_key` 顶部无条件拦截)。**首选这个**。
- 纯 `q`:仅当焦点不在输入框(Composer)且无菜单时才退;菜单打开时 `q` 可能被当成搜索字符吞掉。
- **`Ctrl+C` 不退出**(只中断当前 turn);**`Esc` 不退出**(切焦点/关 overlay)。别用它们退出后改去 `kill`。
- 若开着菜单/overlay:先 `Esc` 关掉,再 `Ctrl+Q`(注意:onboarding 根菜单的 Esc 被吞,见路径 B)。

### ② 强制验证(务必执行,这是本节核心)
```bash
ps -p 48699; pgrep -fl 'octos serve.*octos-data'
```
若 48699 仍在 → 说明析构没跑,手动清理:
```bash
kill 48699 48675        # 必要时 kill -9 48699
```

### ③ 如果只能从外部杀 TUI
**不要只 `kill 48675`**(serve 不会被收割)。两个一起杀,或杀整个进程组,然后回到 ②:
```bash
kill 48675 48699        # 或者 kill -- -<pgid>
```

### ④ 关于 rebuild 与 PID 529
- rebuild **已完成**(见第 0 节),无需等待。macOS 上 cargo 用 rename 换新 inode 是安全的(运行中的旧进程钉旧 inode);只要你**先退出旧 TUI**,下次启动就是新二进制。
- **PID 529 不用动**,与 octos-data 测试互不影响。

---

## 2. 让界面变成中文

启动期优先级(高→低):`--lang` > 配置文件 `lang` > `OCTOS_LANG` > `LANG` > 默认 En。任选其一:

```bash
# 方式 A:命令行 flag(优先级最高,推荐)——注意只接受精确 en|zh
… octos-tui --lang zh …
# 方式 B:环境变量(走前缀匹配,zh_CN.UTF-8 也认)
OCTOS_LANG=zh … octos-tui …
LANG=zh_CN.UTF-8 … octos-tui …
# 方式 C:在 launch json 里加一行 "lang": "zh"
```

坑:
- **`--lang zh_CN` 会 clap 报错**(flag 只认精确 `en`/`zh`);带 locale 后缀只能用 `OCTOS_LANG`/`LANG`/运行时 `/lang`。
- 运行时 `/lang zh` 可切换,但**不持久化**,重启回到启动期解析值。
- 已知小瑕疵:`/help` 里 **`/copy` 的说明永远是英文**(硬编码,非 i18n key),其余应为中文。

---

## 3. 两条路径(任选其一)

> 两条路径都建议加 `--lang zh`,并确认在**同一个 export 了 `MOONSHOT_API_KEY` 的 shell** 里启动(spawn 的 `octos serve` 子进程会继承它;它不在 BLOCKED_ENV_VARS 里)。

### 路径 A:全新走 onboarding(测新流程 + 中文)

目的:从零体验 7 步向导。**用一个全新的、独立的、绝对路径 data-dir**(避免复用现有 alexz profile 触发自动解析而跳过欢迎页,也避免撞 48699 的锁),并且**不要带 `--config`/`--profile-id`/`--session`**。

```bash
export MOONSHOT_API_KEY=...   # 若当前 shell 没有
/Users/zhangalex/Work/Projects/FW/octos-tui/target/release/octos-tui \
  --lang zh \
  --mode protocol \
  --stdio-command "octos serve --stdio --solo --data-dir /Users/zhangalex/Work/Projects/FW/octos-tui/octos-data-onboard"
```

要点与步骤(已对照源码修正):
- **向导是全屏(alt-screen),不是 inline viewport**(任务原始说法有误;inline 滚动屏是 onboarding **之后**的编码会话)。
- 欢迎页不会"一启动就弹",要等后端 capabilities 事件到达(广告 `profile/local/create`)且 `sessions` 为空、无活动菜单时才自动打开。空白 data-dir 满足这些条件。
- 渲染的标题是向导头 **"Step 2 of 7 — Profile"**(语言默认已完成,所以从第 2 步起)。
1. **欢迎页**:填 Full name / Username / **Email(必填!** 后端 `profile/local/create` 拒绝空 email,尽管文案说"仅本地元数据")。可在行内输入,或用 `/onboard name <名>` `/onboard username <handle>` `/onboard email <地址>`。三项都填齐后 "Create profile" 才可用。
2. **创建本地 profile** → 成功后同一帧翻到 "Octos Setup Wizard" provider 页,光标被强制移到 **Model family** 行,并自动触发加载模型 catalog(无需手动)。
3. **选 Moonshot 系列 → kimi 模型 → route**(子菜单;catalog 未加载时 Family 菜单显示 Unavailable,可 `/onboard catalog` 重载)。行内:`/onboard family <id>` `/onboard model <id>` `/onboard route <id>`。
4. **粘贴 API key**(填 `MOONSHOT_API_KEY` 的值;行内 `/onboard key <secret>`,会被掩码)。
5. (可选)`/onboard test` → **`/onboard save`**(保存 provider)。**保存只解锁 "Continue to Workspace →",不是直接开会话**(README 旧文档有误,UX2 把 Activate 拆到了独立的 Workspace 页)。
6. **Continue to Workspace →** → **Validate workspace** → **Activate(= Open coding session)**。Activate **同时要求**:已保存 provider **且** workspace 校验通过,否则 `session/open` 不会触发。
7. 进入编码会话(此时才是 inline 滚动 UI)→ 在 Composer 输入、回车。

### 路径 B:复用 alexz profile,直接进会话(最快)

**前提:必须先完成第 1 节,确认 48699 已清掉**(launch json 指向同一个 octos-data,锁还在就会 503)。

```bash
export MOONSHOT_API_KEY=...   # 若当前 shell 没有
/Users/zhangalex/Work/Projects/FW/octos-tui/target/release/octos-tui \
  --lang zh \
  --config /Users/zhangalex/octos-tui-launch.json
```

诚实说明(直接回答"是否/何时还会弹 onboarding"):
- **用这个确切的 `--config`,onboarding 不会弹**——因为 launch json 带了 `session`,启动时 `state.sessions` 会被 snapshot 同步预填(早于任何协议响应被读取),`maybe_open_onboarding` 的第一个守卫 `!sessions.is_empty()` 直接短路。**不存在 capabilities/session 竞态。**
- 但磁盘上没有这个 session UUID,所以服务端 `get_or_create` 会**新建一个空会话**:你复用的是 **profile + 模型(alexz / moonshot-kimi)**,进去是**全新空对话**,不是恢复历史。若你期待看到旧消息,这里不会有。
- **onboarding 只会在两种情况下弹**:(a) 忘了带 `--config`(则 mode 退回 Mock、无 session、sessions 为空,菜单就可能出现);(b) 把 json 里的 `session` 键删了。`--config` **不会被自动发现**,必须显式传。
- **最短绕过**:万一弹了 onboarding,**根菜单的 Esc 被吞掉(无操作),按 Esc 出不去**——直接 `Ctrl+Q` 退出,带着含 `session` 的 `--config` 重启即可。

---

## 4. 测试 checklist(新版值得逐项试)

> 斜杠命令在 Mock 下也能跑,但 **`/thinking` 需要活动会话**(否则报 `thinking.no_session`),`/copy` 需要有上一条回复。**建议从一个真实编码会话内运行**:路径 B 启动后立刻可用;路径 A 要等 Activate 进会话之后。

**i18n / 主题 / 思考(`always()`,即时可测):**
- `/lang` → 弹菜单(English / 中文,`*` 标当前;数字 1/2 快捷)
- `/lang zh` → "已切换到中文。" ；`/lang en` → "Language switched to English." ；`/lang zh_CN.UTF-8` → 中文 ；`/lang fr` → "Unknown language…",不改变
- `/theme` → 菜单(terminal/codex/claude/slate/solarized,`*` 当前),选 Claude → "主题:claude"
- **`/theme claude`(行内)→ 参数被忽略,只打开菜单**(`/theme` 无行内切换;行内主题只有启动 flag `--theme`)
- `/thinking` → 菜单(Default/Low/Medium/High/Max);`/thinking high` → 设置;`/thinking med` → Medium;`/thinking default`(或 `reset`)→ 清除;`/thinking bogus` → unknown
- `/copy` → "已复制上一条回复到剪贴板(N 个字符)"(无内容则 "Nothing to copy yet")
- **`Ctrl+Y`** = `/copy`(走 OSC52,`ESC]52;c;<base64>BEL`,tmux 自动包 DCS)→ **到别处粘贴验证**

**向导 / 其它:**
- `/setup` → 打开 onboarding 向导(它是 `/onboard` 的别名,`/setup`/`/wizard` 同义)
- `/help` → 检查中文翻译(注意 **`/copy` 的说明仍是英文**,已知)
- **`/new` → "unknown command"**(octos-tui **没有** `/new`;会话 fork 是服务端 octos-bus 的特性)
- 可达菜单(Mock/真实后端都广告):`/model` `/status` `/cost` `/login` `/provider` `/skills` `/mcp` `/tools`
- 受门控、Mock 下可能显示 Unavailable(去真实后端测):`/permissions` `/review` 及自治族 `/agents` `/goal` `/loop` `/task` `/threads` `/turn`

**全局快捷键:** `Ctrl+Q` 退出 · `Ctrl+C` 中断 turn · `Ctrl+U` 清空 composer · `Ctrl+O` 折叠/展开工具输出 · `Ctrl+Y` 复制上一条 · `Alt+A` 重开被隐藏的提问/审批弹窗 · `Alt+J`/`Alt+K` 上下移动。
**Composer readline:** `Ctrl+A`/`Home` 行首 · `Ctrl+E`/`End` 行尾 · `Alt+B`/`Alt+F` 词移 · `Ctrl+W` 删前一个词 · `Alt+D` 删后一个词 · `Ctrl+K` 删到行尾。

**启动/环境/配置矩阵:**
- `octos-tui --lang zh` / `OCTOS_LANG=zh octos-tui` / `LANG=zh_CN.UTF-8 octos-tui` → 中文
- `octos-tui --lang zh_CN` → **clap 报错**(预期)
- 在 json 里 `"lang":"zh"`,且 `--lang` 覆盖 config、config 覆盖 env
- 运行时 `/lang zh` 后重启 → 回到启动值(**不持久化,这是预期不是 bug**)
- 单测:`cargo test -p octos-tui i18n`

---

## 5. 风险 / 坑

1. **多实例抢同一 data-dir(最常见故障)**:每个 TUI 各 spawn 一个 `octos serve`;第二个对每个 profile 走严格 `EpisodeStore::open`,撞 `DatabaseAlreadyOpen` → 仅 warn 跳过该 profile → 该 profile `/api/chat` **503**(GH#899 后不再 crash,但 profile 不可用),表现为"连不上/无可用 profile"。**这就是不清掉 48699 的后果**。要并行测试 → 用**不同的 `--data-dir`**。
2. **信号杀 TUI = serve 孤儿**:`kill`/`kill -9`/关窗口都不跑析构,48699 继续锁 redb。**优雅退出 + `ps`/`pgrep` 验证**是兜底(见第 1 节)。
3. **相对路径陷阱**:旧进程 `./octos-data` 按 serve 子进程的 cwd(= 启动目录)解析,已实测 = 绝对 `…/octos-tui/octos-data`,与 launch json 的绝对路径**同一目录、同一把锁**。路径 A 用独立绝对 data-dir 正是为绕开它。
4. **session 竞态**:仅在**没有 session**(sessions 为空)时,capabilities 响应先到才可能短暂弹菜单;**本 launch json 带 `session`,不存在竞态**。另:两个 serve 同写同一 session JSONL 走 read-modify-rename,竞争会**丢更新但不损坏**文件。
5. **`get_or_create` 静默新建**:写错/轮换过的 session UUID 不会报错,而是**悄悄开一个空会话**;profile 绑定(alexz)仍需服务端能解析,未知 profile 是另一条失败路径。
6. **OSC52 复制依赖终端**:iTerm2/kitty/WezTerm/foot/新版 xterm 支持;tmux 需 `allow-passthrough`。终端不支持时 `/copy`/`Ctrl+Y` **会报成功但系统剪贴板不更新**——务必真的粘贴一次验证。
7. **HOME 级 redb**:`cost_ledger.redb` 在 `~/.octos` 而非 data-dir 作用域,理论上多 serve 可能在此撞锁;本机已被 529 与 48699 长时间并存证伪,换配置/换机器时留意。