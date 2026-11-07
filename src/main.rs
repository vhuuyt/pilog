// pilog (Rust) - pi 会话日志查看器 / 导出工具
// 单文件、无外部依赖（ratatui + crossterm + serde_json）。
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ---------------------------------------------------------------------------
// 会话元数据
// ---------------------------------------------------------------------------
#[derive(Clone)]
struct Session {
    path: PathBuf,
    id: String,
    cwd: String,
    name: Option<String>,
    first_message: String,
    start: String,
    created: i64,
    modified: i64,
    count: usize,
}

impl Session {
    /// pi 的显示名规则：name ?? firstMessage
    fn title(&self) -> &str {
        match self.name.as_deref() {
            Some(n) if !n.is_empty() => n,
            _ => &self.first_message,
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn sess_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return PathBuf::from(d);
    }
    home().join(".pi/agent/sessions")
}

fn all_session_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(sess_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if let Ok(rd2) = fs::read_dir(&p) {
                    for e2 in rd2.flatten() {
                        let f = e2.path();
                        if f.extension().map(|x| x == "jsonl").unwrap_or(false) {
                            out.push(f);
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out
}

fn find_latest() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("PI_SESSION_FILE") {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Some(p);
        }
    }
    all_session_paths()
        .into_iter()
        .max_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn iso_to_ms(s: &str) -> i64 {
    if s.len() < 19 {
        return 0;
    }
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0);
    let (y, mo, d) = (num(0, 4), num(5, 7), num(8, 10));
    let (h, mi, se) = (num(11, 13), num(14, 16), num(17, 19));
    let ms = if s.len() >= 23 && s.as_bytes().get(19) == Some(&b'.') { num(20, 23) } else { 0 };
    (days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se) * 1000 + ms
}

fn activity_ms(entry: &Value, msg: &Value) -> i64 {
    if let Some(n) = msg.get("timestamp") {
        if let Some(i) = n.as_i64() {
            return i;
        }
        if let Some(f) = n.as_f64() {
            return f as i64;
        }
    }
    entry.get("timestamp").and_then(|v| v.as_str()).map(iso_to_ms).unwrap_or(0)
}

fn extract_text(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

fn load_session(path: &Path) -> Option<Session> {
    let content = fs::read_to_string(path).ok()?;
    let mut header = false;
    let (mut id, mut cwd, mut start) = (String::new(), String::new(), String::new());
    let mut created = 0i64;
    let mut name: Option<String> = None;
    let mut first = String::new();
    let mut count = 0usize;
    let mut last = 0i64;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !header {
            if ty != "session" {
                return None;
            }
            header = true;
            id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
            cwd = v.get("cwd").and_then(|x| x.as_str()).unwrap_or("").to_string();
            start = v.get("timestamp").and_then(|x| x.as_str()).unwrap_or("").to_string();
            created = iso_to_ms(&start);
            continue;
        }
        match ty {
            "session_info" => {
                if let Some(n) = v.get("name").and_then(|x| x.as_str()) {
                    let t = n.trim();
                    name = if t.is_empty() { None } else { Some(t.to_string()) };
                }
            }
            "message" => {
                count += 1;
                if let Some(msg) = v.get("message") {
                    let at = activity_ms(&v, msg);
                    if at > last {
                        last = at;
                    }
                    if first.is_empty()
                        && msg.get("role").and_then(|r| r.as_str()) == Some("user")
                    {
                        let t = extract_text(msg);
                        if !t.is_empty() {
                            first = t;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if !header {
        return None;
    }
    let mtime = fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let modified = if last > 0 {
        last
    } else if created > 0 {
        created
    } else {
        mtime
    };
    Some(Session {
        path: path.to_path_buf(),
        id,
        cwd,
        name,
        first_message: if first.is_empty() { "(no messages)".into() } else { first },
        start,
        created,
        modified,
        count,
    })
}

fn load_all() -> Vec<Session> {
    all_session_paths().iter().filter_map(|p| load_session(p)).collect()
}

// ---------------------------------------------------------------------------
// 日志渲染（与 Python 版格式一致）
// ---------------------------------------------------------------------------
const H_USER: &str = "── 用户 ────────────────────────────────────────";
const H_ASSIST: &str = "── 助手 ────────────────────────────────────────";
const H_THINK: &str = "── 思考 ────────────────────────────────────────";

fn clip(s: &str, max_lines: u32) -> String {
    let s = s.trim_end_matches('\n');
    if max_lines == 0 {
        return s.to_string();
    }
    let lines: Vec<&str> = s.split('\n').collect();
    if lines.len() <= max_lines as usize {
        return s.to_string();
    }
    let kept = lines[..max_lines as usize].join("\n");
    format!("{}\n…（省略 {} 行）", kept, lines.len() - max_lines as usize)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    User,
    Assist,
    Think,
    Cmd,
    CmdRisky,
    CmdPlain,
    Out,
    OutErr,
}

/// 一行日志 = 若干「样式 + 文本」片段（支持行内 markdown 多样式）。
type SLine = Vec<(Style, String)>;

fn sline<S: Into<String>>(text: S, st: Style) -> SLine {
    vec![(st, text.into())]
}

fn sline_text(line: &SLine) -> String {
    line.iter().map(|(_, s)| s.as_str()).collect()
}

/// 是否为「危险/写」类命令（启发式，pi 数据里没有该字段）。
fn has_word(c: &str, w: &str) -> bool {
    let mut start = 0usize;
    while let Some(pos) = c[start..].find(w) {
        let p = start + pos;
        let before_ok = p == 0 || !c.as_bytes()[p - 1].is_ascii_alphanumeric();
        let a = p + w.len();
        let after_ok = a >= c.len() || !c.as_bytes()[a].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = p + 1;
    }
    false
}

/// 结构性危险模式（不适合用关键词表达的）。
fn builtin_cmd_risky(c: &str) -> bool {
    if has_word(c, "sed") && c.contains("-i") {
        return true;
    }
    if has_word(c, "git")
        && ["reset", "clean", "restore", "checkout", "push", "rebase"]
            .iter()
            .any(|w| has_word(c, w))
    {
        return true;
    }
    for pm in ["dnf", "yum", "apt", "pacman", "zypper", "brew", "pip", "pip3", "cargo"] {
        if has_word(c, pm)
            && ["install", "remove", "erase", "upgrade", "dist-upgrade", "uninstall"]
                .iter()
                .any(|w| has_word(c, w))
        {
            return true;
        }
    }
    if c.contains("| sh")
        || c.contains("| bash")
        || c.contains("|sh")
        || c.contains("|bash")
        || c.contains(" -delete")
    {
        return true;
    }
    false
}

/// 去掉 shell 注释（行内 `#` 到行尾），避免注释里的字眼造成误判。
fn strip_comments(cmd: &str) -> String {
    cmd.lines()
        .map(|line| {
            let b = line.as_bytes();
            let mut cut = line.len();
            for (i, &ch) in b.iter().enumerate() {
                if ch == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
                    cut = i;
                    break;
                }
            }
            &line[..cut]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 综合配置判断 bash 命令是否危险。
fn command_is_risky(cmd: &str, cfg: &Config) -> bool {
    if !cfg.bash_heuristic {
        return false;
    }
    let c = strip_comments(cmd).to_lowercase();
    if c.trim().is_empty() {
        return false;
    }
    let first = c
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-')
        .to_string();
    if !first.is_empty() && cfg.safe_cmds.iter().any(|w| *w == first) {
        return false;
    }
    if cfg.danger_cmds.iter().any(|w| has_word(&c, w)) {
        return true;
    }
    builtin_cmd_risky(&c)
}

fn log_to_text(lines: &[SLine]) -> String {
    lines.iter().map(|l| sline_text(l)).collect::<Vec<_>>().join("\n")
}

// ---------------------------------------------------------------------------
// 工具分级配置（~/.config/pilog/config）
// ---------------------------------------------------------------------------
struct Config {
    levels: HashMap<String, Kind>,
    bash_heuristic: bool,
    danger_cmds: Vec<String>,
    safe_cmds: Vec<String>,
}

fn config_path() -> PathBuf {
    home().join(".config/pilog/config")
}

const DEFAULT_CONFIG: &str = "\
# pilog 工具分级配置 (~/.config/pilog/config)
#
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

# 危险关键词（按你历史会话实际用到的命令整理，可自行增删）
bash_danger sudo
bash_danger rm
bash_danger pkill
bash_danger kill
bash_danger killall
bash_danger chmod
bash_danger chown
bash_danger chgrp
bash_danger systemctl
bash_danger mv
bash_danger dd
bash_danger mkfs
bash_danger mount
bash_danger umount
bash_danger crontab
bash_danger passwd
bash_danger useradd
bash_danger userdel
bash_danger shutdown
bash_danger reboot
bash_danger shred
bash_danger truncate
bash_danger tee

# 安全命令（首个词命中则一律不判危险）
bash_safe ls
bash_safe cat
bash_safe echo
bash_safe cd
bash_safe grep
bash_safe ps
bash_safe pgrep
bash_safe which
bash_safe file
bash_safe env
bash_safe export
bash_safe head
bash_safe tail
bash_safe wc
bash_safe sort
bash_safe uniq
bash_safe rpm
bash_safe rpmspec
bash_safe gsettings
bash_safe ibus
";

fn load_config() -> Config {
    let p = config_path();
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => {
            if let Some(dir) = p.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(&p, DEFAULT_CONFIG);
            DEFAULT_CONFIG.to_string()
        }
    };
    let mut cfg = Config {
        levels: HashMap::new(),
        bash_heuristic: true,
        danger_cmds: Vec::new(),
        safe_cmds: Vec::new(),
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("").to_lowercase();
        let val = it.next().unwrap_or("");
        match key.as_str() {
            "danger" => {
                cfg.levels.insert(val.to_lowercase(), Kind::CmdRisky);
            }
            "normal" => {
                cfg.levels.insert(val.to_lowercase(), Kind::Cmd);
            }
            "plain" => {
                cfg.levels.insert(val.to_lowercase(), Kind::CmdPlain);
            }
            "bash_heuristic" => {
                cfg.bash_heuristic =
                    matches!(val.to_lowercase().as_str(), "on" | "true" | "1" | "yes");
            }
            "bash_danger" => {
                if !val.is_empty() {
                    cfg.danger_cmds.push(val.to_lowercase());
                }
            }
            "bash_safe" => {
                if !val.is_empty() {
                    cfg.safe_cmds.push(val.to_lowercase());
                }
            }
            _ => {}
        }
    }
    cfg
}

impl Config {
    /// 按工具名决定命令行的背景等级。
    fn classify(&self, tool: &str, cmd: &str) -> Kind {
        let t = tool.to_lowercase();
        if t == "bash" && command_is_risky(cmd, self) {
            return Kind::CmdRisky;
        }
        self.levels.get(&t).copied().unwrap_or(Kind::Cmd)
    }
}

fn render_log(
    path: &Path,
    thinking: bool,
    show_output: bool,
    conversation: bool,
    markdown: bool,
    max_lines: u32,
    cfg: &Config,
) -> Vec<SLine> {
    let mut out: Vec<SLine> = Vec::new();
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![sline(format!("（无法读取: {}）", path.display()), Style::default())],
    };
    let mut pending: HashMap<String, (String, Value)> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let msg = match v.get("message") {
            Some(m) => m,
            None => continue,
        };
        match msg.get("role").and_then(|r| r.as_str()).unwrap_or("") {
            "user" => {
                let t = extract_text(msg);
                if !t.trim().is_empty() {
                    out.push(Vec::new());
                    out.push(sline(H_USER, style_of(Kind::User)));
                    out.push(sline(t, Style::default()));
                }
            }
            "assistant" => {
                let mut txt: Vec<String> = Vec::new();
                let mut thinks: Vec<String> = Vec::new();
                if let Some(Value::Array(blocks)) = msg.get("content") {
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()) {
                            Some("text") => {
                                if let Some(t) = b.get("text").and_then(|x| x.as_str()) {
                                    txt.push(t.to_string());
                                }
                            }
                            Some("thinking") if thinking && !conversation => {
                                if let Some(t) = b.get("thinking").and_then(|x| x.as_str()) {
                                    thinks.push(t.to_string());
                                }
                            }
                            Some("toolCall") => {
                                let id = b.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let name =
                                    b.get("name").and_then(|x| x.as_str()).unwrap_or("?").to_string();
                                let args = b.get("arguments").cloned().unwrap_or(Value::Null);
                                pending.insert(id, (name, args));
                            }
                            _ => {}
                        }
                    }
                }
                let joined = txt.join("\n");
                if !joined.trim().is_empty() {
                    out.push(Vec::new());
                    out.push(sline(H_ASSIST, style_of(Kind::Assist)));
                    if markdown {
                        out.extend(markdown_lines(&joined, Style::default()));
                    } else {
                        out.push(sline(joined, Style::default()));
                    }
                }
                for t in thinks {
                    out.push(Vec::new());
                    out.push(sline(H_THINK, style_of(Kind::Think)));
                    out.push(sline(t, Style::default()));
                }
            }
            "toolResult" => {
                if conversation {
                    continue;
                }
                let cid = msg.get("toolCallId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let is_err = msg.get("isError").and_then(|x| x.as_bool()).unwrap_or(false);
                let (name, args) = match pending.remove(&cid) {
                    Some((n, a)) => (n, a),
                    None => (
                        msg.get("toolName").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
                        Value::Null,
                    ),
                };
                let cmd = args.get("command").and_then(|x| x.as_str()).map(|s| s.to_string());
                let ck = cfg.classify(&name, cmd.as_deref().unwrap_or(""));
                let cst = style_of(ck);
                out.push(Vec::new());
                out.push(sline(format!("▶ {}", name), cst));
                if let Some(cmd) = &cmd {
                    out.push(sline(format!("$ {}", cmd), cst));
                } else if !args.is_null() {
                    if let Ok(s) = serde_json::to_string_pretty(&args) {
                        out.push(sline(s, cst));
                    }
                } else {
                    out.push(sline("(无参数)", cst));
                }
                if show_output {
                    let ok = if is_err { Kind::OutErr } else { Kind::Out };
                    out.push(sline(
                        format!("  ↳ 输出{}", if is_err { " [错误]" } else { "" }),
                        style_of(ok),
                    ));
                    out.push(sline(clip(&extract_text(msg), max_lines), Style::default()));
                }
            }
            _ => {}
        }
    }
    if !conversation {
        for (_, (name, args)) in pending {
            let cmd = args.get("command").and_then(|x| x.as_str()).map(|s| s.to_string());
            let ck = cfg.classify(&name, cmd.as_deref().unwrap_or(""));
            let cst = style_of(ck);
            out.push(Vec::new());
            out.push(sline(format!("▶ {}", name), cst));
            if let Some(c) = cmd {
                out.push(sline(format!("$ {}", c), cst));
            }
        }
    }
    out.push(Vec::new());
    out
}

fn style_of(k: Kind) -> Style {
    match k {
        Kind::User => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Kind::Assist => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        Kind::Think => Style::default().fg(Color::Magenta),
        Kind::Cmd => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        Kind::CmdRisky => {
            Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD)
        }
        Kind::CmdPlain => Style::default(),
        Kind::Out => Style::default().fg(Color::Blue),
        Kind::OutErr => Style::default().fg(Color::Red),
    }
}

// ---------------------------------------------------------------------------
// 轻量 markdown 渲染（用于助手回复）
// ---------------------------------------------------------------------------
/// 用 tui-markdown 库把 markdown 渲染成带样式的行。
fn markdown_lines(text: &str, _base: Style) -> Vec<SLine> {
    let t = tui_markdown::from_str(text);
    let mut out: Vec<SLine> = Vec::new();
    for line in t.lines {
        let mut spans: SLine = Vec::new();
        for sp in line.spans {
            spans.push((sp.style, sp.content.to_string()));
        }
        out.push(spans);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// 清理控制字符：tab→空格，其他控制字符丢弃（否则可能让终端跳到列首）。
fn clean_ch(ch: char) -> Option<char> {
    if ch == '\t' {
        return Some(' ');
    }
    if (ch as u32) < 0x20 || (ch as u32) == 0x7f {
        return None;
    }
    Some(ch)
}

/// 按显示宽度换行，保留每个片段的样式。
#[allow(unused_assignments)]
fn wrap_styled(lines: &[SLine], width: usize) -> Vec<SLine> {
    let width = width.max(1);
    let mut out: Vec<SLine> = Vec::new();
    for line in lines {
        if line.is_empty() {
            out.push(Vec::new());
            continue;
        }
        let mut cur: SLine = Vec::new();
        let mut w = 0usize;
        let mut started = false;
        for (st, text) in line {
            for (pi, phys) in text.split('\n').enumerate() {
                if pi > 0 {
                    out.push(std::mem::take(&mut cur));
                    w = 0;
                    started = false;
                }
                for ch in phys.chars() {
                    let Some(ch) = clean_ch(ch) else { continue };
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if w + cw > width && started {
                        out.push(std::mem::take(&mut cur));
                        w = 0;
                        started = false;
                    }
                    if let Some(last) = cur.last_mut() {
                        if last.0 == *st {
                            last.1.push(ch);
                        } else {
                            cur.push((*st, ch.to_string()));
                        }
                    } else {
                        cur.push((*st, ch.to_string()));
                    }
                    w += cw;
                    started = true;
                }
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
    }
    out
}

/// 把一个片段里匹配关键词的部分高亮后追加到 `out`。
fn push_highlight(base: Style, s: &str, q: &str, hl: Style, out: &mut Vec<Span<'static>>) {
    if s.is_empty() {
        return;
    }
    if q.is_empty() {
        out.push(Span::styled(s.to_string(), base));
        return;
    }
    let ls = s.to_lowercase();
    let lq = q.to_lowercase();
    let mut start = 0usize;
    while let Some(pos) = ls[start..].find(&lq) {
        let p = start + pos;
        let e = p + lq.len();
        if !s.is_char_boundary(p) || !s.is_char_boundary(e) {
            out.push(Span::styled(s.to_string(), base));
            return;
        }
        if p > start {
            out.push(Span::styled(s[start..p].to_string(), base));
        }
        out.push(Span::styled(s[p..e].to_string(), base.patch(hl)));
        start = e;
    }
    if start < s.len() {
        out.push(Span::styled(s[start..].to_string(), base));
    }
}

/// 把一个 SLine 转成 ratatui 的 Line（可带搜索高亮）。
fn sline_to_line(line: &SLine, q: &str, hl: Style, is_match: bool) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (st, text) in line {
        if is_match && !q.is_empty() {
            push_highlight(*st, text, q, hl, &mut spans);
        } else if !text.is_empty() {
            spans.push(Span::styled(text.clone(), *st));
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(""));
    }
    Line::from(spans)
}

fn trunc_w(s: &str, width: usize) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c if (c as u32) < 0x20 || (c as u32) == 0x7f => ' ',
            c => c,
        })
        .collect();
    let s = cleaned.as_str();
    if UnicodeWidthStr::width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    let budget = width.saturating_sub(1);
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > budget {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// TUI
// ---------------------------------------------------------------------------
struct App {
    sessions: Vec<Session>,
    filtered: Vec<usize>,
    sel: usize,
    focus_log: bool,
    log_scroll: usize,
    filter: String,
    filter_input: bool,
    confirm_delete: Option<PathBuf>,
    notice: Option<String>,
    cache: HashMap<PathBuf, Vec<SLine>>,
    wrapped: Vec<SLine>,
    wrap_key: Option<(PathBuf, usize)>,
    show_thinking: bool,
    show_output: bool,
    conversation: bool,
    search: String,
    search_input: bool,
    matches: Vec<usize>,
    match_idx: usize,
    pending_g: bool,
    new_name_input: bool,
    new_name: String,
    new_cwd: PathBuf,
    rename_input: bool,
    rename_name: String,
    config: Config,
}

impl App {
    fn new(mut sessions: Vec<Session>) -> Self {
        sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
        let filtered = (0..sessions.len()).collect();
        App {
            sessions,
            filtered,
            sel: 0,
            focus_log: false,
            log_scroll: 0,
            filter: String::new(),
            filter_input: false,
            confirm_delete: None,
            notice: None,
            cache: HashMap::new(),
            wrapped: Vec::new(),
            wrap_key: None,
            show_thinking: false,
            show_output: true,
            conversation: false,
            search: String::new(),
            search_input: false,
            matches: Vec::new(),
            match_idx: 0,
            pending_g: false,
            new_name_input: false,
            new_name: String::new(),
            new_cwd: PathBuf::new(),
            rename_input: false,
            rename_name: String::new(),
            config: load_config(),
        }
    }

    fn reindex(&mut self) {
        let f = self.filter.to_lowercase();
        self.filtered = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                f.is_empty()
                    || s.cwd.to_lowercase().contains(&f)
                    || s.name.as_deref().unwrap_or("").to_lowercase().contains(&f)
                    || s.first_message.to_lowercase().contains(&f)
                    || s.start.to_lowercase().contains(&f)
                    || s.id.to_lowercase().contains(&f)
            })
            .map(|(i, _)| i)
            .collect();
        if self.sel >= self.filtered.len() {
            self.sel = self.filtered.len().saturating_sub(1);
        }
        self.log_scroll = 0;
    }

    fn current(&self) -> Option<&Session> {
        self.filtered.get(self.sel).map(|&i| &self.sessions[i])
    }

    /// 重命名选中会话：往 JSONL 追加一条 session_info，不启动 pi。
    fn rename_current(&mut self, newname: &str) {
        let Some(path) = self.current().map(|s| s.path.clone()) else { return };
        match rename_session(&path, newname) {
            Ok(()) => {
                reload_sessions(self);
                self.notice = Some(format!("已重命名为「{}」", newname));
            }
            Err(e) => self.notice = Some(format!("重命名失败: {}", e)),
        }
    }

    fn request_delete(&mut self) {
        let Some(s) = self.current() else { return };
        let path = s.path.clone();
        let active = std::env::var_os("PI_SESSION_FILE").map(PathBuf::from);
        if active.as_deref() == Some(path.as_path()) {
            self.notice = Some("不能删除当前正在使用的会话".into());
        } else {
            self.confirm_delete = Some(path);
        }
    }

    fn delete_current(&mut self) {
        let Some(path) = self.confirm_delete.clone() else { return };
        match delete_session_file(&path) {
            Ok(DeleteMethod::Trash) => {
                self.sessions.retain(|s| s.path != path);
                self.cache.remove(&path);
                self.notice = Some("会话已移入回收站".into());
            }
            Ok(DeleteMethod::Unlink) => {
                self.sessions.retain(|s| s.path != path);
                self.cache.remove(&path);
                self.notice = Some("会话已删除".into());
            }
            Err(e) => self.notice = Some(format!("删除失败: {}", e)),
        }
        self.confirm_delete = None;
        self.focus_log = false;
        self.wrap_key = None;
        self.reindex();
    }

    /// 确保当前会话在当前宽度下已换行缓存（仅在选择/宽度变化时重算）。
    fn ensure_wrap(&mut self, width: usize) {
        let Some(path) = self.current().map(|s| s.path.clone()) else {
            self.wrapped.clear();
            self.wrap_key = None;
            return;
        };
        if self.wrap_key.as_ref() == Some(&(path.clone(), width)) {
            return;
        }
        if !self.cache.contains_key(&path) {
            let v = render_log(
                &path,
                self.show_thinking,
                self.show_output,
                self.conversation,
                true,
                0,
                &self.config,
            );
            self.cache.insert(path.clone(), v);
        }
        self.wrapped = self
            .cache
            .get(&path)
            .map(|raw| wrap_styled(raw, width))
            .unwrap_or_default();
        self.wrap_key = Some((path, width));
    }

    /// 根据当前搜索词重新计算命中行，并跳到第一个命中。
    fn recompute_matches(&mut self) {
        self.matches.clear();
        self.match_idx = 0;
        if self.search.is_empty() {
            return;
        }
        let q = self.search.to_lowercase();
        for (i, line) in self.wrapped.iter().enumerate() {
            if sline_text(line).to_lowercase().contains(&q) {
                self.matches.push(i);
            }
        }
        if let Some(&first) = self.matches.first() {
            self.log_scroll = first.saturating_sub(2);
        }
    }

    /// 跳到下一个 / 上一个匹配。
    fn jump_match(&mut self, dir: isize) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        if dir >= 0 {
            self.match_idx = (self.match_idx + 1) % n;
        } else {
            self.match_idx = (self.match_idx + n - 1) % n;
        }
        self.log_scroll = self.matches[self.match_idx].saturating_sub(2);
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 4 || area.width < 24 {
        let p = Paragraph::new("终端太小").style(Style::default().fg(Color::Red));
        f.render_widget(p, area);
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let (header, body, status) = (rows[0], rows[1], rows[2]);

    let left_w = ((area.width as usize * 2 / 5).clamp(30, 48)).min(area.width as usize - 24).max(18) as u16;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(left_w), Constraint::Length(1), Constraint::Min(8)])
        .split(body);
    let (larea, sarea, rarea) = (cols[0], cols[1], cols[2]);

    // 当前会话日志：只换行/构建可见窗口，不再每帧处理整份日志
    let body_h = body.height as usize;
    let prev_key = app.wrap_key.clone();
    app.ensure_wrap(rarea.width as usize);
    if app.wrap_key != prev_key && !app.search.is_empty() {
        app.recompute_matches();
    }
    let total = app.wrapped.len();
    let max_off = total.saturating_sub(body_h);
    if app.log_scroll > max_off {
        app.log_scroll = max_off;
    }

    // 标题栏
    let filt = if app.filter.is_empty() { String::new() } else { format!("  过滤:{}", app.filter) };
    let left_hdr = Paragraph::new(format!(" 会话{}", filt))
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(left_hdr, header_left(header, left_w));
    if let Some(s) = app.current() {
        let t = format!(" {}  ({} 条 / {} 行)", s.title(), s.count, total);
        let p = Paragraph::new(trunc_w(&t, rarea.width as usize))
            .style(Style::default().add_modifier(Modifier::BOLD));
        f.render_widget(p, header_right(header, left_w));
    }

    // 左列表
    let list_h = body_h.saturating_sub(1);
    let top = if app.sel >= list_h && list_h > 0 { app.sel - list_h + 1 } else { 0 };
    let mut items: Vec<Line> = Vec::new();
    for (row, &idx) in app.filtered.iter().enumerate().skip(top).take(list_h) {
        let s = &app.sessions[idx];
        let when = if s.start.len() >= 16 { s.start[..16].replace('T', " ") } else { s.start.clone() };
        let title = trunc_w(s.title(), left_w as usize - 22);
        let text = trunc_w(&format!(" {} {:>4} {}", when, s.count, title), left_w as usize);
        let st = if row == app.sel && !app.focus_log {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        items.push(Line::from(Span::styled(text, st)));
    }
    f.render_widget(Paragraph::new(items), larea);

    // 底部计数
    let cnt = Paragraph::new(format!(" {} 个会话 ", app.filtered.len()))
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(cnt, Rect { x: larea.x, y: larea.y + larea.height - 1, width: larea.width, height: 1 });

    // 分隔线
    let sep = "│\n".repeat(body_h.saturating_sub(1)) + "│";
    f.render_widget(Paragraph::new(sep).style(Style::default().fg(Color::DarkGray)), sarea);

    // 右日志：只取可见窗口这一段（命中处高亮）
    let start = app.log_scroll;
    let end = (start + body_h).min(total);
    let hl = Style::default().fg(Color::Black).bg(Color::Yellow);
    let cur_hl = Style::default().fg(Color::Black).bg(Color::LightGreen);
    let cur_match = app.matches.get(app.match_idx).copied();
    let mut log_items: Vec<Line> = Vec::with_capacity(end.saturating_sub(start));
    for li in start..end {
        let line = &app.wrapped[li];
        let is_match = !app.search.is_empty() && app.matches.binary_search(&li).is_ok();
        let extra = if Some(li) == cur_match { cur_hl } else { hl };
        log_items.push(sline_to_line(line, &app.search, extra, is_match));
    }
    f.render_widget(Paragraph::new(log_items), rarea);

    // 状态栏
    let bar = if let Some(n) = &app.notice {
        format!(" {} ", n)
    } else if app.confirm_delete.is_some() {
        " 删除该会话？ y 确认 · n/Esc 取消 ".to_string()
    } else if app.rename_input {
        format!(" 重命名会话: {}▏  (Enter 保存 / Esc 取消)", app.rename_name)
    } else if app.new_name_input {
        format!(
            " 新会话命名 [目录: {}]: {}▏  (Enter 开始 / Esc 取消)",
            app.new_cwd.display(),
            app.new_name
        )
    } else if app.search_input {
        format!(" 搜索: {}▏  (Enter 确定 / Esc 清除)", app.search)
    } else if app.filter_input {
        format!(" 过滤: {}▏  (Enter 确定 / Esc 清空)", app.filter)
    } else if !app.search.is_empty() {
        let pos = if app.matches.is_empty() { 0 } else { app.match_idx + 1 };
        format!(
            " 搜索 \"{}\": {}/{} · n/N 跳转 · ↑↓ 滚动 · Esc 清除 · q 退出",
            app.search,
            pos,
            app.matches.len()
        )
    } else if app.focus_log {
        let o = if app.show_output { "开" } else { "关" };
        let t = if app.show_thinking { "开" } else { "关" };
        let c = if app.conversation { " · 对话模式" } else { "" };
        format!(
            " 日志 {}/{} · 输出:{} 思考:{}{} · / 搜索 · Tab 回列表 · ↵ 进入 · q 退出",
            app.log_scroll, max_off, o, t, c
        )
    } else {
        " n 新建 · ↵ 进入 · Tab 切焦点 · / 过滤 · Del 删除 · q 退出".to_string()
    };
    let mut style = Style::default().add_modifier(Modifier::REVERSED);
    if app.notice.is_some() || app.confirm_delete.is_some() {
        style = style.fg(Color::Red);
    } else if app.rename_input || app.new_name_input {
        style = style.fg(Color::Green);
    } else if app.search_input || app.filter_input || !app.search.is_empty() {
        style = style.fg(Color::Blue);
    }
    let sb = Paragraph::new(trunc_w(&bar, area.width as usize)).style(style);
    f.render_widget(sb, status);
}

fn header_left(header: Rect, left_w: u16) -> Rect {
    Rect { x: header.x, y: header.y, width: left_w, height: 1 }
}
fn header_right(header: Rect, left_w: u16) -> Rect {
    Rect {
        x: header.x + left_w + 1,
        y: header.y,
        width: header.width.saturating_sub(left_w + 1),
        height: 1,
    }
}

// ---------------------------------------------------------------------------
// 删除：与 pi /resume 一致 —— 先试 trash CLI，否则回退 unlink
// ---------------------------------------------------------------------------
enum DeleteMethod {
    Trash,
    Unlink,
}

// ---------------------------------------------------------------------------
// 时间戳 / 重命名
// ---------------------------------------------------------------------------
fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn ms_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (y, mo, d) = civil_from_days(days);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, mo, d, h, mi, s, millis)
}

/// 追加一条 session_info 记录来重命名（与 pi 机制一致）。
fn rename_session(path: &Path, name: &str) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut last_id = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(id) = v.get("id").and_then(|x| x.as_str()) {
                last_id = id.to_string();
            }
        }
    }
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let id = format!("{:08x}", (nanos as u64 ^ ((std::process::id() as u64) << 20)) as u32);
    let mut entry = serde_json::Map::new();
    entry.insert("type".into(), Value::String("session_info".into()));
    entry.insert("id".into(), Value::String(id));
    if !last_id.is_empty() {
        entry.insert("parentId".into(), Value::String(last_id));
    }
    entry.insert("timestamp".into(), Value::String(ms_to_iso(now_ms())));
    entry.insert("name".into(), Value::String(name.to_string()));
    let line = serde_json::to_string(&Value::Object(entry)).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    writeln!(f, "{}", line).map_err(|e| e.to_string())?;
    Ok(())
}

fn delete_session_file(path: &Path) -> Result<DeleteMethod, String> {
    let mut cmd = std::process::Command::new("trash");
    if path.to_string_lossy().starts_with('-') {
        cmd.arg("--");
    }
    cmd.arg(path);
    // trash 存在且成功（或文件已消失）→ 视为移入回收站
    if let Ok(out) = cmd.output() {
        if out.status.success() || !path.exists() {
            return Ok(DeleteMethod::Trash);
        }
    }
    // 否则回退到直接删除
    match fs::remove_file(path) {
        Ok(_) => Ok(DeleteMethod::Unlink),
        Err(e) => {
            if !path.exists() {
                Ok(DeleteMethod::Unlink)
            } else {
                Err(e.to_string())
            }
        }
    }
}

enum Action {
    Quit,
    Launch(PathBuf),
    NewSession { name: Option<String>, cwd: PathBuf },
}

fn suspend_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
) -> io::Result<()> {
    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }
    terminal.show_cursor()?;
    Ok(())
}

fn resume_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
) -> io::Result<()> {
    enable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    }
    terminal.clear()?;
    Ok(())
}

fn reload_sessions(app: &mut App) {
    app.sessions = load_all();
    app.sessions.sort_by(|a, b| b.modified.cmp(&a.modified));
    app.cache.clear();
    app.wrap_key = None;
    app.reindex();
}

/// 把终端交给 pi 跑完，再回到查看器并重载会话。
fn run_pi_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
    app: &mut App,
    cmd: &mut std::process::Command,
) -> io::Result<()> {
    suspend_terminal(terminal, mouse)?;
    if let Err(e) = cmd.status() {
        eprintln!("无法启动 pi: {e}");
    }
    resume_terminal(terminal, mouse)?;
    reload_sessions(app);
    Ok(())
}

fn run_tui(thinking: bool, mouse: bool) -> io::Result<()> {
    let mut app = App::new(load_all());
    app.show_thinking = thinking;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if mouse {
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    } else {
        execute!(stdout, EnterAlternateScreen)?;
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = loop {
        match event_loop(&mut terminal, &mut app, mouse) {
            Ok(Action::Quit) => break Ok(()),
            Ok(Action::Launch(path)) => {
                let mut cmd = std::process::Command::new("pi");
                cmd.arg("--session").arg(&path);
                run_pi_command(&mut terminal, mouse, &mut app, &mut cmd)?;
            }
            Ok(Action::NewSession { name, cwd }) => {
                let mut cmd = std::process::Command::new("pi");
                if let Some(n) = &name {
                    cmd.arg("--name").arg(n);
                }
                cmd.current_dir(&cwd);
                run_pi_command(&mut terminal, mouse, &mut app, &mut cmd)?;
            }
            Err(e) => break Err(e),
        }
    };

    disable_raw_mode()?;
    if mouse {
        execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    } else {
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    }
    terminal.show_cursor()?;
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    mouse: bool,
) -> io::Result<Action> {
    loop {
        terminal.draw(|f| ui(f, app))?;
        match event::read()? {
            Event::Key(k) if k.kind == KeyEventKind::Press || k.kind == KeyEventKind::Repeat => {
                app.notice = None;
                if app.confirm_delete.is_some() {
                    match k.code {
                        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => app.delete_current(),
                        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.confirm_delete = None,
                        _ => {}
                    }
                    continue;
                }
                if app.filter_input {
                    match k.code {
                        KeyCode::Enter => app.filter_input = false,
                        KeyCode::Esc => {
                            app.filter.clear();
                            app.filter_input = false;
                            app.reindex();
                        }
                        KeyCode::Backspace => {
                            app.filter.pop();
                            app.reindex();
                        }
                        KeyCode::Char(c) => {
                            app.filter.push(c);
                            app.reindex();
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.search_input {
                    match k.code {
                        KeyCode::Enter => app.search_input = false,
                        KeyCode::Esc => {
                            app.search.clear();
                            app.search_input = false;
                            app.matches.clear();
                            app.match_idx = 0;
                        }
                        KeyCode::Backspace => {
                            app.search.pop();
                            app.recompute_matches();
                        }
                        KeyCode::Char(c) => {
                            app.search.push(c);
                            app.recompute_matches();
                        }
                        _ => {}
                    }
                    continue;
                }
                if app.new_name_input {
                    match k.code {
                        KeyCode::Enter => {
                            let name = app.new_name.trim().to_string();
                            app.new_name_input = false;
                            app.new_name.clear();
                            return Ok(Action::NewSession {
                                name: if name.is_empty() { None } else { Some(name) },
                                cwd: app.new_cwd.clone(),
                            });
                        }
                        KeyCode::Esc => {
                            app.new_name_input = false;
                            app.new_name.clear();
                        }
                        KeyCode::Backspace => {
                            app.new_name.pop();
                        }
                        KeyCode::Char(c) => app.new_name.push(c),
                        _ => {}
                    }
                    continue;
                }
                if app.rename_input {
                    match k.code {
                        KeyCode::Enter => {
                            let name = app.rename_name.trim().to_string();
                            app.rename_input = false;
                            if !name.is_empty() {
                                app.rename_current(&name);
                            }
                        }
                        KeyCode::Esc => app.rename_input = false,
                        KeyCode::Backspace => {
                            app.rename_name.pop();
                        }
                        KeyCode::Char(c) => app.rename_name.push(c),
                        _ => {}
                    }
                    continue;
                }
                if !matches!(k.code, KeyCode::Char('g')) {
                    app.pending_g = false;
                }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(Action::Quit),
                    KeyCode::Esc => {
                        if app.focus_log && !app.search.is_empty() {
                            app.search.clear();
                            app.matches.clear();
                            app.match_idx = 0;
                        } else if !app.focus_log && !app.filter.is_empty() {
                            app.filter.clear();
                            app.reindex();
                        } else {
                            return Ok(Action::Quit);
                        }
                    }
                    KeyCode::Char('/') => {
                        if app.focus_log {
                            app.search_input = true;
                            app.search.clear();
                            app.matches.clear();
                            app.match_idx = 0;
                        } else {
                            app.filter_input = true;
                            app.filter.clear();
                            app.reindex();
                        }
                    }
                    KeyCode::Tab => app.focus_log = !app.focus_log,
                    KeyCode::Char('n') if app.focus_log => app.jump_match(1),
                    KeyCode::Char('N') if app.focus_log => app.jump_match(-1),
                    KeyCode::Char('n') if !app.focus_log => {
                        app.new_name_input = true;
                        app.new_name.clear();
                        // 默认在「选中会话的目录」下新建（不存在则用当前目录）
                        app.new_cwd = app
                            .current()
                            .map(|s| PathBuf::from(&s.cwd))
                            .filter(|p| p.is_dir())
                            .or_else(|| std::env::current_dir().ok())
                            .unwrap_or_else(|| PathBuf::from("."));
                    }
                    KeyCode::Char('g') => {
                        if app.pending_g {
                            if app.focus_log {
                                app.log_scroll = 0;
                            } else {
                                app.sel = 0;
                                app.log_scroll = 0;
                            }
                            app.pending_g = false;
                        } else {
                            app.pending_g = true;
                        }
                    }
                    KeyCode::Char('G') => {
                        if app.focus_log {
                            app.log_scroll = usize::MAX;
                        } else {
                            app.sel = app.filtered.len().saturating_sub(1);
                            app.log_scroll = 0;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(s) = app.current() {
                            return Ok(Action::Launch(s.path.clone()));
                        }
                    }
                    KeyCode::Delete => app.request_delete(),
                    KeyCode::Char('c') => {
                        app.conversation = !app.conversation;
                        app.cache.clear();
                        app.wrap_key = None;
                    }
                    KeyCode::Char('o') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.show_output = !app.show_output;
                        app.cache.clear();
                        app.wrap_key = None;
                    }
                    KeyCode::Char('t') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.show_thinking = !app.show_thinking;
                        app.cache.clear();
                        app.wrap_key = None;
                    }
                    KeyCode::Char('r') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        if app.current().is_some() {
                            app.rename_name =
                                app.current().and_then(|s| s.name.clone()).unwrap_or_default();
                            app.rename_input = true;
                        }
                    }
                    KeyCode::Backspace if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.request_delete()
                    }
                    KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.request_delete()
                    }
                    _ => {
                        let list_h = terminal.size()?.height.saturating_sub(3) as usize;
                        if !app.focus_log {
                            match k.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.sel = app.sel.saturating_sub(1);
                                    app.log_scroll = 0;
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if app.sel + 1 < app.filtered.len() {
                                        app.sel += 1;
                                    }
                                    app.log_scroll = 0;
                                }
                                KeyCode::PageUp => {
                                    app.sel = app.sel.saturating_sub(list_h);
                                    app.log_scroll = 0;
                                }
                                KeyCode::PageDown => {
                                    app.sel = (app.sel + list_h).min(app.filtered.len().saturating_sub(1));
                                    app.log_scroll = 0;
                                }
                                KeyCode::Home => {
                                    app.sel = 0;
                                    app.log_scroll = 0;
                                }
                                KeyCode::End => {
                                    app.sel = app.filtered.len().saturating_sub(1);
                                    app.log_scroll = 0;
                                }
                                _ => {}
                            }
                        } else {
                            let page = terminal.size()?.height.saturating_sub(2) as usize;
                            let step = (page / 2).max(1);
                            match k.code {
                                KeyCode::Up => app.log_scroll = app.log_scroll.saturating_sub(1),
                                KeyCode::Down => app.log_scroll += 1,
                                KeyCode::Char('k') => {
                                    app.log_scroll = app.log_scroll.saturating_sub(step)
                                }
                                KeyCode::Char('j') => app.log_scroll += step,
                                KeyCode::PageUp => app.log_scroll = app.log_scroll.saturating_sub(page),
                                KeyCode::PageDown => app.log_scroll += page,
                                KeyCode::Home => app.log_scroll = 0,
                                KeyCode::End => app.log_scroll = usize::MAX,
                                _ => {}
                            }
                        }
                    }
                }
            }
            Event::Mouse(m) if mouse => match m.kind {
                MouseEventKind::ScrollDown => {
                    if app.focus_log {
                        app.log_scroll = app.log_scroll.saturating_add(1);
                    } else if app.sel + 1 < app.filtered.len() {
                        app.sel += 1;
                        app.log_scroll = 0;
                    }
                }
                MouseEventKind::ScrollUp => {
                    if app.focus_log {
                        app.log_scroll = app.log_scroll.saturating_sub(1);
                    } else {
                        app.sel = app.sel.saturating_sub(1);
                        app.log_scroll = 0;
                    }
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// 导出
// ---------------------------------------------------------------------------
fn fmt_when(start: &str) -> String {
    if start.len() >= 16 {
        start[..16].replace('T', " ")
    } else {
        start.to_string()
    }
}

fn do_list(sessions: &[Session]) -> String {
    let mut lines = vec![format!("{:<17}{:>5}  {:<18}{}", "时间", "条数", "名称", "项目路径")];
    for s in sessions {
        let name = trunc_w(s.title(), 18);
        lines.push(format!("{:<17}{:>5}  {:<18}{}", fmt_when(&s.start), s.count, name, s.cwd));
    }
    lines.push(String::new());
    lines.push(format!("共 {} 个会话", sessions.len()));
    lines.join("\n")
}

fn session_header(s: &Session) -> String {
    let bar = "=".repeat(70);
    format!(
        "{}\n# {}\n  项目: {}\n  开始: {}\n  条数: {}\n  文件: {}\n{}",
        bar,
        s.title(),
        s.cwd,
        s.start,
        s.count,
        s.path.display(),
        bar
    )
}

fn write_out(text: &str, output: Option<&str>) {
    match output {
        Some(p) => {
            if let Err(e) = fs::write(p, text) {
                eprintln!("写入失败 {}: {}", p, e);
                std::process::exit(1);
            }
            eprintln!("已写出: {}", p);
        }
        None => {
            use std::io::Write;
            let stdout = io::stdout();
            let mut h = stdout.lock();
            if h.write_all(text.as_bytes()).is_err() {
                std::process::exit(0);
            }
        }
    }
}

const HELP: &str = "\
pilog - pi 会话日志查看器 / 导出工具 (Rust)

用法:
  pilog                   打开查看器（左列表 / 右日志，q 退出）
  pilog <会话.jsonl>      导出指定会话
  pilog --latest          导出最近一次会话
  pilog --list            列出所有会话
  pilog --all             导出所有会话（按时间分组）
  pilog --thinking        附上思考过程
  pilog --mouse           启用鼠标/触摸板滚轮（默认关，避免触摸板乱滚）
  pilog --max-lines N     每条输出最多显示 N 行
  pilog -o FILE           写文件（默认 stdout）
  pilog -h                显示帮助

查看器按键（按焦点区分）:
  Tab         切换焦点（会话列表 <-> 对话日志）
  ↑ / ↓       列表: 选会话        | 日志: 单行滚动
  j / k       列表: 选会话        | 日志: 半屏滚动
  PgUp/PgDn   列表: 翻一页会话    | 日志: 严格整页
  gg / G      列表: 首个/末个会话 | 日志: 首行/末尾（Home/End 同）
  /           列表: 过滤会话      | 日志: 搜索关键词
  n / N       列表: n=新建会话(用选中会话的目录) | 日志: 下一个/上一个命中
  Esc         列表: 清过滤 / 退出 | 日志: 清搜索 / 退出
  c           对话模式（只看你和 AI 的对话，隐藏工具调用/输出；AI 回复按 markdown 渲染）
  Ctrl+O      显示/隐藏工具输出
  Ctrl+T      显示/隐藏 AI 思考
  Ctrl+R      重命名选中会话（直接改 session 文件，不打开 pi）
  ↵ Enter     进入选中的会话（两焦点一致）
  Del         删除选中的会话（两焦点一致，需二次确认）
  q           退出
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut session: Option<String> = None;
    let mut list = false;
    let mut all = false;
    let mut latest = false;
    let mut pick = false;
    let mut thinking = false;
    let mut mouse = false;
    let mut max_lines: u32 = 0;
    let mut output: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-h" | "--help" => {
                print!("{}", HELP);
                return;
            }
            "-V" | "--version" => {
                println!("pilog {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--list" => list = true,
            "--all" => all = true,
            "--latest" => latest = true,
            "--pick" | "--view" => pick = true,
            "--thinking" => thinking = true,
            "--mouse" => mouse = true,
            "--max-lines" => {
                i += 1;
                max_lines = args.get(i).and_then(|x| x.parse().ok()).unwrap_or(0);
            }
            "-o" | "--output" => {
                i += 1;
                output = args.get(i).cloned();
            }
            other if !other.starts_with('-') => session = Some(other.to_string()),
            _ => {}
        }
        i += 1;
    }

    let mut sessions = load_all();
    sessions.sort_by(|a, b| b.modified.cmp(&a.modified));

    if list {
        write_out(&format!("{}\n", do_list(&sessions)), output.as_deref());
        return;
    }

    if all {
        let mut by_start = sessions.clone();
        by_start.sort_by_key(|s| s.created);
        let mut chunks = Vec::new();
        for s in &by_start {
            chunks.push(session_header(s));
            let cfg = load_config();
            chunks.push(log_to_text(&render_log(
                &s.path, thinking, true, false, false, max_lines, &cfg,
            )));
        }
        let text = chunks.join("\n");
        write_out(&text, output.as_deref());
        eprintln!("共导出 {} 个会话", by_start.len());
        return;
    }

    let interactive = pick
        || (session.is_none()
            && !all
            && !latest
            && io::stdin().is_terminal());

    if interactive {
        if let Err(e) = run_tui(thinking, mouse) {
            eprintln!("查看器错误: {}", e);
            std::process::exit(1);
        }
        return;
    }

    let path = match session {
        Some(s) => PathBuf::from(s),
        None => match find_latest() {
            Some(p) => p,
            None => {
                eprintln!("找不到任何 pi 会话文件");
                std::process::exit(1);
            }
        },
    };
    if !path.is_file() {
        eprintln!("找不到会话文件: {}", path.display());
        std::process::exit(1);
    }
    let basename = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let body = log_to_text(&render_log(
        &path,
        thinking,
        true,
        false,
        false,
        max_lines,
        &load_config(),
    ));
    write_out(&format!("# {}\n\n{}", basename, body), output.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn make_fixture(text_json: &str) {
        let dir = std::path::Path::new("/tmp/pilog-test-render/--tmp-home--");
        std::fs::create_dir_all(dir).unwrap();
        let content = format!(
            "{}\n{}\n{}\n",
            "{\"type\":\"session\",\"id\":\"a\",\"timestamp\":\"2026-10-05T00:00:00.000Z\",\"cwd\":\"/tmp/home\"}",
            "{\"type\":\"message\",\"timestamp\":\"2026-10-05T00:00:05.000Z\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}",
            format!("{{\"type\":\"message\",\"timestamp\":\"2026-10-05T00:00:06.000Z\",\"message\":{{\"role\":\"toolResult\",\"toolCallId\":\"c1\",\"toolName\":\"bash\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}}}", text_json)
        );
        std::fs::write(dir.join("s.jsonl"), content).unwrap();
    }

    fn dump(label: &str, text_json: &str, w: u16, h: u16) -> String {
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-test-render");
        make_fixture(text_json);
        let mut app = App::new(load_all());
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let mut full = String::new();
        println!("===== {} =====", label);
        for y in 0..h {
            let mut row = String::new();
            for x in 0..w {
                row.push_str(buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "));
            }
            println!("{:02}|{}|", y, row);
            full.push_str(&row);
            full.push('\n');
        }
        full
    }

    #[test]
    fn conversation_and_markdown() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-test-render");
        let dir = std::path::Path::new("/tmp/pilog-test-render/--tmp-home--");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
        std::fs::create_dir_all(dir).unwrap();
        let content = [
            r#"{"type":"session","id":"a","timestamp":"2026-10-05T00:00:00.000Z","cwd":"/tmp/home"}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:05.000Z","message":{"role":"user","content":"你好"}}"#,
            r###"{"type":"message","timestamp":"2026-10-05T00:00:06.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"SECRET"},{"type":"text","text":"## 标题\n**加粗** 和 `代码`"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}]}}"###,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:07.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"RESULT"}],"isError":false}}"#,
        ]
        .join("\n")
            + "\n";
        std::fs::write(dir.join("s.jsonl"), content).unwrap();
        let path = dir.join("s.jsonl");
        let cfg = load_config();
        let j = |v: &[SLine]| v.iter().map(|l| sline_text(l)).collect::<Vec<_>>().join("\n");

        let normal = j(&render_log(&path, false, true, false, true, 0, &cfg));
        assert!(normal.contains("▶ bash"), "普通模式应含工具调用");
        assert!(normal.contains("标题"), "markdown 应渲染标题");
        assert!(normal.contains("加粗") && !normal.contains("**"), "markdown 应渲染加粗");

        let conv = j(&render_log(&path, true, true, true, true, 0, &cfg));
        assert!(!conv.contains("▶ bash"), "对话模式不应含工具调用");
        assert!(!conv.contains("RESULT"), "对话模式不应含工具输出");
        assert!(!conv.contains("SECRET"), "对话模式不应含思考");
        assert!(conv.contains("你好") && conv.contains("标题"), "对话模式应保留双方对话");

        let raw = j(&render_log(&path, false, true, false, false, 0, &cfg));
        assert!(raw.contains("## 标题") && raw.contains("**加粗**"), "markdown=false 应保留原文");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
    }

    #[test]
    fn config_classify_by_tool() {
        let mut levels = std::collections::HashMap::new();
        levels.insert("write".to_string(), Kind::CmdRisky);
        levels.insert("edit".to_string(), Kind::CmdRisky);
        levels.insert("read".to_string(), Kind::Cmd);
        levels.insert("bash".to_string(), Kind::Cmd);
        let cfg = Config {
            levels,
            bash_heuristic: true,
            danger_cmds: vec!["sudo".into(), "rm".into(), "pkill".into()],
            safe_cmds: vec!["cat".into(), "ls".into()],
        };
        assert_eq!(cfg.classify("write", ""), Kind::CmdRisky);
        assert_eq!(cfg.classify("edit", ""), Kind::CmdRisky);
        assert_eq!(cfg.classify("read", ""), Kind::Cmd);
        assert_eq!(cfg.classify("bash", "ls -la"), Kind::Cmd);
        assert_eq!(cfg.classify("bash", "sudo rm -rf /tmp/x"), Kind::CmdRisky);
        assert_eq!(cfg.classify("bash", "pkill -f foo"), Kind::CmdRisky);
        assert_eq!(cfg.classify("bash", "cat notes-about-rm.txt"), Kind::Cmd, "safe 首词应覆盖");
        assert_eq!(cfg.classify("unknown", ""), Kind::Cmd);
        let off = Config {
            levels: std::collections::HashMap::new(),
            bash_heuristic: false,
            danger_cmds: vec!["sudo".into()],
            safe_cmds: vec![],
        };
        assert_eq!(off.classify("bash", "sudo rm -rf /"), Kind::Cmd);
    }

    #[test]
    fn command_backgrounds_by_tool() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-test-render");
        let dir = std::path::Path::new("/tmp/pilog-test-render/--tmp-home--");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
        std::fs::create_dir_all(dir).unwrap();
        let content = [
            r#"{"type":"session","id":"a","timestamp":"2026-10-05T00:00:00.000Z","cwd":"/tmp/home"}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:05.000Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:06.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c1","name":"write","arguments":{"path":"/tmp/x"}}]}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:07.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"write","content":[{"type":"text","text":"ok"}],"isError":false}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:08.000Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"c2","name":"read","arguments":{"path":"/tmp/y"}}]}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:09.000Z","message":{"role":"toolResult","toolCallId":"c2","toolName":"read","content":[{"type":"text","text":"data"}],"isError":false}}"#,
        ]
        .join("\n")
            + "\n";
        std::fs::write(dir.join("s.jsonl"), content).unwrap();
        let mut app = App::new(load_all());
        app.ensure_wrap(80);
        let mut w = None;
        let mut r = None;
        for line in &app.wrapped {
            let text = sline_text(line);
            if text.contains("▶ write") {
                w = line.first().map(|(st, _)| st.bg);
            }
            if text.contains("▶ read") {
                r = line.first().map(|(st, _)| st.bg);
            }
        }
        assert_eq!(w, Some(Some(Color::Red)), "write 应为红底");
        assert_eq!(r, Some(None), "read 不应有背景");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
    }

    #[test]
    fn toggle_output_and_thinking() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-test-render");
        let dir = std::path::Path::new("/tmp/pilog-test-render/--tmp-home--");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
        std::fs::create_dir_all(dir).unwrap();
        let content = [
            r#"{"type":"session","id":"a","timestamp":"2026-10-05T00:00:00.000Z","cwd":"/tmp/home"}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:05.000Z","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:06.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"SECRET_THOUGHT"},{"type":"text","text":"answer"},{"type":"toolCall","id":"c1","name":"bash","arguments":{"command":"ls"}}]}}"#,
            r#"{"type":"message","timestamp":"2026-10-05T00:00:07.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"bash","content":[{"type":"text","text":"RESULT_LINE"}],"isError":false}}"#,
        ]
        .join("\n")
            + "\n";
        std::fs::write(dir.join("s.jsonl"), content).unwrap();
        let mut app = App::new(load_all());
        let text = |a: &App| a.wrapped.iter().map(|l| sline_text(l)).collect::<Vec<_>>().join("\n");

        app.ensure_wrap(80);
        assert!(text(&app).contains("RESULT_LINE"), "默认应显示输出");
        assert!(!text(&app).contains("SECRET_THOUGHT"), "默认不显示思考");

        app.show_output = false;
        app.cache.clear();
        app.wrap_key = None;
        app.ensure_wrap(80);
        let t = text(&app);
        assert!(!t.contains("RESULT_LINE") && !t.contains("↳ 输出"), "隐藏输出后不应有输出");
        assert!(t.contains("▶ bash"), "命令本身仍应保留");

        app.show_thinking = true;
        app.cache.clear();
        app.wrap_key = None;
        app.ensure_wrap(80);
        assert!(text(&app).contains("SECRET_THOUGHT"), "打开思考后应显示");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
    }

    #[test]
    fn render_splits_lines_and_no_overflow() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let (w, h) = (80u16, 14u16);

        // 多行输出必须拆成独立行，不能挤成一行
        let frame = dump("multiline", "aaa\\nbbb\\nccc", w, h);
        let row_of = |needle: &str| frame.lines().position(|l| l.contains(needle)).unwrap_or(usize::MAX);
        assert!(row_of("aaa") != usize::MAX, "缺少 aaa");
        assert!(row_of("bbb") != usize::MAX, "缺少 bbb");
        assert!(row_of("ccc") != usize::MAX, "缺少 ccc");
        assert_ne!(row_of("aaa"), row_of("bbb"), "aaa/bbb 被挤到同一行");
        assert_ne!(row_of("bbb"), row_of("ccc"), "bbb/ccc 被挤到同一行");

        // 超长 CJK 行：不得溢出到左栏
        let left_w = ((w as usize * 2 / 5).clamp(30, 48)).min(w as usize - 24).max(18);
        let big = dump("cjk", &"长".repeat(300), w, h);
        for row in big.lines() {
            let left: String = row.chars().take(left_w).collect();
            assert!(!left.contains('长'), "左栏被污染: {:?}", left);
        }
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
    }

    #[test]
    fn search_finds_and_jumps() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-test-render");
        make_fixture("alpha NEEDLE beta\\nNEEDLE again");
        let mut app = App::new(load_all());
        app.ensure_wrap(60);
        app.search = "needle".into();
        app.recompute_matches();
        assert!(app.matches.len() >= 2, "应命中至少 2 行，实际 {}", app.matches.len());
        let first = app.log_scroll;
        app.jump_match(1);
        assert_ne!(app.log_scroll, first, "n 应跳到下一个命中");
        let _ = std::fs::remove_dir_all("/tmp/pilog-test-render");
    }

    #[test]
    fn perf_large_session_frame() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // 造一个大会话：2000 条消息，每条带多行输出
        let dir = std::path::Path::new("/tmp/pilog-perf/--tmp-home--");
        let _ = std::fs::remove_dir_all("/tmp/pilog-perf");
        std::fs::create_dir_all(dir).unwrap();
        let mut content = String::new();
        content.push_str("{\"type\":\"session\",\"id\":\"p\",\"timestamp\":\"2026-10-05T00:00:00.000Z\",\"cwd\":\"/tmp/home\"}\n");
        for i in 0..2000 {
            let body = format!("第{}条输出\\n行二\\n行三，这是一些中文内容用来换行测试 {}", i, "x".repeat(60));
            content.push_str(&format!(
                "{{\"type\":\"message\",\"timestamp\":\"2026-10-05T00:00:05.000Z\",\"message\":{{\"role\":\"toolResult\",\"toolCallId\":\"c{}\",\"toolName\":\"bash\",\"content\":[{{\"type\":\"text\",\"text\":\"{}\"}}]}}}}\n",
                i, body
            ));
        }
        std::fs::write(dir.join("p.jsonl"), content).unwrap();

        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/pilog-perf");
        let mut app = App::new(load_all());
        let backend = TestBackend::new(160, 45);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui(f, &mut app)).unwrap(); // 预热
        let n = 200;
        let t = std::time::Instant::now();
        for _ in 0..n {
            app.log_scroll = (app.log_scroll + 1) % 50;
            term.draw(|f| ui(f, &mut app)).unwrap();
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("大会话每帧耗时: {:.3} ms (总行数 {})", per, app.wrapped.len());
        let _ = std::fs::remove_dir_all("/tmp/pilog-perf");
        assert!(per < 20.0, "每帧 {:.3} ms 太慢（>20ms）", per);
    }
}
