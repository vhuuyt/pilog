# pilog

> pi 会话日志查看器 / 导出工具（TUI） — Rust 版
>
> 浏览、搜索、导出 [pi](https://github.com/earendil-works/pi) 编码助手的会话记录，单文件实现，零额外服务依赖。

```
┌─ 会话 ─────────────┬─ 日志 ────────────────────────────────────────────┐
│ ▶ 输入法lua脚本增加 │  ── 用户 ──────────────────────────────────────  │
│   绘图工具使用      │  帮我看下这段配置哪里不对                            │
│   blender           │  ── 助手 ──────────────────────────────────────  │
│   pilog打包         │  你好，我看了一下……                                │
└────────────────────┴──────────────────────────────────────────────────┘
```

## 特性

- **双栏 TUI**：左侧会话列表，右侧实时日志；`Tab` 切换焦点
- **对话模式**（`c`）：只看你和 AI 的对话，隐藏工具调用与原始输出，AI 回复按 Markdown 渲染
- **搜索 / 过滤**：`/` 过滤会话列表、`/` 搜索日志、`n` / `N` 在命中项间跳转
- **分级着色**：按「调用了哪个工具」给命令行背景上色（写操作红底），可用配置文件自定义
- **思考过程**：默认隐藏，`Ctrl+T` 随时开关
- **会话管理**：`Ctrl+R` 重命名会话（直接改 session 文件，不启动 pi）、`Del` 删除（带二次确认）、`n` 新建会话
- **导出**：`--list` / `--all` / `--latest` / `--max-lines` / `-o`，可输出纯文本或 Markdown
- **纯 Rust 单文件**：除 `ratatui` / `crossterm` / `serde_json` 等 crate 外无任何依赖，交叉编译友好（Linux / macOS / Windows）

## 安装

### 从源码构建

```bash
git clone https://github.com/vhuuyt/pilog.git
cd pilog
cargo build --release

# 安装到 ~/.local/bin
mkdir -p ~/.local/bin
cp target/release/pilog ~/.local/bin/

# 加入 PATH（zsh）
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
```

Windows（PowerShell）：

```powershell
cargo build --release
Copy-Item target\release\pilog.exe $env:USERPROFILE\bin\
```

### 直接运行

```bash
cargo run --release -- --list
```

## 使用

### 命令行

```
用法:
  pilog                   打开查看器（左列表 / 右日志，q 退出）
  pilog <会话.jsonl>      导出指定会话
  pilog --latest          导出最近一次会话
  pilog --list            列出所有会话
  pilog --all             导出所有会话（按时间分组）
  pilog --pick            打开查看器（显式）
  pilog --thinking        附上思考过程
  pilog --mouse           启用鼠标/触摸板滚轮（默认关，避免触摸板乱滚）
  pilog --max-lines N     每条输出最多显示 N 行
  pilog -o FILE           写文件（默认 stdout）
  pilog -h                显示帮助
```

示例：

```bash
pilog --list                          # 列出全部会话（时间 / 条数 / 名称 / 项目路径）
pilog --latest -o latest.md           # 导出最近一次会话到 Markdown
pilog --all --max-lines 200 -o all.md # 导出所有会话，每条最多 200 行
```

### 查看器按键

| 按键 | 会话列表 | 日志区 |
| --- | --- | --- |
| `Tab` | 切换焦点 | 切换焦点 |
| `↑` / `↓` | 选择会话 | 单行滚动 |
| `j` / `k` | 选择会话 | 半屏滚动 |
| `PgUp` / `PgDn` | 翻页 | 严格整页 |
| `gg` / `G` | 首个 / 末个会话 | 首行 / 末尾 |
| `/` | 过滤会话 | 搜索关键词 |
| `n` | 新建会话（用选中会话的目录） | 下一个命中 |
| `N` | — | 上一个命中 |
| `Esc` | 清过滤 / 退出 | 清搜索 / 退出 |
| `c` | 进入对话模式 | 同左 |
| `Ctrl+O` | 显示 / 隐藏工具输出 | 同左 |
| `Ctrl+T` | 显示 / 隐藏 AI 思考 | 同左 |
| `Ctrl+R` | 重命名选中会话 | 同左 |
| `Enter` | 进入选中会话 | 同左 |
| `Del` | 删除选中会话（需二次确认） | 同左 |
| `q` | 退出 | 退出 |

## 配置

配置文件位置：`~/.config/pilog/config`（首次运行会自动生成默认配置）。

```ini
# ── 1) 工具分级：按「调用了哪个工具」决定命令行背景 ──
# 语法: <danger|normal|plain> <工具名>
#   danger = 红底（危险 / 写入类）   normal/plain = 不着色
# 未列出的工具默认不着色。
danger write
danger edit
normal read
normal bash

# ── 2) bash 命令启发式 ──
# on 时，按下面的关键词把危险命令升级为红底（safe 命中优先）。
bash_heuristic on

# 危险关键词
bash_danger rm
bash_danger sudo
bash_danger mv
...

# 安全命令（首个词命中则一律不判危险）
bash_safe ls
bash_safe cat
bash_safe echo
...
```

完整示例见 [`config.example`](config.example)。

## 会话来源

| 来源 | 说明 |
| --- | --- |
| `~/.pi/agent/sessions/**.jsonl` | 默认扫描目录（pi 的会话存储） |
| `PI_CODING_AGENT_SESSION_DIR` | 覆盖默认扫描目录 |
| `PI_SESSION_FILE` | 指定单个会话文件（`--latest` 优先使用） |

会话文件为 JSONL，首行 `{"type":"session", ...}` 提供 `id` / `cwd` / `timestamp`，
之后每行一条 `message`；`session_info` 行可提供会话显示名。

## 开发

```bash
cargo build          # 调试构建
cargo test           # 单元测试（7 个）
cargo fmt            # 格式化
cargo clippy         # 静态检查
```

全部逻辑集中在 `src/main.rs`（约 2100 行），包含：

- 会话发现与解析（`load_session` / `load_all`）
- 日志渲染（`render_log`，含 Markdown 渲染与行内高亮）
- 工具分级与危险命令启发式（`Config` / `command_is_risky`）
- TUI 事件循环（`event_loop`）

## License

[MIT](LICENSE)
