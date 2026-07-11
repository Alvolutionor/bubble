const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawn } = require("child_process");

const REAL = "C:\\Users\\MSI_NB\\AppData\\Local\\OpenAI\\Codex\\runtimes\\cua_node\\1b23c930bdf84ed6\\bin\\node_modules\\@oai\\sky\\bin\\windows\\codex-computer-use.exe";
const REAL_PREFIX = ["turn-ended"];

const passthrough = process.argv.slice(2);

try {
  if (fs.existsSync(REAL)) {
    spawn(REAL, [...REAL_PREFIX, ...passthrough], { detached: true, stdio: "ignore" }).unref();
  }
} catch {}

try {
  let cwd = process.cwd();
  const payload = passthrough[passthrough.length - 1];
  if (payload) {
    try {
      const d = JSON.parse(payload);
      if (d && typeof d.cwd === "string") cwd = d.cwd;
    } catch {}
  }
  const rec = { cwd, project: path.basename(cwd), session_id: "codex", ts: Date.now() };
  const dir = path.join(os.homedir(), ".claude", "bubble", "inbox");
  fs.mkdirSync(dir, { recursive: true });
  const name = rec.ts + "-" + Math.random().toString(36).slice(2, 10) + ".json";
  fs.writeFileSync(path.join(dir, name), JSON.stringify(rec));
} catch {}
