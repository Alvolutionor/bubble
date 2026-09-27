use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RECENT_MS: i64 = 24 * 60 * 60 * 1000;
const STALE_MS: i64 = 20 * 60 * 1000;
const BACKGROUND_QUIET_MS: i64 = 2 * 60 * 1000;
pub const SEEN_MS: i64 = 15 * 1000;
const TAIL_BYTES: u64 = 64 * 1024;
const TIMESTAMP_PROBE_BYTES: u64 = 8 * 1024;

#[derive(Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    #[default]
    Idle,
    Running,
    Waiting,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub project: String,
    pub cwd: String,
    #[serde(default)]
    pub state: SessionState,
    pub last_ts: i64,
    #[serde(default)]
    pub changed_at: i64,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub mark: u64,
    #[serde(default)]
    pub dismissed_at: i64,
}

impl Task {
    pub fn visible(&self) -> bool {
        self.last_ts > self.dismissed_at
    }

    pub fn set(&mut self, state: SessionState, at: i64) -> bool {
        if self.state == state {
            return false;
        }
        self.state = state;
        self.changed_at = at;
        if state != SessionState::Waiting {
            self.note.clear();
        }
        true
    }

    pub fn session_link(&self) -> Option<String> {
        is_rollout(&self.transcript).then(|| format!("vscode://openai.chatgpt/local/{}", self.id))
    }

    pub fn opens_in_companion(&self) -> bool {
        !self.transcript.is_empty() && !is_rollout(&self.transcript)
    }
}

#[derive(Default, Deserialize)]
pub struct InboxEvent {
    cwd: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    ts: i64,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    prompt: String,
    #[serde(default)]
    transcript: String,
    #[serde(default)]
    mark: u64,
    #[serde(default)]
    since: i64,
    #[serde(default)]
    agent: String,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TurnMark {
    Active,
    Ended,
    Interrupted,
}

#[derive(Clone, Copy)]
enum CodexState {
    Running(i64),
    Complete,
    Aborted,
}

#[derive(Default)]
struct Rollout {
    size: u64,
    active_at: i64,
    scanned: u64,
    meta_read: bool,
    session: Option<(String, String)>,
    state: Option<CodexState>,
    prompt: String,
}

struct CodexTurn {
    id: String,
    cwd: String,
    path: String,
    prompt: String,
    state: CodexState,
    active_at: i64,
}

pub struct Tracker {
    claude_root: PathBuf,
    codex_root: PathBuf,
    rollouts: HashMap<PathBuf, Rollout>,
    transcripts: HashMap<PathBuf, Seen>,
}

#[derive(Clone)]
struct Seen {
    mtime: i64,
    active_at: i64,
    cwd: Option<String>,
}

#[derive(Clone)]
struct Found {
    path: PathBuf,
    size: u64,
    active_at: i64,
    cwd: String,
}

pub struct Scan {
    claude: Vec<Found>,
    codex: Vec<CodexTurn>,
}

impl Tracker {
    pub fn new(claude_root: PathBuf, codex_root: PathBuf) -> Self {
        Tracker { claude_root, codex_root, rollouts: HashMap::new(), transcripts: HashMap::new() }
    }

    pub fn scan(&mut self, now: i64) -> Scan {
        Scan {
            claude: recent_transcripts(&self.claude_root, now, &mut self.transcripts),
            codex: active_rollouts(&self.codex_root, now, &mut self.rollouts),
        }
    }
}

pub fn apply(tasks: &mut Vec<Task>, scan: Scan, inbox: &Path, now: i64, windows: Option<&[String]>) -> (bool, bool) {
    let open = |cwd: &str| windows.is_none_or(|titles| window_open(cwd, titles));
    let (mut changed, mut alert) = ingest_inbox(tasks, inbox, now);
    let codex = scan
        .codex
        .into_iter()
        .filter(|c| open(&c.cwd) || tasks.iter().any(|t| t.id == c.id))
        .collect();
    let (c, a) = apply_codex(tasks, codex, now);
    changed |= c;
    alert |= a;
    changed |= apply_discovered(tasks, scan.claude.into_iter().filter(|f| open(&f.cwd)).collect());
    let before = tasks.len();
    tasks.retain(|t| t.state == SessionState::Waiting || (open(&t.cwd) && now - t.last_ts < RECENT_MS));
    changed |= tasks.len() != before;
    for t in tasks.iter_mut().filter(|t| !t.transcript.is_empty() && !is_rollout(&t.transcript)) {
        let (c, a) = refresh_claude(t, now);
        changed |= c;
        alert |= a;
    }
    (changed, alert)
}

pub fn window_open(cwd: &str, titles: &[String]) -> bool {
    Path::new(cwd)
        .ancestors()
        .filter_map(|dir| dir.file_name()?.to_str())
        .any(|name| {
            let suffix = format!("{name} - Visual Studio Code");
            titles
                .iter()
                .any(|t| t.strip_suffix(&suffix).is_some_and(|head| head.is_empty() || head.ends_with(' ')))
        })
}

fn project_of(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cwd)
        .to_string()
}

fn first_line(s: &str) -> String {
    s.trim().lines().next().unwrap_or("").chars().take(120).collect()
}

fn modified_ms(path: &Path) -> Option<i64> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64)
}

fn is_rollout(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("rollout-"))
}

fn row<'a>(tasks: &'a mut Vec<Task>, id: &str, cwd: &str) -> &'a mut Task {
    if let Some(i) = tasks.iter().position(|t| t.id == id) {
        return &mut tasks[i];
    }
    tasks.push(Task {
        id: id.to_string(),
        project: project_of(cwd),
        cwd: cwd.to_string(),
        ..Task::default()
    });
    tasks.last_mut().unwrap()
}

fn merge_event(tasks: &mut Vec<Task>, ev: InboxEvent, now: i64) -> bool {
    let untracked_codex = ev.agent == "codex" && !tasks.iter().any(|t| t.id == ev.session_id && is_rollout(&t.transcript));
    if ev.kind == "progress" || untracked_codex {
        return false;
    }
    let id = if ev.session_id.is_empty() { ev.cwd.clone() } else { ev.session_id.clone() };
    let ts = if ev.ts != 0 { ev.ts } else { now };
    let t = row(tasks, &id, &ev.cwd);
    t.last_ts = t.last_ts.max(ts);
    t.cwd = ev.cwd;
    t.project = if ev.project.is_empty() { project_of(&t.cwd) } else { ev.project };
    if !ev.transcript.is_empty() {
        t.transcript = ev.transcript;
        t.mark = ev.mark;
    }
    match ev.kind.as_str() {
        "end" => {
            t.set(SessionState::Idle, ts);
            false
        }
        "start" => {
            if !ev.prompt.is_empty() {
                t.prompt = ev.prompt;
            }
            t.set(SessionState::Running, ts);
            false
        }
        "stop" if !t.transcript.is_empty() => {
            if t.state == SessionState::Idle {
                t.set(SessionState::Running, ts);
            }
            false
        }
        _ => {
            let alert = t.set(SessionState::Waiting, ts);
            if !ev.note.is_empty() {
                t.note = ev.note;
            }
            alert
        }
    }
}

fn under(cwd: &str, root: &str) -> bool {
    let cwd = cwd.to_lowercase();
    let root = root.trim_end_matches(['\\', '/']).to_lowercase();
    !root.is_empty() && cwd.strip_prefix(&root).is_some_and(|rest| rest.is_empty() || rest.starts_with(['\\', '/']))
}

fn mark_seen(tasks: &mut [Task], ev: &InboxEvent, now: i64) -> bool {
    let ts = if ev.ts != 0 { ev.ts } else { now };
    let Some(t) = tasks.iter_mut().find(|t| t.id == ev.session_id && t.state == SessionState::Waiting) else {
        return false;
    };
    under(&t.cwd, &ev.cwd) && ts - ev.since.max(t.changed_at) >= SEEN_MS && t.set(SessionState::Idle, now)
}

fn ingest_inbox(tasks: &mut Vec<Task>, dir: &Path, now: i64) -> (bool, bool) {
    let _ = fs::create_dir_all(dir);
    let Ok(entries) = fs::read_dir(dir) else {
        return (false, false);
    };
    let mut changed = false;
    let mut alert = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match fs::read_to_string(&path).ok().and_then(|c| serde_json::from_str::<InboxEvent>(&c).ok()) {
            Some(ev) if ev.kind == "seen" => changed |= mark_seen(tasks, &ev, now),
            Some(ev) => {
                alert |= merge_event(tasks, ev, now);
                changed = true;
            }
            None => {}
        }
        let _ = fs::remove_file(&path);
    }
    (changed, alert)
}

fn read_tail(path: &Path, from: u64, cap: u64) -> Option<(Vec<u8>, bool)> {
    let mut f = fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = from.max(len.saturating_sub(cap));
    if start >= len {
        return None;
    }
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some((buf, start > from))
}

fn tail_lines(path: &Path, from: u64) -> Vec<String> {
    let Some((buf, cut)) = read_tail(path, from, TAIL_BYTES) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = String::from_utf8_lossy(&buf).lines().map(String::from).collect();
    if cut && !lines.is_empty() {
        lines.remove(0);
    }
    lines
}

fn is_interrupt(entry: &Value) -> bool {
    entry["message"]["content"]
        .to_string()
        .contains("[Request interrupted by user")
}

fn turn_mark(entry: &Value) -> Option<TurnMark> {
    match entry["type"].as_str()? {
        "system" if entry["subtype"] == "stop_hook_summary" => Some(TurnMark::Ended),
        "user" | "assistant" if is_interrupt(entry) => Some(TurnMark::Interrupted),
        "user" | "assistant" => Some(TurnMark::Active),
        _ => None,
    }
}

fn claude_last_mark(path: &Path, from: u64) -> Option<TurnMark> {
    tail_lines(path, from)
        .iter()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| turn_mark(&v))
}

fn prompt_text(entry: &Value) -> Option<String> {
    if entry["type"] != "user" || entry["isMeta"] == true {
        return None;
    }
    let text = match &entry["message"]["content"] {
        Value::String(s) => s.as_str(),
        Value::Array(parts) if parts.first()?["type"] == "text" => parts[0]["text"].as_str()?,
        _ => return None,
    };
    if text.trim_start().starts_with('<') || text.starts_with("[Request interrupted") {
        return None;
    }
    Some(first_line(text))
}

fn iso_ms(s: &str) -> Option<i64> {
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, sec) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let ms = num(20, 23).unwrap_or(0);
    let (y, mo) = if mo <= 2 { (y - 1, mo + 9) } else { (y, mo - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + (153 * mo + 2) / 5 + d - 1;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 24 + h) * 60 + mi) * 60_000 + sec * 1000 + ms)
}

fn first_cwd(path: &Path) -> Option<String> {
    BufReader::new(fs::File::open(path).ok()?)
        .lines()
        .take(50)
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .find_map(|v| v["cwd"].as_str().map(String::from))
}

fn background_busy(transcript: &Path, now: i64) -> bool {
    let Ok(entries) = fs::read_dir(transcript.with_extension("").join("subagents")) else {
        return false;
    };
    let writing: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let id = name.strip_prefix("agent-")?.strip_suffix(".jsonl")?.to_string();
            let m = e.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
            (now - (m.as_millis() as i64) < BACKGROUND_QUIET_MS).then_some(id)
        })
        .collect();
    if writing.is_empty() {
        return false;
    }
    let tail = tail_lines(transcript, 0).join("\n");
    writing.iter().any(|id| !tail.contains(&format!("<task-id>{id}</task-id>")))
}

fn refresh_claude(t: &mut Task, now: i64) -> (bool, bool) {
    let path = PathBuf::from(&t.transcript);
    let grew = fs::metadata(&path).is_ok_and(|md| md.len() > t.mark);
    if let Some(m) = modified_ms(&path).filter(|_| grew) {
        t.last_ts = t.last_ts.max(m);
    }
    let mut changed = false;
    match claude_last_mark(&path, t.mark) {
        Some(TurnMark::Interrupted) => changed |= t.set(SessionState::Idle, now),
        Some(TurnMark::Active) => changed |= t.set(SessionState::Running, now),
        Some(TurnMark::Ended) if t.state != SessionState::Waiting => {
            let busy = background_busy(&path, now);
            if t.state == SessionState::Running && !busy {
                t.set(SessionState::Waiting, now);
                return (true, true);
            }
            if busy {
                changed |= t.set(SessionState::Running, now);
            }
        }
        _ => {}
    }
    if t.state == SessionState::Running && now - t.last_ts > STALE_MS && !background_busy(&path, now) {
        changed |= t.set(SessionState::Idle, now);
    }
    (changed, false)
}

fn discover_claude(found: Found) -> Option<Task> {
    let entries: Vec<Value> = tail_lines(&found.path, 0)
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let running = matches!(entries.iter().rev().find_map(turn_mark), Some(TurnMark::Active));
    Some(Task {
        id: found.path.file_stem()?.to_str()?.to_string(),
        project: project_of(&found.cwd),
        cwd: found.cwd,
        state: if running { SessionState::Running } else { SessionState::Idle },
        last_ts: found.active_at,
        changed_at: found.active_at,
        prompt: entries.iter().rev().find_map(prompt_text).unwrap_or_default(),
        transcript: found.path.to_string_lossy().into_owned(),
        mark: found.size,
        ..Task::default()
    })
}

fn recent_transcripts(root: &Path, now: i64, seen: &mut HashMap<PathBuf, Seen>) -> Vec<Found> {
    let mut out = Vec::new();
    let mut next = HashMap::new();
    for project in fs::read_dir(root).into_iter().flatten().flatten() {
        for e in fs::read_dir(project.path()).into_iter().flatten().flatten() {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            let Some((size, mtime)) = e.metadata().ok().and_then(|md| {
                let m = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
                Some((md.len(), m))
            }) else {
                continue;
            };
            if now - mtime >= RECENT_MS {
                continue;
            }
            let known = seen.get(&path).cloned();
            let entry = match known {
                Some(s) if s.mtime == mtime => s,
                _ => Seen {
                    mtime,
                    active_at: last_timestamp(&path).unwrap_or(mtime),
                    cwd: known.and_then(|s| s.cwd).or_else(|| first_cwd(&path)),
                },
            };
            next.insert(path.clone(), entry.clone());
            if let Some(cwd) = entry.cwd.filter(|_| now - entry.active_at < RECENT_MS) {
                out.push(Found { path, size, active_at: entry.active_at, cwd });
            }
        }
    }
    *seen = next;
    out
}

fn apply_discovered(tasks: &mut Vec<Task>, found: Vec<Found>) -> bool {
    let mut changed = false;
    for f in found {
        let known = f
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|id| tasks.iter().any(|t| t.id == id));
        if known {
            continue;
        }
        if let Some(t) = discover_claude(f) {
            tasks.push(t);
            changed = true;
        }
    }
    changed
}

fn session_of(first_line: &[u8]) -> Option<(String, String)> {
    let meta: Value = serde_json::from_slice(first_line).ok()?;
    let p = &meta["payload"];
    if meta["type"] != "session_meta" || p["thread_source"] != "user" || p["source"] == "exec" {
        return None;
    }
    Some((p["id"].as_str()?.to_string(), p["cwd"].as_str()?.to_string()))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn advance(path: &Path, r: &mut Rollout) {
    let Some((buf, _)) = read_tail(path, r.scanned, u64::MAX) else {
        return;
    };
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else {
        return;
    };
    for line in buf[..end].split(|&b| b == b'\n') {
        if !r.meta_read {
            r.meta_read = true;
            r.session = session_of(line);
            continue;
        }
        let marker = [&b"task_started"[..], b"task_complete", b"turn_aborted", b"user_message"]
            .iter()
            .any(|m| contains(line, m));
        if r.session.is_none() || !marker {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if v["type"] != "event_msg" {
            continue;
        }
        let e = &v["payload"];
        match e["type"].as_str() {
            Some("task_started") => {
                r.state = Some(CodexState::Running(e["started_at"].as_i64().unwrap_or(0) * 1000));
                r.prompt.clear();
            }
            Some("user_message") => r.prompt = first_line(e["message"].as_str().unwrap_or("")),
            Some("task_complete") => r.state = Some(CodexState::Complete),
            Some("turn_aborted") => r.state = Some(CodexState::Aborted),
            _ => {}
        }
    }
    r.scanned += end as u64 + 1;
}

fn last_timestamp(path: &Path) -> Option<i64> {
    let (buf, _) = read_tail(path, 0, TIMESTAMP_PROBE_BYTES)?;
    let text = String::from_utf8_lossy(&buf);
    let key = "\"timestamp\":\"";
    let at = text.rfind(key)? + key.len();
    iso_ms(text.get(at..at + 24)?)
}

fn turn_of(path: &Path, r: &Rollout) -> Option<CodexTurn> {
    let (id, cwd) = r.session.clone()?;
    Some(CodexTurn {
        id,
        cwd,
        path: path.to_string_lossy().into_owned(),
        prompt: r.prompt.clone(),
        state: r.state?,
        active_at: r.active_at,
    })
}

fn active_rollouts(root: &Path, now: i64, rollouts: &mut HashMap<PathBuf, Rollout>) -> Vec<CodexTurn> {
    let first_scan = rollouts.is_empty();
    let mut next = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(md) = e.metadata() else {
                continue;
            };
            if md.is_dir() {
                stack.push(path);
                continue;
            }
            if !is_rollout(&path.to_string_lossy()) {
                continue;
            }
            let mut r = rollouts.remove(&path).unwrap_or_else(|| Rollout {
                size: md.len(),
                active_at: if first_scan { last_timestamp(&path).unwrap_or(0) } else { now },
                ..Rollout::default()
            });
            if r.size != md.len() {
                r.size = md.len();
                r.active_at = now;
            }
            next.insert(path, r);
        }
    }
    *rollouts = next;
    rollouts
        .iter_mut()
        .filter(|(_, r)| now - r.active_at < RECENT_MS)
        .filter_map(|(path, r)| {
            advance(path, r);
            turn_of(path, r)
        })
        .collect()
}

fn apply_codex(tasks: &mut Vec<Task>, turns: Vec<CodexTurn>, now: i64) -> (bool, bool) {
    let mut changed = false;
    let mut alert = false;
    for c in turns {
        let fresh = !tasks.iter().any(|t| t.id == c.id);
        let t = row(tasks, &c.id, &c.cwd);
        t.last_ts = t.last_ts.max(c.active_at);
        t.transcript = c.path;
        if !c.prompt.is_empty() {
            t.prompt = c.prompt;
        }
        changed |= fresh;
        match c.state {
            CodexState::Running(_) if now - c.active_at > STALE_MS => {
                changed |= t.set(SessionState::Idle, now);
            }
            CodexState::Running(since)
                if t.state == SessionState::Idle || (t.state == SessionState::Waiting && since > t.changed_at) =>
            {
                changed |= t.set(SessionState::Running, since);
            }
            CodexState::Complete if t.state == SessionState::Running => {
                t.set(SessionState::Waiting, now);
                changed = true;
                alert = true;
            }
            CodexState::Aborted => changed |= t.set(SessionState::Idle, now),
            _ => {}
        }
    }
    (changed, alert)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000_000;

    fn ev(session: &str, kind: &str) -> InboxEvent {
        InboxEvent {
            cwd: "/p".into(),
            session_id: session.into(),
            ts: T0,
            kind: kind.into(),
            ..InboxEvent::default()
        }
    }

    fn temp_file(name: &str, lines: &[&str]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{name}-{}.jsonl", std::process::id()));
        fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        path
    }

    fn append(path: &Path, line: &str) {
        let mut s = fs::read_to_string(path).unwrap();
        s.push_str(line);
        s.push('\n');
        fs::write(path, s).unwrap();
    }

    fn read_codex(path: &Path) -> Option<CodexTurn> {
        let mut r = Rollout { active_at: T0, ..Rollout::default() };
        advance(path, &mut r);
        turn_of(path, &r)
    }

    fn state(tasks: &[Task], id: &str) -> SessionState {
        tasks.iter().find(|t| t.id == id).unwrap().state
    }

    #[test]
    fn hook_events_drive_the_session_state() {
        let mut tasks = Vec::new();
        assert!(!merge_event(&mut tasks, ev("a", "start"), T0));
        assert_eq!(state(&tasks, "a"), SessionState::Running);

        assert!(merge_event(&mut tasks, ev("a", "notify"), T0), "a permission prompt alerts");
        assert!(!merge_event(&mut tasks, ev("a", "notify"), T0), "a duplicate notify must not alert again");

        merge_event(&mut tasks, ev("a", "start"), T0);
        assert_eq!(state(&tasks, "a"), SessionState::Running, "a new prompt clears the waiting state");

        merge_event(&mut tasks, ev("a", "end"), T0);
        assert_eq!(state(&tasks, "a"), SessionState::Idle, "a closed session stays in the recent list");

        merge_event(&mut tasks, ev("b", "notify"), T0);
        merge_event(&mut tasks, ev("c", "start"), T0);
        assert_eq!(state(&tasks, "b"), SessionState::Waiting, "c's prompt must not clear b");
        assert!(!merge_event(&mut tasks, ev("x", "progress"), T0));
        assert!(tasks.iter().all(|t| t.id != "x"));
    }

    #[test]
    fn claude_turn_end_waits_for_the_user() {
        let path = temp_file("claude-turn", &[r#"{"type":"user","message":{"content":"go"}}"#]);
        let mut tasks = Vec::new();
        let mut stop = ev("s", "stop");
        stop.transcript = path.to_string_lossy().into_owned();
        stop.mark = fs::metadata(&path).unwrap().len();
        assert!(!merge_event(&mut tasks, stop, T0), "a Stop alone does not alert before the transcript agrees");
        assert_eq!(state(&tasks, "s"), SessionState::Running);

        append(&path, r#"{"type":"system","subtype":"stop_hook_summary"}"#);
        append(&path, r#"{"type":"ai-title","title":"x"}"#);
        assert_eq!(refresh_claude(&mut tasks[0], T0), (true, true));
        assert_eq!(tasks[0].state, SessionState::Waiting);
        assert_eq!(refresh_claude(&mut tasks[0], T0), (false, false));

        append(&path, r#"{"type":"user","message":{"content":"next"}}"#);
        assert_eq!(refresh_claude(&mut tasks[0], T0), (true, false), "answering in the editor clears it");
        assert_eq!(tasks[0].state, SessionState::Running);

        append(&path, r#"{"type":"user","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#);
        refresh_claude(&mut tasks[0], T0);
        assert_eq!(tasks[0].state, SessionState::Idle, "Esc ends the run without asking for anything");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn background_agents_keep_a_finished_turn_running() {
        let root = std::env::temp_dir().join(format!("bubble-bg-{}", std::process::id()));
        let agents = root.join("s").join("subagents");
        fs::create_dir_all(&agents).unwrap();
        let path = root.join("s.jsonl");
        fs::write(&path, "{\"type\":\"user\",\"message\":{\"content\":\"go\"}}\n").unwrap();
        fs::write(agents.join("agent-a1.jsonl"), "{}\n").unwrap();
        fs::write(agents.join("agent-a1.meta.json"), "{}").unwrap();
        let now = modified_ms(&agents.join("agent-a1.jsonl")).unwrap();

        let mut t = Task {
            id: "s".into(),
            state: SessionState::Running,
            transcript: path.to_string_lossy().into_owned(),
            last_ts: now,
            ..Task::default()
        };
        append(&path, r#"{"type":"system","subtype":"stop_hook_summary"}"#);
        assert_eq!(refresh_claude(&mut t, now), (false, false), "an agent still writing means no one needs the user");
        assert_eq!(t.state, SessionState::Running);

        append(&path, r#"{"type":"user","message":{"content":"<task-notification>\n<task-id>a1</task-id>\n</task-notification>"}}"#);
        append(&path, r#"{"type":"system","subtype":"stop_hook_summary"}"#);
        assert_eq!(refresh_claude(&mut t, now), (true, true), "a reported agent is done");

        t.set(SessionState::Idle, now);
        fs::write(&path, "{\"type\":\"system\",\"subtype\":\"stop_hook_summary\"}\n").unwrap();
        assert_eq!(refresh_claude(&mut t, now), (true, false), "an idle session with a busy agent is running");
        assert_eq!(t.state, SessionState::Running);
        assert!(background_busy(&path, now));
        assert!(!background_busy(&path, now + BACKGROUND_QUIET_MS), "an agent that went quiet no longer blocks");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn permission_answered_in_the_editor_resumes() {
        let path = temp_file("claude-permission", &[r#"{"type":"assistant","message":{"content":[{"type":"tool_use"}]}}"#]);
        let mut tasks = Vec::new();
        let mut asked = ev("p", "notify");
        asked.note = "Claude needs your permission to use Bash".into();
        asked.transcript = path.to_string_lossy().into_owned();
        asked.mark = fs::metadata(&path).unwrap().len();
        assert!(merge_event(&mut tasks, asked, T0));
        assert_eq!(refresh_claude(&mut tasks[0], T0), (false, false));
        assert_eq!(tasks[0].note, "Claude needs your permission to use Bash");

        append(&path, r#"{"type":"user","message":{"content":[{"type":"tool_result"}]}}"#);
        refresh_claude(&mut tasks[0], T0);
        assert_eq!(tasks[0].state, SessionState::Running);
        assert!(tasks[0].note.is_empty());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn staying_on_a_shown_session_clears_it() {
        let waiting = |id: &str| Task {
            id: id.into(),
            cwd: r"C:\work\bubble\src".into(),
            state: SessionState::Waiting,
            changed_at: T0,
            ..Task::default()
        };
        let mut tasks = vec![waiting("s"), waiting("c1")];
        let seen = |cwd: &str, session: &str, since: i64| InboxEvent {
            kind: "seen".into(),
            cwd: cwd.into(),
            session_id: session.into(),
            since,
            ts: T0 + SEEN_MS,
            ..InboxEvent::default()
        };

        assert!(!mark_seen(&mut tasks, &seen(r"c:\work\bubble", "s", T0 + 1), T0), "a glance is not enough");
        assert!(!mark_seen(&mut tasks, &seen(r"c:\work\bub", "s", T0), T0), "shown in another project's window");
        assert!(!mark_seen(&mut tasks, &seen("", "s", T0), T0), "a window without a folder proves nothing");
        assert!(mark_seen(&mut tasks, &seen(r"c:\work\bubble\", "s", T0 - 60_000), T0), "dwell counts from when it began waiting");
        assert_eq!(state(&tasks, "s"), SessionState::Idle);
        assert_eq!(state(&tasks, "c1"), SessionState::Waiting, "only the shown session clears");

        let inbox = std::env::temp_dir().join(format!("bubble-inbox-seen-{}", std::process::id()));
        fs::create_dir_all(&inbox).unwrap();
        fs::write(inbox.join("1.json"), r#"{"kind":"seen","cwd":"/elsewhere","session_id":"nobody","since":0,"ts":1}"#).unwrap();
        assert_eq!(ingest_inbox(&mut tasks, &inbox, T0), (false, false));
        assert_eq!(tasks.len(), 2, "a seen event never creates a row");
        let _ = fs::remove_dir_all(inbox);
    }

    #[test]
    fn rows_link_to_their_session() {
        let claude = Task { id: "s1".into(), transcript: r"C:\p\s1.jsonl".into(), ..Task::default() };
        let codex = Task { id: "c1".into(), transcript: r"C:\x\rollout-a-c1.jsonl".into(), ..Task::default() };
        let hook_only = Task { id: "/p".into(), ..Task::default() };
        assert!(claude.opens_in_companion() && claude.session_link().is_none(), "Claude opens through the companion, never a URI prompt");
        assert_eq!(codex.session_link().unwrap(), "vscode://openai.chatgpt/local/c1", "Codex opens it in its own sidebar");
        assert!(!codex.opens_in_companion());
        assert!(!hook_only.opens_in_companion() && hook_only.session_link().is_none(), "hook-only rows have no session to open");
    }

    #[test]
    fn rows_expire_after_an_hour_unless_waiting() {
        let mut tasks = vec![
            Task { id: "old".into(), last_ts: T0, ..Task::default() },
            Task { id: "ask".into(), last_ts: T0, state: SessionState::Waiting, ..Task::default() },
            Task { id: "new".into(), last_ts: T0 + RECENT_MS, ..Task::default() },
        ];
        let empty = std::env::temp_dir().join(format!("bubble-inbox-{}", std::process::id()));
        let scan = Scan { claude: Vec::new(), codex: Vec::new() };
        assert!(apply(&mut tasks, scan, &empty, T0 + RECENT_MS + 1, None).0);
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["ask", "new"]);
        let _ = fs::remove_dir_all(empty);
    }

    #[test]
    fn recent_rows_need_an_open_editor_window() {
        let titles = vec![
            "main.js - bubble - Visual Studio Code".to_string(),
            "\u{25cf} notes.md - eevi - Visual Studio Code".to_string(),
        ];
        assert!(window_open(r"C:\work\bubble", &titles));
        assert!(window_open(r"D:\eevi\eevi\sub", &titles), "a window on an ancestor folder counts");
        assert!(!window_open(r"C:\work\app", &titles));
        assert!(!window_open(r"C:\work\mybubble", &titles), "names must match whole");

        let mut tasks = vec![
            Task { id: "closed".into(), cwd: r"C:\work\app".into(), last_ts: T0, ..Task::default() },
            Task { id: "asking".into(), cwd: r"C:\work\app".into(), last_ts: T0, state: SessionState::Waiting, ..Task::default() },
            Task { id: "open".into(), cwd: r"C:\work\bubble".into(), last_ts: T0, ..Task::default() },
        ];
        let inbox = std::env::temp_dir().join(format!("bubble-inbox-w-{}", std::process::id()));
        let scan = || Scan { claude: Vec::new(), codex: Vec::new() };
        apply(&mut tasks, scan(), &inbox, T0 + 1, None);
        assert_eq!(tasks.len(), 3, "without window info nothing is dropped");
        apply(&mut tasks, scan(), &inbox, T0 + 1, Some(&titles));
        let ids: Vec<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["asking", "open"], "waiting rows stay even without a window");
        let _ = fs::remove_dir_all(inbox);
    }

    #[test]
    fn dismissed_rows_hide_until_new_activity() {
        let mut t = Task { last_ts: T0, dismissed_at: T0, ..Task::default() };
        assert!(!t.visible());
        t.last_ts = T0 + 1;
        assert!(t.visible());
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(iso_ms("2000-01-01T00:00:00.000Z"), Some(946_684_800_000));
        assert_eq!(iso_ms("2024-02-29T12:00:00.500Z"), Some(1_709_208_000_500));
        assert_eq!(iso_ms("2026-09-24T01:07:28.177Z"), Some(1_790_212_048_177));
        assert_eq!(iso_ms("garbage"), None);
    }

    #[test]
    fn claude_sessions_are_found_from_recent_transcripts() {
        let root = std::env::temp_dir().join(format!("bubble-claude-{}", std::process::id()));
        let project = root.join("c--work-proj");
        fs::create_dir_all(project.join("s1").join("subagents")).unwrap();
        fs::write(project.join("s1").join("subagents").join("agent-x.jsonl"), "{}\n").unwrap();
        let running = project.join("s1.jsonl");
        fs::write(&running, "{\"type\":\"queue-operation\"}\n").unwrap();
        append(&running, r#"{"type":"user","cwd":"/work/proj","message":{"content":"<ide_selection>x</ide_selection>"}}"#);
        append(&running, r#"{"type":"user","cwd":"/work/proj","message":{"content":[{"type":"text","text":"ship the fix\nnow"}]}}"#);
        append(&running, r#"{"type":"assistant","cwd":"/work/elsewhere","message":{"content":[{"type":"tool_use"}]}}"#);
        let idle = project.join("s2.jsonl");
        fs::write(&idle, "{\"type\":\"user\",\"cwd\":\"/work/proj\",\"message\":{\"content\":\"hi\"}}\n{\"type\":\"system\",\"subtype\":\"stop_hook_summary\"}\n").unwrap();

        let touched = project.join("s3.jsonl");
        fs::write(&touched, "{\"type\":\"user\",\"cwd\":\"/work/proj\",\"timestamp\":\"2020-01-01T00:00:00.000Z\",\"message\":{\"content\":\"old\"}}\n").unwrap();

        let now = [&running, &idle, &touched].iter().map(|p| modified_ms(p).unwrap()).max().unwrap();
        let mut seen = HashMap::new();
        let found = recent_transcripts(&root, now, &mut seen);
        assert_eq!(found.len(), 2, "subagent transcripts and long-silent sessions with a fresh mtime are not recent");
        assert!(recent_transcripts(&root, now + RECENT_MS, &mut seen).is_empty());

        let mut tasks = Vec::new();
        assert!(apply_discovered(&mut tasks, found.clone()));
        let s1 = tasks.iter().find(|t| t.id == "s1").unwrap();
        assert_eq!(s1.state, SessionState::Running);
        assert_eq!((s1.cwd.as_str(), s1.project.as_str(), s1.prompt.as_str()), ("/work/proj", "proj", "ship the fix"));
        assert_eq!(state(&tasks, "s2"), SessionState::Idle, "a finished session is recent, not waiting");
        assert!(!apply_discovered(&mut tasks, found), "known sessions are not added twice");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn codex_rollouts() {
        let meta = r#"{"type":"session_meta","payload":{"id":"c1","cwd":"/q","source":"vscode","thread_source":"user"}}"#;
        let started = r#"{"type":"event_msg","payload":{"type":"task_started","started_at":1790000001}}"#;
        let asked = r#"{"type":"event_msg","payload":{"type":"user_message","message":"fix the bug\nplease"}}"#;
        let path = temp_file("rollout-main", &[meta, started, asked]);

        let turn = read_codex(&path).unwrap();
        assert!(matches!(turn.state, CodexState::Running(1_790_000_001_000)));
        assert_eq!(turn.prompt, "fix the bug");

        let mut tasks = Vec::new();
        apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + 2_000);
        assert_eq!(state(&tasks, "c1"), SessionState::Running);

        append(&path, r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#);
        assert_eq!(apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + 3_000), (true, true));
        assert_eq!(state(&tasks, "c1"), SessionState::Waiting);

        append(&path, r#"{"type":"event_msg","payload":{"type":"task_started","started_at":1790000002}}"#);
        apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + 4_000);
        assert_eq!(state(&tasks, "c1"), SessionState::Waiting, "a turn older than the notification must not clear it");
        append(&path, r#"{"type":"event_msg","payload":{"type":"task_started","started_at":1790000005}}"#);
        apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + 5_000);
        assert_eq!(state(&tasks, "c1"), SessionState::Running, "a newer turn means the user answered in the editor");

        append(&path, r#"{"type":"event_msg","payload":{"type":"turn_aborted"}}"#);
        apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + 6_000);
        assert_eq!(state(&tasks, "c1"), SessionState::Idle);

        let exec = temp_file("rollout-exec", &[&meta.replace("vscode", "exec"), started]);
        assert!(read_codex(&exec).is_none(), "headless exec sessions have no window to switch to");
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(exec);
    }

    #[test]
    fn codex_notify_only_counts_for_tracked_sessions() {
        let mut tasks = vec![Task {
            id: "c1".into(),
            transcript: r"C:\x\rollout-a-c1.jsonl".into(),
            state: SessionState::Running,
            ..Task::default()
        }];
        let done = |session: &str| InboxEvent { agent: "codex".into(), ..ev(session, "notify") };
        assert!(!merge_event(&mut tasks, done("title-helper"), T0), "a helper thread without a rollout is not the user's session");
        assert_eq!(tasks.len(), 1);
        assert!(merge_event(&mut tasks, done("c1"), T0));
        assert_eq!(state(&tasks, "c1"), SessionState::Waiting);
    }

    #[test]
    fn stalled_codex_turns_go_idle() {
        let meta = r#"{"type":"session_meta","payload":{"id":"c2","cwd":"/q","source":"vscode","thread_source":"user"}}"#;
        let started = r#"{"type":"event_msg","payload":{"type":"task_started","started_at":1790000001}}"#;
        let path = temp_file("rollout-stalled", &[meta, started]);
        let mut tasks = Vec::new();
        apply_codex(&mut tasks, vec![read_codex(&path).unwrap()], T0 + STALE_MS + 1);
        assert_eq!(state(&tasks, "c2"), SessionState::Idle);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn advance_reads_only_complete_new_lines() {
        let meta = r#"{"type":"session_meta","payload":{"id":"c1","cwd":"/q","source":"vscode","thread_source":"user"}}"#;
        let path = temp_file("rollout-incremental", &[meta]);
        let mut r = Rollout::default();
        advance(&path, &mut r);
        assert!(r.session.is_some() && r.state.is_none());

        let mut body = fs::read_to_string(&path).unwrap();
        body.push_str(r#"{"type":"event_msg","payload":{"type":"task_started","started_at":7}}"#);
        fs::write(&path, &body).unwrap();
        advance(&path, &mut r);
        assert!(r.state.is_none(), "a half-written line waits for its newline");

        body.push('\n');
        fs::write(&path, body).unwrap();
        advance(&path, &mut r);
        assert!(matches!(r.state, Some(CodexState::Running(7_000))));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn rollouts_are_recent_by_growth_or_last_timestamp() {
        let root = std::env::temp_dir().join(format!("bubble-rollouts-{}", std::process::id()));
        let day = root.join("2026").join("09").join("24");
        fs::create_dir_all(&day).unwrap();
        let meta = r#"{"type":"session_meta","payload":{"id":"c1","cwd":"/q","source":"vscode","thread_source":"user"}}"#;
        let done = r#"{"timestamp":"2026-09-24T01:00:00.000Z","type":"event_msg","payload":{"type":"task_complete"}}"#;
        let file = day.join("rollout-x.jsonl");
        fs::write(&file, format!("{meta}\n{done}\n")).unwrap();
        fs::write(day.join("notes.txt"), "x").unwrap();

        let last = iso_ms("2026-09-24T01:00:00.000Z").unwrap();
        let mut rollouts = HashMap::new();
        assert_eq!(active_rollouts(&root, last + 1_000, &mut rollouts).len(), 1, "startup trusts the last timestamp");
        assert!(active_rollouts(&root, last + RECENT_MS, &mut rollouts).is_empty(), "a day of silence is not recent");

        append(&file, r#"{"type":"event_msg","payload":{"type":"task_started","started_at":1}}"#);
        assert_eq!(active_rollouts(&root, last + 2 * RECENT_MS, &mut rollouts).len(), 1, "growth makes it recent again");
        let _ = fs::remove_dir_all(root);
    }
}
