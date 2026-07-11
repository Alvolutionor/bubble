const { invoke } = window.__TAURI__.core;
const { getCurrentWindow } = window.__TAURI__.window;
const { listen } = window.__TAURI__.event;
const appWindow = getCurrentWindow();

let tasks = [];
let expanded = false;

const LANG = (navigator.language || "en").toLowerCase().startsWith("zh") ? "zh" : "en";
const T = {
  zh: {
    tip: "AI 任务 · 点开看待处理",
    allDone: "全部处理完了",
    waiting: (n) => `${n} 个在等你`,
    empty: "🎉 没有等你的任务",
    clear: "清空",
    quit: "退出",
    dismiss: "划掉",
    times: (n) => `  ·  ${n} 次`,
    just: "刚刚",
    min: (n) => `${n} 分钟前`,
    hour: (n) => `${n} 小时前`,
    day: (n) => `${n} 天前`,
  },
  en: {
    tip: "AI tasks · click to view",
    allDone: "All caught up",
    waiting: (n) => `${n} waiting`,
    empty: "🎉 Nothing waiting",
    clear: "Clear",
    quit: "Quit",
    dismiss: "Dismiss",
    times: (n) => `  ·  ×${n}`,
    just: "just now",
    min: (n) => `${n}m ago`,
    hour: (n) => `${n}h ago`,
    day: (n) => `${n}d ago`,
  },
}[LANG];

function relTime(ts) {
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return T.just;
  const m = Math.floor(s / 60);
  if (m < 60) return T.min(m);
  const h = Math.floor(m / 60);
  if (h < 24) return T.hour(h);
  return T.day(Math.floor(h / 24));
}

function render() {
  const n = tasks.length;
  const badge = document.getElementById("badge");
  badge.textContent = n > 99 ? "99+" : String(n);
  document.getElementById("ball").classList.toggle("empty", n === 0);
  document.body.classList.toggle("no-tasks", n === 0);
  document.getElementById("panel-title").textContent =
    n === 0 ? T.allDone : T.waiting(n);

  const list = document.getElementById("list");
  list.innerHTML = "";
  for (const t of tasks) {
    const li = document.createElement("li");
    li.className = "row";
    li.innerHTML =
      '<div class="info"><div class="proj"></div><div class="meta"></div></div>' +
      '<button class="x"></button>';
    li.querySelector(".proj").textContent = t.project;
    const x = li.querySelector(".x");
    x.textContent = "×";
    x.title = T.dismiss;
    li.querySelector(".meta").textContent =
      relTime(t.last_ts) + (t.count > 1 ? T.times(t.count) : "");
    li.addEventListener("click", (e) => {
      if (e.target.closest(".x")) return;
      openTask(t.id);
    });
    li.querySelector(".x").addEventListener("click", (e) => {
      e.stopPropagation();
      dismiss(t.id);
    });
    list.appendChild(li);
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
async function dismiss(id) {
  tasks = await invoke("dismiss_task", { id });
  render();
}
async function clearAll() {
  tasks = await invoke("clear_all");
  render();
}

async function expand() {
  if (expanded) return;
  expanded = true;
  const openUp = await invoke("expand_window");
  document.body.classList.toggle("open-up", !!openUp);
  document.body.classList.add("expanded");
  refresh();
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
  document.getElementById("clear").textContent = T.clear;
  document.getElementById("quit").textContent = T.quit;
  document.getElementById("empty").textContent = T.empty;
  document.getElementById("panel-title").textContent = T.allDone;
  document.getElementById("ball").title = T.tip;
  wireBall();
  document.getElementById("clear").addEventListener("click", clearAll);
  document.getElementById("quit").addEventListener("click", () => invoke("quit_app"));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") collapse();
  });
  appWindow.onFocusChanged(({ payload: focused }) => {
    if (!focused) collapse();
  });
  refresh();
  listen("tasks-updated", () => {
    refresh();
    pulse();
  });
  listen("collapsed", () => {
    expanded = false;
    document.body.classList.remove("expanded");
    document.body.classList.remove("open-up");
  });
  setInterval(render, 30000);
});
