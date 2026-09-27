mod sessions;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sessions::{SessionState, Task, Tracker};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateEllipticRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, HGDIOBJ,
    RGN_OR,
};

// Collapsed window width. Windows floors a captioned window to SM_CXMIN (~136px),
// so we use 136 there and rely on the extra area being transparent + click-through
// (verified via WindowFromPoint); the ball sits at its right edge. Other platforms
// have no such floor, so the ball's own size is enough.
#[cfg(windows)]
const COL_W: f64 = 136.0;
#[cfg(not(windows))]
const COL_W: f64 = 72.0;
const COL_H: f64 = 72.0;
const BALL_SIZE: f64 = 54.0;
const BALL_MARGIN: f64 = 9.0;
const PANEL_W: f64 = 304.0;
const PANEL_H: f64 = 320.0;

fn collapsed_region(scale: f64) -> (i32, i32, i32, i32) {
    (
        ((COL_W - BALL_MARGIN - BALL_SIZE) * scale).round() as i32,
        0,
        (COL_W * scale).round() as i32,
        (COL_H * scale).round() as i32,
    )
}

#[cfg(windows)]
fn set_collapsed_region(
    window: &tauri::WebviewWindow,
    collapsed: bool,
    show_badge: bool,
) -> bool {
    let Ok(hwnd) = window.hwnd() else {
        return false;
    };
    unsafe {
        if !collapsed {
            return SetWindowRgn(hwnd, None, true) != 0;
        }
        let scale = window.scale_factor().unwrap_or(1.0);
        let (left, _, right, _) = collapsed_region(scale);
        let ball_top = (BALL_MARGIN * scale).round() as i32;
        let ball_right = left + (BALL_SIZE * scale).round() as i32;
        let ball_bottom = ball_top + (BALL_SIZE * scale).round() as i32;
        let region = CreateEllipticRgn(left, ball_top, ball_right, ball_bottom);
        if region.is_invalid() {
            return false;
        }
        if show_badge {
            let badge = CreateRoundRectRgn(
                left + (27.0 * scale).round() as i32,
                ball_top - (3.0 * scale).round() as i32,
                right - (6.0 * scale).round() as i32,
                ball_top + (16.0 * scale).round() as i32,
                (19.0 * scale).round() as i32,
                (19.0 * scale).round() as i32,
            );
            if badge.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(region.0));
                return false;
            }
            if CombineRgn(Some(region), Some(region), Some(badge), RGN_OR).0 == 0 {
                let _ = DeleteObject(HGDIOBJ(region.0));
                let _ = DeleteObject(HGDIOBJ(badge.0));
                return false;
            }
            let _ = DeleteObject(HGDIOBJ(badge.0));
        }
        if SetWindowRgn(hwnd, Some(region), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
            return false;
        }
    }
    true
}

#[cfg(not(windows))]
fn set_collapsed_region(
    _window: &tauri::WebviewWindow,
    _collapsed: bool,
    _show_badge: bool,
) -> bool {
    true
}

#[derive(Default)]
struct AppState {
    tasks: Mutex<Vec<Task>>,
    ball_pos: Mutex<Option<(i32, i32)>>,
    // ponytail: set while we move the window ourselves, so WindowEvent::Moved
    // can tell our own repositioning apart from a user drag.
    moving_self: Mutex<bool>,
    // ponytail: the panel->ball delta actually applied at expand time. Recomputing
    // it is wrong because clamp_on_screen can shift the panel, making the
    // transform non-invertible near screen edges. None until an expand sets it,
    // so a panel-sized window we did not position cannot corrupt ball_pos.
    panel_delta: Mutex<Option<(i32, i32)>>,
    tracker: Mutex<Option<Tracker>>,
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
fn open_request_file() -> PathBuf {
    bubble_dir().join("open.json")
}
fn claude_projects_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude")
        .join("projects")
}
fn codex_sessions_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".codex")
        .join("sessions")
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

fn move_self(window: &tauri::WebviewWindow, state: &AppState, x: i32, y: i32) {
    *state.moving_self.lock().unwrap() = true;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    *state.moving_self.lock().unwrap() = false;
}

fn show_ball(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let bp = *app.state::<AppState>().ball_pos.lock().unwrap();
        let _ = win.set_size(tauri::LogicalSize::new(COL_W, COL_H));
        if let Some((x, y)) = bp {
            move_self(&win, &app.state::<AppState>(), x, y);
        }
        let show_badge = app
            .state::<AppState>()
            .tasks
            .lock()
            .unwrap()
            .iter()
            .any(|task| task.state == SessionState::Waiting);
        let _ = set_collapsed_region(&win, true, show_badge);
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.emit("collapsed", ());
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

fn poll(state: &AppState) -> (bool, bool, bool) {
    let now = now_ms();
    let scan = state
        .tracker
        .lock()
        .unwrap()
        .get_or_insert_with(|| Tracker::new(claude_projects_dir(), codex_sessions_dir()))
        .scan(now);
    let titles = vscode_titles();
    let mut tasks = state.tasks.lock().unwrap();
    let (changed, alert) = sessions::apply(&mut tasks, scan, &inbox_dir(), now, titles.as_deref());
    if changed {
        save_state(&tasks);
    }
    let show_badge = tasks.iter().any(|task| task.state == SessionState::Waiting);
    (changed, alert, show_badge)
}

#[cfg(windows)]
fn vscode_titles() -> Option<Vec<String>> {
    type Hwnd = isize;
    #[link(name = "user32")]
    extern "system" {
        fn EnumWindows(callback: extern "system" fn(Hwnd, isize) -> i32, param: isize) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, text: *mut u16, max: i32) -> i32;
        fn IsWindowVisible(hwnd: Hwnd) -> i32;
    }
    extern "system" fn collect(hwnd: Hwnd, param: isize) -> i32 {
        let titles = unsafe { &mut *(param as *mut Vec<String>) };
        let mut buf = [0u16; 512];
        let len = unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
        };
        let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        if title.ends_with(" - Visual Studio Code") {
            titles.push(title);
        }
        1
    }
    let mut titles: Vec<String> = Vec::new();
    unsafe { EnumWindows(collect, &mut titles as *mut Vec<String> as isize) };
    titles.sort();
    Some(titles)
}

#[cfg(not(windows))]
fn vscode_titles() -> Option<Vec<String>> {
    None
}

fn focus_or_open(project: &str, cwd: &str, link: Option<String>) {
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
            .args(link.iter().flat_map(|l| ["-Link", l.as_str()]))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    // macOS / Linux: `code <cwd>` opens or focuses the folder's window. Less precise
    // than the Windows path (may reuse a window) — a native focus-by-title helper is
    // welcome. See README "Platform support".
    #[cfg(not(windows))]
    {
        let _ = (project, link);
        let _ = Command::new("code").arg(cwd).spawn();
    }
}

fn request_companion_open(t: &Task) {
    let tmp = bubble_dir().join("open.json.tmp");
    let req = serde_json::json!({ "cwd": t.cwd, "session_id": t.id, "ts": now_ms() });
    if fs::write(&tmp, req.to_string()).is_ok() {
        let _ = fs::rename(&tmp, open_request_file());
    }
}

fn visible(tasks: &[Task]) -> Vec<Task> {
    let mut v: Vec<Task> = tasks.iter().filter(|t| t.visible()).cloned().collect();
    v.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
    v
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

fn open_up_for(
    window: &tauri::WebviewWindow,
    anchor: Option<(i32, i32)>,
    scale: f64,
) -> bool {
    match (anchor, window.primary_monitor()) {
        (Some((_, by)), Ok(Some(m))) => {
            let mid_y = m.position().y + (m.size().height as i32) / 2;
            (by + (COL_H * scale / 2.0) as i32) > mid_y
        }
        _ => false,
    }
}

// Lets the webview lay the panel out in the still-collapsed window, so the resize
// and the panel's first paint land in the same frame instead of two.
#[tauri::command]
fn peek_open_up(window: tauri::WebviewWindow, state: tauri::State<AppState>) -> bool {
    let scale = window.scale_factor().unwrap_or(1.0);
    let anchor = *state.ball_pos.lock().unwrap();
    open_up_for(&window, anchor, scale)
}

fn handled(t: &mut Task, now: i64) {
    if t.state == SessionState::Waiting {
        t.set(SessionState::Idle, now);
    }
}

#[tauri::command]
fn list_tasks(state: tauri::State<AppState>) -> Vec<Task> {
    let tasks = state.tasks.lock().unwrap();
    visible(&tasks)
}

#[tauri::command]
fn open_task(id: String, state: tauri::State<AppState>) -> Vec<Task> {
    let mut tasks = state.tasks.lock().unwrap();
    if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
        if t.opens_in_companion() {
            request_companion_open(t);
        }
        focus_or_open(&t.project, &t.cwd, t.session_link());
        handled(t, now_ms());
    }
    save_state(&tasks);
    visible(&tasks)
}

#[tauri::command]
fn dismiss_task(id: String, state: tauri::State<AppState>) -> Vec<Task> {
    let mut tasks = state.tasks.lock().unwrap();
    let now = now_ms();
    if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
        handled(t, now);
        t.dismissed_at = now.max(t.last_ts);
    }
    save_state(&tasks);
    visible(&tasks)
}

#[tauri::command]
fn clear_all(state: tauri::State<AppState>) -> Vec<Task> {
    let mut tasks = state.tasks.lock().unwrap();
    let now = now_ms();
    tasks.iter_mut().for_each(|t| handled(t, now));
    save_state(&tasks);
    visible(&tasks)
}

#[tauri::command]
fn expand_window(window: tauri::WebviewWindow, state: tauri::State<AppState>) -> bool {
    let scale = window.scale_factor().unwrap_or(1.0);
    let _ = set_collapsed_region(&window, false, false);
    // ponytail: ball_pos is the only truth; never re-read outer_position here.
    let anchor = *state.ball_pos.lock().unwrap();
    let open_up = open_up_for(&window, anchor, scale);
    // ponytail: move before resize. Resizing first paints one frame of the
    // full-size panel at the ball's origin, which reads as a flash.
    if let Some((bx, by)) = anchor {
        let right = bx + (COL_W * scale) as i32;
        let new_x = right - (PANEL_W * scale) as i32;
        let new_y = if open_up {
            (by + (COL_H * scale) as i32) - (PANEL_H * scale) as i32
        } else {
            by
        };
        let (nx, ny) = clamp_on_screen(&window, new_x, new_y, scale);
        *state.panel_delta.lock().unwrap() = Some((bx - nx, by - ny));
        move_self(&window, &state, nx, ny);
    }
    let _ = window.set_size(tauri::LogicalSize::new(PANEL_W, PANEL_H));
    let _ = window.set_focus();
    open_up
}

#[tauri::command]
fn collapse_window(window: tauri::WebviewWindow, state: tauri::State<AppState>) {
    *state.panel_delta.lock().unwrap() = None;
    let anchor = *state.ball_pos.lock().unwrap();
    if let Some((bx, by)) = anchor {
        move_self(&window, &state, bx, by);
    }
    let _ = window.set_size(tauri::LogicalSize::new(COL_W, COL_H));
    let show_badge = state
        .tasks
        .lock()
        .unwrap()
        .iter()
        .any(|task| task.state == SessionState::Waiting);
    let _ = set_collapsed_region(&window, true, show_badge);
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
            let app_state = app.state::<AppState>();
            let show_badge = {
                let mut tasks = app_state.tasks.lock().unwrap();
                *tasks = load_state();
                tasks.iter().any(|task| task.state == SessionState::Waiting)
            };
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_size(tauri::LogicalSize::new(COL_W, COL_H));
                let start = load_pos().unwrap_or_else(|| startup_pos(&win));
                move_self(&win, &app.state::<AppState>(), start.0, start.1);
                *app.state::<AppState>().ball_pos.lock().unwrap() = Some(start);
                let _ = set_collapsed_region(&win, true, show_badge);
                let _ = win.show();
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                loop {
                    let (changed, alert, show_badge) = poll(&handle.state::<AppState>());
                    if changed {
                        if let Some(win) = handle.get_webview_window("main") {
                            let scale = win.scale_factor().unwrap_or(1.0);
                            let collapsed = win
                                .inner_size()
                                .map(|size| size.width as f64 <= (COL_W + 1.0) * scale)
                                .unwrap_or(false);
                            if collapsed {
                                let _ = set_collapsed_region(&win, true, show_badge);
                            }
                            let _ = win.emit("tasks-updated", alert);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1500));
                }
            });
            let show_i = MenuItem::with_id(app, "show", "显示气泡球", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出 bubble", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("bubble")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_ball(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_ball(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            open_task,
            dismiss_task,
            clear_all,
            peek_open_up,
            expand_window,
            collapse_window,
            quit_app
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Moved(pos) = event {
                let state = window.state::<AppState>();
                if *state.moving_self.lock().unwrap() {
                    return;
                }
                let scale = window.scale_factor().unwrap_or(1.0);
                let expanded = window
                    .inner_size()
                    .map(|sz| (sz.width as f64) >= (COL_W + 24.0) * scale)
                    .unwrap_or(false);
                // Dragging the expanded panel must still move the ball: reuse the
                // exact delta expand_window applied, clamp included. Without a
                // recorded delta this move is not ours to interpret -- ignore it
                // rather than write a panel corner into ball_pos.
                let (bx, by) = if expanded {
                    match *state.panel_delta.lock().unwrap() {
                        Some((dx, dy)) => (pos.x + dx, pos.y + dy),
                        None => return,
                    }
                } else {
                    (pos.x, pos.y)
                };
                save_pos(bx, by);
                *state.ball_pos.lock().unwrap() = Some((bx, by));
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod window_region_tests {
    use super::collapsed_region;

    #[test]
    fn collapsed_region_starts_at_the_visible_ball() {
        assert_eq!(collapsed_region(1.0), (73, 0, 136, 72));
        assert_eq!(collapsed_region(1.5), (110, 0, 204, 108));
    }
}
