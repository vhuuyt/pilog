# pilog

> A TUI viewer / exporter for [pi](https://github.com/earendil-works/pi) coding-agent sessions.
>
> Single-file Rust implementation — no server, no database, just your session logs.

```
┌─ Sessions ──────────┬─ Log ─────────────────────────────────────────────┐
│ ▶ lua script tweaks  │  ── 用户 ──────────────────────────────────────  │
│   drawing tool       │  help me check this config                       │
│   blender            │  ── 助手 ──────────────────────────────────────  │
│   pilog packaging    │  sure, I took a look …                           │
└──────────────────────┴──────────────────────────────────────────────────┘
```

[中文文档](README.md)

## Features

- **Two-pane TUI**: session list on the left, live log on the right; `Tab` switches focus
- **Conversation mode** (`c`): shows only you and the AI, hides tool calls / raw output, renders AI replies as Markdown
- **Search & filter**: `/` filters the session list, `/` searches the log, `n` / `N` jump between hits
- **Risk-aware colouring**: command lines are colour-coded by the tool that produced them (writes get a red background), fully configurable
- **Thinking toggle**: hidden by default, `Ctrl+T` shows it
- **Session management**: `Ctrl+R` renames a session (edits the session file directly, no pi needed), `Del` deletes it (with confirmation), `n` starts a new one
- **Export**: `--list` / `--all` / `--latest` / `--max-lines` / `-o` to plain text or Markdown
- **Pure Rust**: only `ratatui` / `crossterm` / `serde_json` / `unicode-width` / `tui-markdown`; builds on Linux, macOS and Windows

## Install

```bash
git clone https://github.com/vhuuyt/pilog.git
cd pilog
cargo build --release

mkdir -p ~/.local/bin
cp target/release/pilog ~/.local/bin/
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc && source ~/.zshrc
```

Windows (PowerShell):

```powershell
cargo build --release
Copy-Item target\release\pilog.exe $env:USERPROFILE\bin\
```

## Usage

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

### Keybindings

| Key | Session list | Log pane |
| --- | --- | --- |
| `Tab` | switch focus | switch focus |
| `↑` / `↓` | select session | scroll one line |
| `j` / `k` | select session | scroll half a page |
| `PgUp` / `PgDn` | page up / down | page up / down |
| `gg` / `G` | first / last session | top / bottom |
| `/` | filter sessions | search |
| `n` | new session | next match |
| `N` | — | previous match |
| `Esc` | clear filter / quit | clear search / quit |
| `c` | conversation mode | same |
| `Ctrl+O` | toggle tool output | same |
| `Ctrl+T` | toggle AI thinking | same |
| `Ctrl+R` | rename session | same |
| `Enter` | open session | same |
| `Del` | delete session (confirm) | same |
| `q` | quit | quit |

## Configuration

Config file: `~/.config/pilog/config` (a default one is generated on first run).

```ini
# 1) tool levels — decide the background colour of a command line
danger write      # red background (writes)
danger edit
normal read
normal bash

# 2) bash heuristic — upgrade dangerous commands to red
bash_heuristic on
bash_danger rm
bash_danger sudo
...
bash_safe ls
bash_safe cat
...
```

See [`config.example`](config.example) for the full file.

## Where sessions come from

| Source | Description |
| --- | --- |
| `~/.pi/agent/sessions/**.jsonl` | default scan directory (pi's session store) |
| `PI_CODING_AGENT_SESSION_DIR` | override the scan directory |
| `PI_SESSION_FILE` | a single session file (used first by `--latest`) |

Session files are JSONL: the first line is `{"type":"session", …}` with `id` / `cwd` / `timestamp`,
followed by one `message` per line; a `session_info` line may carry the display name.

## Development

```bash
cargo build     # debug build
cargo test      # unit tests (7)
cargo fmt
cargo clippy
```

Everything lives in `src/main.rs` (~2100 lines): session discovery and parsing, log rendering
(with Markdown support and inline highlighting), tool-level / dangerous-command classification,
and the TUI event loop.

## License

[MIT](LICENSE)
