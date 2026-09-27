const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;
const appWindow = getCurrentWindow();

let tasks = [];
let expanded = false;
let dragInFlight = false;

const STRINGS = {
  zh: {
    tip: "AI 会话 · 点开查看和切换",
    allDone: "全部处理完了",
    summary: (w, r) => `${w} 个等你 · ${r} 个最近`,
    waiting: "等你",
    recent: "最近 24 小时 · 窗口已打开",
    running: "运行中",
    runningN: (n) => `${n} 个运行中`,
    empty: "🎉 没有等你或最近的会话",
    clear: "清空",
    quit: "退出",
    dismiss: "划掉",
    just: "刚刚",
    min: (n) => `${n} 分钟前`,
    hour: (n) => `${n} 小时前`,
    day: (n) => `${n} 天前`,
  },
  en: {
    tip: "AI sessions · click to view and switch",
    allDone: "All caught up",
    summary: (w, r) => `${w} waiting · ${r} recent`,
    waiting: "Waiting for you",
    recent: "Last 24h · window open",
    running: "running",
    runningN: (n) => `${n} running`,
    empty: "🎉 Nothing waiting or recent",
    clear: "Clear",
    quit: "Quit",
    dismiss: "Dismiss",
    just: "just now",
    min: (n) => `${n}m ago`,
    hour: (n) => `${n}h ago`,
    day: (n) => `${n}d ago`,
  },
};

let lang = localStorage.getItem("bubble-lang");
if (lang !== "zh" && lang !== "en") {
  lang = (navigator.language || "en").toLowerCase().startsWith("zh") ? "zh" : "en";
}
let T = STRINGS[lang];

function applyStrings() {
  T = STRINGS[lang];
  document.getElementById("clear").textContent = T.clear;
  document.getElementById("quit").textContent = T.quit;
  document.getElementById("empty").textContent = T.empty;
  document.getElementById("ball").title = T.tip;
  const lt = document.getElementById("lang");
  if (lt) lt.textContent = lang === "zh" ? "EN" : "中";
  render();
}

function setLang(l) {
  lang = l;
  localStorage.setItem("bubble-lang", lang);
  applyStrings();
}

function relTime(ts) {
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return T.just;
  const m = Math.floor(s / 60);
  if (m < 60) return T.min(m);
  const h = Math.floor(m / 60);
  if (h < 24) return T.hour(h);
  return T.day(Math.floor(h / 24));
}

function byProject(rows) {
  const groups = new Map();
  for (const t of rows) {
    const g = groups.get(t.project) || { ids: [], sessions: [] };
    g.ids.push(t.id);
    g.sessions.push(t);
    groups.set(t.project, g);
  }
  return [...groups.values()].map(({ ids, sessions }) => {
    const live = sessions.filter((t) => t.state === "running");
    return { ...(live[0] || sessions[0]), ids, liveCount: live.length };
  });
}

function renderRow(t) {
  const li = document.createElement("li");
  const running = t.state === "running";
  li.className = running ? "row running" : "row";
  li.innerHTML =
    '<div class="info"><div class="proj"></div><div class="note"></div>' +
    '<div class="meta"></div></div><button class="x"></button>';
  li.querySelector(".proj").textContent = t.project;
  const note = li.querySelector(".note");
  const text = (t.state === "waiting" && t.note) || t.prompt || "";
  note.textContent = text;
  note.style.display = text ? "" : "none";
  const x = li.querySelector(".x");
  x.textContent = "×";
  x.title = T.dismiss;
  x.style.visibility = running ? "hidden" : "";
  li.querySelector(".meta").textContent =
    t.liveCount > 1 ? T.runningN(t.liveCount) : running ? T.running : relTime(t.last_ts);
  li.addEventListener("click", (e) => {
    if (e.target.closest(".x")) return;
    openTask(t.id);
  });
  x.addEventListener("click", (e) => {
    e.stopPropagation();
    dismiss(t.ids || [t.id]);
  });
  return li;
}

function render() {
  const waiting = tasks.filter((t) => t.state === "waiting");
  const recent = byProject(tasks.filter((t) => t.state !== "waiting"));
  const w = waiting.length;
  document.getElementById("badge").textContent = w > 99 ? "99+" : String(w);
  const ball = document.getElementById("ball");
  ball.classList.toggle("empty", w === 0);
  ball.classList.toggle("busy", tasks.some((t) => t.state === "running"));
  document.body.classList.toggle("no-tasks", tasks.length === 0);
  document.getElementById("panel-title").textContent =
    tasks.length === 0 ? T.allDone : T.summary(w, recent.length);

  const list = document.getElementById("list");
  list.innerHTML = "";
  for (const [label, rows] of [[T.waiting, waiting], [T.recent, recent]]) {
    if (!rows.length) continue;
    const head = document.createElement("li");
    head.className = "section";
    head.textContent = `${label} · ${rows.length}`;
    list.appendChild(head);
    for (const t of rows) list.appendChild(renderRow(t));
  }
}

async function refresh() {
  tasks = await invoke("list_tasks");
  render();
}
async function openTask(id) {
  tasks = await invoke("open_task", { id });
  render();
  collapse();
}
async function dismiss(ids) {
  for (const id of ids) tasks = await invoke("dismiss_task", { id });
  render();
}
async function clearAll() {
  tasks = await invoke("clear_all");
  render();
}

async function expand() {
  if (expanded) return;
  expanded = true;
  // Resolve direction and fill the list BEFORE the window changes size, so the
  // panel's first paint and the resize land together. Doing it the other way
  // round painted the relocated ball one frame ahead of the panel, which read
  // as the ball being shoved sideways.
  const openUp = await invoke("peek_open_up");
  tasks = await invoke("list_tasks");
  document.body.classList.toggle("open-up", !!openUp);
  document.body.classList.add("expanded");
  render();
  await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
  if (!expanded) return;
  await invoke("expand_window");
}
async function collapse() {
  if (!expanded) return;
  expanded = false;
  document.body.classList.remove("expanded");
  document.body.classList.remove("open-up");
  await invoke("collapse_window");
}
function toggle() {
  return expanded ? collapse() : expand();
}

function pulse() {
  const b = document.getElementById("ball");
  b.classList.remove("pulse");
  void b.offsetWidth;
  b.classList.add("pulse");
}

// Click vs drag: a press released in place is a click -> toggle; moving past a
// small threshold starts a native window drag instead.
function wireBall() {
  const ball = document.getElementById("ball");
  ball.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    const sx = e.screenX;
    const sy = e.screenY;
    let dragging = false;
    const cleanup = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    const move = (ev) => {
      if (!dragging && (Math.abs(ev.screenX - sx) > 4 || Math.abs(ev.screenY - sy) > 4)) {
        dragging = true;
        dragInFlight = true;
        setTimeout(() => { dragInFlight = false; }, 3000);
        cleanup();
        appWindow.startDragging();
      }
    };
    const up = () => {
      cleanup();
      if (!dragging) toggle();
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  });
}

window.addEventListener("DOMContentLoaded", () => {
  applyStrings();
  document.getElementById("lang").addEventListener("click", () => setLang(lang === "zh" ? "en" : "zh"));
  wireBall();
  document.getElementById("clear").addEventListener("click", clearAll);
  document.getElementById("quit").addEventListener("click", () => invoke("quit_app"));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") collapse();
  });
  appWindow.onFocusChanged(({ payload: focused }) => {
    // ponytail: startDragging drops focus for the duration of the OS drag loop;
    // collapsing there would fight the drag and desync size from state.
    if (focused) {
      dragInFlight = false;
      return;
    }
    if (dragInFlight) return;
    collapse();
  });
  refresh();
  listen("tasks-updated", ({ payload: notify }) => {
    refresh();
    if (notify !== false) pulse();
  });
  listen("collapsed", () => {
    expanded = false;
    document.body.classList.remove("expanded");
    document.body.classList.remove("open-up");
  });
  setInterval(render, 30000);
});
