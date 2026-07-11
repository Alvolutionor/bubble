use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

// Collapsed window width. Windows floors a captioned window to SM_CXMIN (~136px),
// so we use 136 there and rely on the extra area being transparent + click-through
// (verified via WindowFromPoint); the ball sits at its right edge. Other platforms
// have no such floor, so the ball's own size is enough.
#[cfg(windows)]
const COL_W: f64 = 136.0;
#[cfg(not(windows))]
const COL_W: f64 = 72.0;
const COL_H: f64 = 72.0;
const PANEL_W: f64 = 232.0;
const PANEL_H: f64 = 320.0;

#[derive(Clone, Serialize, Deserialize)]
struct Task {
    id: String,
    project: String,
    cwd: String,
    session_id: String,
    count: u32,
    last_ts: i64,
}

#[derive(Deserialize)]
struct InboxEvent {
    cwd: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    ts: i64,
}

#[derive(Default)]
struct AppState {
    tasks: Mutex<Vec<Task>>,
    ball_pos: Mutex<Option<(i32, i32)>>,
}

fn bubble_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("bubble")
}
fn inbox_dir() -> PathBuf {
    bubble_dir().join("inbox")
}
fn state_file() -> PathBuf {
    bubble_dir().join("state.json")
}
fn pos_file() -> PathBuf {
    bubble_dir().join("pos.json")
}

fn load_pos() -> Option<(i32, i32)> {
    let s = fs::read_to_string(pos_file()).ok()?;
    let v: [i32; 2] = serde_json::from_str(&s).ok()?;
    Some((v[0], v[1]))
}

fn save_pos(x: i32, y: i32) {
    let _ = fs::create_dir_all(bubble_dir());
    if let Ok(s) = serde_json::to_string(&[x, y]) {
        let _ = fs::write(pos_file(), s);
    }
}

fn apply_visibility(window: &tauri::WebviewWindow, count: usize, ball_pos: Option<(i32, i32)>) {
    if count > 0 {
        let _ = window.show();
    } else {
        // Reset to the collapsed ball before hiding, so the next appearance is
        // always a clean collapsed ball (never a stranded expanded panel).
        let _ = window.set_size(tauri::LogicalSize::new(COL_W, COL_H));
        if let Some((x, y)) = ball_pos {
            let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        }
        let _ = window.hide();
        let _ = window.emit("collapsed", ());
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn load_state() -> Vec<Task> {
    fs::read_to_string(state_file())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_state(tasks: &[Task]) {
    let _ = fs::create_dir_all(bubble_dir());
    if let Ok(s) = serde_json::to_string_pretty(tasks) {
        let _ = fs::write(state_file(), s);
    }
}

fn merge_event(tasks: &mut Vec<Task>, ev: InboxEvent) {
    let project = if ev.project.is_empty() {
        Path::new(&ev.cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&ev.cwd)
            .to_string()
    } else {
        ev.project
    };
    let ts = if ev.ts != 0 { ev.ts } else { now_ms() };
    if let Some(t) = tasks.iter_mut().find(|t| t.cwd == ev.cwd) {
        t.count += 1;
        t.last_ts = ts;
        t.session_id = ev.session_id;
        t.project = project;
    } else {
        tasks.push(Task {
            id: ev.cwd.clone(),
            project,
            cwd: ev.cwd,
            session_id: ev.session_id,
            count: 1,
            last_ts: ts,
        });
    }
}

fn ingest_inbox(tasks: &mut Vec<Task>) -> bool {
    let dir = inbox_dir();
    let _ = fs::create_dir_all(&dir);
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut changed = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(ev) = serde_json::from_str::<InboxEvent>(&content) {
                merge_event(tasks, ev);
                changed = true;
            }
        }
        let _ = fs::remove_file(&path);
    }
    changed
}

fn focus_or_open(project: &str, cwd: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = include_str!("focus_window.ps1");
        let mut path = std::env::temp_dir();
        path.push("bubble_focus.ps1");
        let _ = fs::write(&path, script);
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&path)
            .args(["-Project", project, "-Cwd", cwd])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    // macOS / Linux: `code <cwd>` opens or focuses the folder's window. Less precise
    // than the Windows path (may reuse a window) — a native focus-by-title helper is
    // welcome. See README "Platform support".
    #[cfg(not(windows))]
    {
        let _ = project;
        let _ = Command::new("code").arg(cwd).spawn();
    }
}

fn sorted(tasks: &[Task]) -> Vec<Task> {
    let mut v = tasks.to_vec();
    v.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    v
}

fn remove_task(state: &tauri::State<AppState>, id: &str) -> Vec<Task> {
    let mut tasks = state.tasks.lock().unwrap();
    tasks.retain(|t| t.id != id);
    save_state(&tasks);
    sorted(&tasks)
}

fn clamp_on_screen(window: &tauri::WebviewWindow, x: i32, y: i32, scale: f64) -> (i32, i32) {
    let monitor = match window.current_monitor() {
        Ok(Some(m)) => m,
        _ => return (x, y),
    };
    let msize = monitor.size();
    let mpos = monitor.position();
    let w = (PANEL_W * scale) as i32;
    let h = (PANEL_H * scale) as i32;
    let margin = (8.0 * scale) as i32;
    let min_x = mpos.x + margin;
    let min_y = mpos.y + margin;
    let max_x = (mpos.x + msize.width as i32 - w - margin).max(min_x);
    let max_y = (mpos.y + msize.height as i32 - h - margin).max(min_y);
    (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
}

fn startup_pos(window: &tauri::WebviewWindow) -> (i32, i32) {
    if let Ok(Some(m)) = window.primary_monitor() {
        let scale = window.scale_factor().unwrap_or(1.0);
        let ms = m.size();
        let mp = m.position();
        let x = mp.x + ms.width as i32 - ((COL_W + 16.0) * scale) as i32;
        let y = mp.y + ms.height as i32 - ((COL_H + 56.0) * scale) as i32;
        (x, y)
    } else {
        (1200, 700)
    }
}

#[tauri::command]
fn list_tasks(state: tauri::State<AppState>) -> Vec<Task> {
    let tasks = state.tasks.lock().unwrap();
    sorted(&tasks)
}

#[tauri::command]
fn open_task(id: String, state: tauri::State<AppState>, window: tauri::WebviewWindow) -> Vec<Task> {
    let target = {
        let tasks = state.tasks.lock().unwrap();
        tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.project.clone(), t.cwd.clone()))
    };
    if let Some((project, cwd)) = target {
        focus_or_open(&project, &cwd);
    }
    let list = remove_task(&state, &id);
    apply_visibility(&window, list.len(), *state.ball_pos.lock().unwrap());
    list
}

#[tauri::command]
fn dismiss_task(id: String, state: tauri::State<AppState>, window: tauri::WebviewWindow) -> Vec<Task> {
    let list = remove_task(&state, &id);
    apply_visibility(&window, list.len(), *state.ball_pos.lock().unwrap());
    list
}

#[tauri::command]
fn clear_all(state: tauri::State<AppState>, window: tauri::WebviewWindow) -> Vec<Task> {
    {
        let mut tasks = state.tasks.lock().unwrap();
        tasks.clear();
        save_state(&tasks);
    }
    apply_visibility(&window, 0, *state.ball_pos.lock().unwrap());
    Vec::new()
}

#[tauri::command]
fn expand_window(window: tauri::WebviewWindow, state: tauri::State<AppState>) -> bool {
    let scale = window.scale_factor().unwrap_or(1.0);
    let anchor = (*state.ball_pos.lock().unwrap())
        .or_else(|| window.outer_position().ok().map(|p| (p.x, p.y)));
    let open_up = match (anchor, window.primary_monitor()) {
        (Some((_, by)), Ok(Some(m))) => {
            let mid_y = m.position().y + (m.size().height as i32) / 2;
            (by + (COL_H * scale / 2.0) as i32) > mid_y
        }
        _ => false,
    };
    let _ = window.set_size(tauri::LogicalSize::new(PANEL_W, PANEL_H));
    if let Some((bx, by)) = anchor {
        let right = bx + (COL_W * scale) as i32;
        let new_x = right - (PANEL_W * scale) as i32;
        let new_y = if open_up {
            (by + (COL_H * scale) as i32) - (PANEL_H * scale) as i32
        } else {
            by
        };
        let (nx, ny) = clamp_on_screen(&window, new_x, new_y, scale);
        let _ = window.set_position(tauri::PhysicalPosition::new(nx, ny));
    }
    let _ = window.set_focus();
    open_up
}

#[tauri::command]
fn collapse_window(window: tauri::WebviewWindow, state: tauri::State<AppState>) {
    let anchor = *state.ball_pos.lock().unwrap();
    let _ = window.set_size(tauri::LogicalSize::new(COL_W, COL_H));
    if let Some((bx, by)) = anchor {
        let _ = window.set_position(tauri::PhysicalPosition::new(bx, by));
    }
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}))
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            let count = {
                let state = app.state::<AppState>();
                let mut tasks = state.tasks.lock().unwrap();
                *tasks = load_state();
                ingest_inbox(&mut tasks);
                save_state(&tasks);
                tasks.len()
            };
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_size(tauri::LogicalSize::new(COL_W, COL_H));
                let start = load_pos().unwrap_or_else(|| startup_pos(&win));
                let _ = win.set_position(tauri::PhysicalPosition::new(start.0, start.1));
                *app.state::<AppState>().ball_pos.lock().unwrap() = Some(start);
                apply_visibility(&win, count, Some(start));
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let mut last_saved: Option<(i32, i32)> = None;
                loop {
                    std::thread::sleep(Duration::from_millis(1500));
                    let state = handle.state::<AppState>();
                    let (changed, count) = {
                        let mut tasks = state.tasks.lock().unwrap();
                        let c = ingest_inbox(&mut tasks);
                        if c {
                            save_state(&tasks);
                        }
                        (c, tasks.len())
                    };
                    if let Some(win) = handle.get_webview_window("main") {
                        if changed {
                            let bp = *state.ball_pos.lock().unwrap();
                            apply_visibility(&win, count, bp);
                            let _ = handle.emit("tasks-updated", ());
                        }
                        let scale = win.scale_factor().unwrap_or(1.0);
                        if let (Ok(sz), Ok(pos)) = (win.inner_size(), win.outer_position()) {
                            if (sz.width as f64) < 184.0 * scale {
                                let cur = (pos.x, pos.y);
                                let anchor = *state.ball_pos.lock().unwrap();
                                let moved = anchor.map_or(true, |(bx, by)| {
                                    (cur.0 - bx).abs() > 10 || (cur.1 - by).abs() > 10
                                });
                                if moved && last_saved != Some(cur) {
                                    save_pos(cur.0, cur.1);
                                    *state.ball_pos.lock().unwrap() = Some(cur);
                                    last_saved = Some(cur);
                                }
                            }
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            open_task,
            dismiss_task,
            clear_all,
            expand_window,
            collapse_window,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
