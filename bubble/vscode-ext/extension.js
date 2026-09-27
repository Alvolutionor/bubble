const vscode = require("vscode");
const crypto = require("crypto");
const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");

const BUBBLE = path.join(os.homedir(), ".claude", "bubble");
const INBOX = path.join(BUBBLE, "inbox");
const OPEN_REQUEST = "open.json";
const OPEN_FRESH_MS = 30000;
const HEARTBEAT_MS = 5000;
const CODEX_PIPE = "\\\\.\\pipe\\codex-ipc";
const CODEX_RETRY_MS = 10000;
const CLAUDE_MESSAGE = "Received message from webview: ";

const claudeShown = new Map();
const codexShown = new Map();
let claudeLogOffset = 0;
let focusedSince = vscode.window.state.focused ? Date.now() : 0;
let stopped = false;
let codexSock = null;

function readClaudeLog(file) {
  let size;
  try {
    size = fs.statSync(file).size;
  } catch {
    return;
  }
  if (size < claudeLogOffset) claudeLogOffset = 0;
  if (size === claudeLogOffset) return;
  const chunk = Buffer.alloc(size - claudeLogOffset);
  const fd = fs.openSync(file, "r");
  fs.readSync(fd, chunk, 0, chunk.length, claudeLogOffset);
  fs.closeSync(fd);
  const end = chunk.lastIndexOf(0x0a);
  if (end < 0) return;
  claudeLogOffset += end + 1;
  for (const line of chunk.subarray(0, end).toString("utf8").split("\n")) {
    const at = line.indexOf(CLAUDE_MESSAGE);
    if (at < 0) continue;
    let req;
    try {
      req = JSON.parse(line.slice(at + CLAUDE_MESSAGE.length)).request;
    } catch {
      continue;
    }
    if (req?.type !== "update_session_state" || !req.sessionId) continue;
    if (req.isFarewell || req.panelNoLongerHosts) claudeShown.delete(req.sessionId);
    else if (!claudeShown.has(req.sessionId)) claudeShown.set(req.sessionId, Date.now());
  }
}

function frame(msg) {
  const body = Buffer.from(JSON.stringify(msg), "utf8");
  const head = Buffer.alloc(4);
  head.writeUInt32LE(body.length, 0);
  return Buffer.concat([head, body]);
}

function onCodexMessage(sock, msg) {
  if (msg.type === "client-discovery-request") {
    sock.write(frame({ type: "client-discovery-response", requestId: msg.requestId, response: { canHandle: false } }));
    return;
  }
  if (msg.type !== "broadcast") return;
  const p = msg.params ?? {};
  if (msg.method === "thread-stream-following-changed" && p.conversationId) {
    const viewers = codexShown.get(p.conversationId) ?? { clients: new Set(), since: Date.now() };
    if (p.following) viewers.clients.add(msg.sourceClientId);
    else viewers.clients.delete(msg.sourceClientId);
    if (viewers.clients.size) codexShown.set(p.conversationId, viewers);
    else codexShown.delete(p.conversationId);
  }
  if (msg.method === "client-status-changed" && p.status !== "connected") {
    for (const [id, viewers] of codexShown) {
      viewers.clients.delete(p.clientId);
      if (!viewers.clients.size) codexShown.delete(id);
    }
  }
}

function watchCodex() {
  const sock = (codexSock = net.connect(CODEX_PIPE, () => {
    sock.write(frame({
      type: "request",
      requestId: crypto.randomUUID(),
      sourceClientId: "initializing-client",
      version: 0,
      method: "initialize",
      params: { clientType: "bubble-companion" },
    }));
  }));
  let buf = Buffer.alloc(0);
  sock.on("data", (data) => {
    buf = Buffer.concat([buf, data]);
    while (buf.length >= 4 && buf.length >= 4 + buf.readUInt32LE(0)) {
      const n = buf.readUInt32LE(0);
      const body = buf.subarray(4, 4 + n);
      buf = buf.subarray(4 + n);
      try {
        onCodexMessage(sock, JSON.parse(body.toString("utf8")));
      } catch {}
    }
  });
  sock.on("error", () => {});
  sock.on("close", () => {
    codexShown.clear();
    if (!stopped) setTimeout(watchCodex, CODEX_RETRY_MS);
  });
}

function heartbeat() {
  if (!focusedSince) return;
  const cwd = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? "";
  const now = Date.now();
  const shown = [...claudeShown, ...[...codexShown].map(([id, v]) => [id, v.since])];
  try {
    fs.mkdirSync(INBOX, { recursive: true });
    for (const [id, since] of shown) {
      const rec = { kind: "seen", cwd, session_id: id, since: Math.max(since, focusedSince), ts: now };
      fs.writeFileSync(path.join(INBOX, `${now}-${Math.random().toString(36).slice(2, 10)}.json`), JSON.stringify(rec));
    }
  } catch {}
}

function samePath(p) {
  const full = path.resolve(p);
  return process.platform === "win32" ? full.toLowerCase() : full;
}

function inWorkspace(cwd) {
  const dir = samePath(cwd);
  return (vscode.workspace.workspaceFolders ?? []).some((f) => {
    const root = samePath(f.uri.fsPath);
    return dir === root || dir.startsWith(root + path.sep);
  });
}

function openRequested() {
  const file = path.join(BUBBLE, OPEN_REQUEST);
  let req;
  try {
    req = JSON.parse(fs.readFileSync(file, "utf8"));
  } catch {
    return;
  }
  if (!req.session_id || !req.cwd || Date.now() - req.ts > OPEN_FRESH_MS || !inWorkspace(req.cwd)) return;
  try {
    fs.unlinkSync(file);
  } catch {
    return;
  }
  vscode.commands.executeCommand("claude-vscode.editor.open", req.session_id, undefined, undefined, undefined, undefined, {
    programmatic: "honor-preferred-location",
  });
}

function activate(context) {
  const claudeLog = path.join(path.dirname(context.logUri.fsPath), "Anthropic.claude-code", "Claude VSCode.log");
  const timer = setInterval(() => {
    readClaudeLog(claudeLog);
    heartbeat();
  }, HEARTBEAT_MS);
  if (process.platform === "win32") watchCodex();
  fs.mkdirSync(BUBBLE, { recursive: true });
  const openWatcher = fs.watch(BUBBLE, (_, name) => {
    if (name === OPEN_REQUEST) openRequested();
  });
  context.subscriptions.push(
    { dispose: () => { stopped = true; clearInterval(timer); codexSock?.destroy(); openWatcher.close(); } },
    vscode.window.onDidChangeWindowState((s) => {
      focusedSince = s.focused ? focusedSince || Date.now() : 0;
    }),
  );
  readClaudeLog(claudeLog);
  openRequested();
}

module.exports = { activate };
