# bubble 🫧

A tiny always-on-top desktop **floating ball** that collects your finished AI/agent
tasks and lets you jump straight back to the right editor window.

When an AI coding session finishes a turn, a task is pushed onto the ball. Glance at
the ball to see how many are waiting, click to see them grouped by project, and click
one to **switch to that project's VS Code window** — then it's crossed off. When
nothing is waiting, the ball hides itself completely.

<p align="center"><img src="assets/demo.png" alt="bubble floating over the editor with a waiting task" width="380"></p>

---

## Why

A finish sound tells you *something* is done, but not **which** of your projects now
needs you. bubble turns "an agent stopped" into a glanceable, clickable queue: the
badge is your count, and one click takes you to the window that's waiting.

## How it works

```
  AI session finishes a turn
            │  (a hook runs a command)
            ▼
  hook/bubble-hook.js  ──writes one JSON file──▶  ~/.claude/bubble/inbox/
            │                                              │
       (also beeps, if you keep the beep)          ball polls every 1.5s
                                                           ▼
                                          the ball appears, badge +1
                                          click → panel grouped by project
                                          click a project → focus its editor window
                                          task removed; 0 tasks → ball hides
```

Three decoupled pieces, each with one job:

- **Collector** — `hook/bubble-hook.js`, a small Node script run by your tool's
  "task finished" hook. It writes one JSON file per event into
  `~/.claude/bubble/inbox/` (one file per event = zero write contention, survives the
  ball being closed).
- **Store** — `~/.claude/bubble/`: an `inbox/` the ball drains, plus a `state.json`
  the ball owns and a `pos.json` remembering where you dragged the ball.
- **Ball** — a [Tauri](https://tauri.app) app (Rust + a system WebView, ~30&nbsp;MB
  RAM). Polls the inbox, shows/hides itself, and switches editor windows.

## Features

- **Appears only when needed** — hidden at 0 tasks, pops up (with a pulse) on a new one.
- **Grouped by project** — repeated finishes in the same folder collapse to one row with a count.
- **One-click window switching** — jumps to the editor window for that project.
- **Draggable & remembered** — drag it anywhere; position persists across launches.
- **Opens toward the screen center** — panel expands up or down so it never falls off an edge; the ball itself never moves.
- **Light** — system WebView, no bundled Chromium.
- **Single instance** — a second launch just exits.
- **Bilingual UI** — English / 中文, auto-detected from your system language.

## Platform support

| Platform | Status |
| --- | --- |
| **Windows** | ✅ Fully working & tested. Precise window focus via a Win32 helper (`EnumWindows` + foreground-lock bypass). |
| **macOS / Linux** | ⚙️ Builds and runs. Window switching falls back to `code <path>`, which opens/focuses the folder but may reuse a window instead of raising the exact one. A native focus-by-title helper (AppleScript / Accessibility API on macOS, `wmctrl`/similar on Linux) is **welcome as a PR** — see [`focus_or_open` in lib.rs](bubble/src-tauri/src/lib.rs). |

## Build

Prereqs: [Rust](https://rustup.rs) ≥ 1.85, Node ≥ 18, and the `code` CLI on your PATH.
On Windows the WebView2 runtime ships with Windows 11.

```bash
cd bubble/src-tauri
cargo build --release
# → target/release/bubble  (bubble.exe on Windows)
```

Or the Tauri dev/bundler flow: `cd bubble && npm install && npm run tauri dev`.

Run the binary and it sits quietly (hidden) until a task arrives. To auto-start it,
add it to your OS login items (Windows: `shell:startup`; macOS: Login Items /
LaunchAgent; Linux: your desktop's autostart).

## Wiring it to your AI tool

bubble is driven by a hook that runs a command when a session finishes.

> **Easiest way — just ask your AI to wire it up.** Since you're already using an AI
> coding agent, hand it this task. In Claude Code, say:
>
> > *Add a `Stop` hook to my `~/.claude/settings.json` that runs
> > `node "<absolute-path-to>/hook/bubble-hook.js"`, keeping any existing hooks. Then
> > start `bubble` (the built binary).*
>
> It'll edit the settings, verify the JSON, and can launch the app for you. The manual
> steps below are the fallback if you'd rather do it by hand.

For **Claude Code**, the hook to add to `~/.claude/settings.json` is:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          { "type": "command", "command": "node \"/ABSOLUTE/PATH/TO/hook/bubble-hook.js\"" }
        ]
      }
    ]
  }
}
```

Keep (or add) your finish sound as a second command in the same array — the beep is
independent of bubble:

- Windows: `rundll32 user32.dll,MessageBeep -1`
- macOS: `afplay /System/Library/Sounds/Glass.aiff`
- Linux: `paplay /usr/share/sounds/freedesktop/stereo/complete.oga`

### Feeding bubble from anything else

The hook is just a JSON writer. Any tool can feed the ball by dropping a file into
`~/.claude/bubble/inbox/` (filename can be anything ending in `.json`):

```json
{ "cwd": "/path/to/project", "project": "myproject", "session_id": "abc", "ts": 1700000000000 }
```

`project` defaults to the basename of `cwd`; `ts` defaults to now.

## Usage

- **Drag** the ball to move it (position is remembered).
- **Single click** the ball → open the panel; click again / `Esc` / click elsewhere → close.
- **Click a project row** → switch to its editor window and remove it.
- **×** on a row dismisses it; the panel footer has **Clear all** and **Quit**.

## Architecture notes & deliberate trade-offs

- Tasks are keyed by `cwd`, so parallel agents in the *same* folder collapse into one row.
- Window switching matches by title (`<project> - Visual Studio Code`); two projects with the same folder name could match the wrong window.
- The ball polls the inbox every 1.5s (no filesystem watcher).
- The list shows project + time, not a summary of what the agent did.
- On Windows the collapsed window is 136px wide (OS minimum for a captioned window); the extra area is transparent and clicks pass through it.

## Contributing

Issues and PRs welcome. The most impactful contribution right now is **native
window-focus for macOS and Linux** (see Platform support above). Please keep the
"one file, one job" structure and match the existing style.

## License

[MIT](LICENSE)
