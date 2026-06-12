# octos-tui 完整使用指南（moonshot kimi-k2.6）

> 基于对 octos / octos-tui 最新源码（HEAD `840ee2e`）的实测整理。
> octos-tui 是前端 TUI，后端是它自己 spawn 的 `octos serve --stdio` 子进程。
> **新版亮点**：多种一行安装（npm/brew/shell/cargo）、中文界面（`/lang`）、`/theme` `/thinking` `/copy` 等命令、inline 视口可原生选择/复制。

---

## 0. 先消除一个误解：退出重开弹 onboarding ≠ 配置丢了

- 你的 profile 完整保存在 `<data-dir>/profiles/alexz.json`：
  - `llm.primary` = **moonshot / kimi-k2.6**（主模型，正确）+ `env_vars.MOONSHOT_API_KEY`
- 截图里 **"Profile: alexz - Local profile ready"** 就是它已加载的证明。

**为什么每次还弹 onboarding？** 源码 `src/store.rs:4686` 的判断：启动时只要**没有打开的 session**（`sessions.is_empty()`）就自动弹 setup 菜单——TUI **不记忆上次 session**，所以每次冷启动都满足，和 profile 在不在无关。**弹的是 setup 菜单，不是要你重配**。（此逻辑新版未变。）

---

## 1. 安装

都装出一个自包含的 `octos-tui` 二进制。**后端 `octos`（主仓）也要在 PATH**（你已装在 `~/.cargo/bin/octos`）。

### 方式 A：预编译二进制（推荐，无需 Rust）
```bash
# npm
npm install -g @octos-org/octos-tui

# Homebrew
brew install octos-org/tap/octos-tui

# shell 一行安装（macOS / Linux）
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/octos-org/octos-tui/releases/latest/download/octos-tui-installer.sh | sh
```

### 方式 B：用 Cargo 从源码装（需 Rust 1.85+）
```bash
cargo install --git https://github.com/octos-org/octos-tui octos-tui   # 直接从 git
# 或发布到 crates.io 后：
cargo install octos-tui
```

### 方式 C：本地开发 build（你当前用的）
```bash
cd /Users/zhangalex/Work/Projects/FW/octos-tui
cargo build --release      # 产出 ./target/release/octos-tui
```

> 下文命令以全局 `octos-tui` 为准；若用方式 C，把 `octos-tui` 换成 `./target/release/octos-tui`。

---

## 2. 中文界面 / 语言切换（新）

- 启动时默认语言取自 `OCTOS_LANG` / `LANG` 环境变量，或 `--lang zh|en` 参数。
- 运行时随时切换：
  - `/lang` —— 打开语言选择菜单
  - `/lang zh` —— 直接切中文；`/lang en` —— 切英文（即时重绘，确认消息用新语言显示）
- 想默认就是中文：在 config 文件加 `"lang": "zh"`，或 shell 里 `export OCTOS_LANG=zh`。

---

## 3. 首次配置（你已完成，留作参考）

全新空 data-dir、**不传** `--profile-id`：
```bash
octos-tui --mode protocol \
  --stdio-command "octos serve --stdio --solo --data-dir ./octos-data"
```
进入 "Welcome to Octos" 后（新版光标默认落在 **Model family** 行，只读状态行移到了右侧信息栏）：

1. 填 **Full name / Username / Email**（email 仅本地元数据，不发 OTP）→ **创建本地 profile**（`profile/local/create`）
2. **加载 provider catalog** → **Model family: moonshot** → **Model: kimi-k2.6** → **Provider route: Official API (moonshot)**
3. **API key**：选中该行输入（或 `/onboard key <secret>`，脱敏）— 也可改用环境变量（见 §6）
4. （可选）**Test provider** 验证连通
5. **Save provider to profile**（`profile/llm/upsert`，写入 `alexz.json` 的 `primary`）
6. **Validate workspace** → **Open coding session** → Composer 输入、Enter 发送

> 随时 `/setup` 重开此向导。

---

## 4. ✅ 推荐：用启动配置文件，重启免重配

已为你生成 `~/octos-tui-launch.json`：
```json
{
  "mode": "protocol",
  "stdio-command": "octos serve --stdio --solo --data-dir /Users/zhangalex/Work/Projects/FW/octos-tui/octos-data",
  "profile-id": "alexz",
  "session": "fd1a6150-6304-416b-bf3e-572367dbc85d"
}
```
要点：
- **绝对 data-dir** —— 消除 `./octos-data` 相对路径隐患（§7），任意目录启动都指向同一份 profile。
- **`profile-id: alexz`** —— 启动即加载该 profile 的 LLM 配置。
- **`session: <固定UUID>`** —— 启动时 `OpenSession`（`src/transport.rs`），使 `sessions` 非空，**尽量跳过 onboarding 直接进会话**，对话历史也累积在同一会话。
- （可选）加 `"lang": "zh"` 默认中文。

启动：
```bash
octos-tui --config ~/octos-tui-launch.json
```
建议 shell 别名（写进 `~/.zshrc`）：
```bash
alias octt='octos-tui --config ~/octos-tui-launch.json'
```
> 确保启动 `octt` 的 shell 里有 `MOONSHOT_API_KEY`（或密钥已存进 profile，你已存）。

---

## 5. 如果启动后仍弹出 setup 菜单

profile 已就绪，**两步进会话**（方向键移 `>`，Enter）：
1. `Validate workspace` → Enter（若 not validated）
2. `Open coding session` → Enter

进会话后 Composer 打字、Enter 发送。**不用重输 family / model / API key**。

> **新增（onboarding 逃生门）**：如果误入"创建档案"表单（多半是忘了传 `--profile-id`），表单里现在有两行可用：
> - **使用已有档案（输入 ID）** —— 回车编辑，输入已有 profile id（如 `alex`）后向导直接跳到 provider 设置，不再要求新建；等价命令 `/onboard profile alex`
> - **退出 octos-tui** —— 不创建任何东西直接退出（Esc 在此界面被刻意屏蔽以防误触，Ctrl+Q 也始终可退）

---

## 6. 密钥（MOONSHOT_API_KEY）三种来源与优先级

serve 取密钥顺序（`octos config.rs get_api_key`）：

| 优先级 | 来源 | 怎么设 | 是否落盘 |
|---|---|---|---|
| ① | auth store | `octos auth login -p moonshot` | 系统 keychain（最安全） |
| ② | **profile 的 env_vars** | TUI 里 `/onboard key <secret>`（你用的就是这个） | 明文写进 `alexz.json` |
| ③ | 进程环境变量 | shell `export MOONSHOT_API_KEY=...` | 不落盘 |

你已走 ②，新 shell 没 export 也能用。

---

## 7. ⚠️ 隐患：相对 data-dir

`--data-dir ./octos-data` 相对**启动时的当前目录**解析（透传给 serve 子进程）。从别的目录启动同样命令 → 指向别处空目录 → 找不到 alexz → 这才会真的“从头配”。§4 的 config 已用**绝对路径**根除此隐患。

---

## 7.5 滚动与输入框钉底（scroll-mode，新）

| 模式 | 行为 | 取舍 |
|---|---|---|
| `native`（默认） | 滚轮走终端原生 scrollback，**输入框会随屏滚走**；按 **Ctrl+T 或 PageUp** 进入全屏回看（pager），此时输入框钉底、PgUp/PgDn/滚轮/方向键滚动内容，Esc 退出 | 保留终端原生鼠标选择/复制 |
| `pinned` | 鼠标滚轮被 App 接管：**上滚自动进入 pager（输入框始终钉在底部），滚回底部自动退回实时视图**——体感即"无论怎么滚输入框都不动" | 原生鼠标选择文本需改用 **Shift+拖选** |

设置方式（CLI 优先于 config 文件）：

```bash
octos-tui --scroll-mode pinned ...      # 命令行
# 或 config 文件里：
{ "scroll-mode": "pinned" }
```

pager 内输入不受影响：照常打字、Enter 发送。

## 8. 常用命令 / 快捷键

| 想做 | 操作 |
|---|---|
| 日常启动 | `octt`（别名）或 `octos-tui --config ~/octos-tui-launch.json` |
| 切换语言 | `/lang`（菜单）或 `/lang zh` / `/lang en` |
| 重开 setup 向导 | `/setup` |
| setup 里直接进会话 | `Validate workspace` → `Open coding session` |
| 换模型 | `/setup` → 重选 family/model → Save provider |
| 切主题 | `/theme` |
| 设思考强度 | `/thinking <low\|medium\|high\|max\|default>` |
| 复制上一条回复 | `/copy` 或 `Ctrl+Y`（OSC52，新版支持终端原生选择/复制） |
| 发消息 | Composer 输入 → Enter |
| 检查器 / 展开 | Tab / Ctrl+O |
| 查看/改 profile | 编辑 `<data-dir>/profiles/alexz.json` |

---

## 9. profile JSON 结构（alexz.json 摘要）

```json
{
  "id": "alexz", "name": "AlexZhang", "username": "alexz",
  "config": {
    "env_vars": { "MOONSHOT_API_KEY": "sk-..." },
    "llm": {
      "primary":   { "family_id": "moonshot", "model_id": "kimi-k2.6",
                     "route": { "route_id": "moonshot",
                                "base_url": "https://api.moonshot.ai/v1",
                                "api_key_env": "MOONSHOT_API_KEY" } },
      "fallbacks": [ { ...同上... } ]
    }
  }
}
```
`octos serve` 启动时扫描所有 profile，`primary` 含 `family_id`/`model_id` 即视为“有 LLM 选择”，自动装配该 profile 的运行时——这就是 TUI 能直接用 moonshot 的依据。

---

## 10. 后端 octos 的配置目录（新版变化，供参考）

新版 `octos`（#1435/#1439）把 **config/auth** 默认目录从 `~/.octos` 改到 **`~/.config/octos`**（macOS 也是，遵循 XDG/CLI 惯例），非破坏性自动迁移（老文件复制不删 + fallback）。**不影响 octos-tui**：TUI 用显式 `--data-dir`，profile 在 data_dir 下（解析规则未变），密钥在 profile env_vars 里。
