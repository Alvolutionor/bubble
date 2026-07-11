const fs = require("fs");
const os = require("os");
const path = require("path");

let done = false;

function push(input) {
  if (done) return;
  done = true;
  let data = {};
  try {
    data = JSON.parse(input || "{}");
  } catch {}
  const cwd = data.cwd || process.cwd();
  const rec = {
    cwd,
    project: path.basename(cwd),
    session_id: data.session_id || "",
    ts: Date.now(),
  };
  try {
    const dir = path.join(os.homedir(), ".claude", "bubble", "inbox");
    fs.mkdirSync(dir, { recursive: true });
    const name = rec.ts + "-" + Math.random().toString(36).slice(2, 10) + ".json";
    fs.writeFileSync(path.join(dir, name), JSON.stringify(rec));
  } catch {}
  process.exit(0);
}

let buf = "";
process.stdin.on("data", (d) => (buf += d));
process.stdin.on("end", () => push(buf));
setTimeout(() => push(buf), 800);
