const fs = require("fs");
const os = require("os");
const path = require("path");

let done = false;

const KINDS = {
  UserPromptSubmit: "start",
  Stop: "stop",
  Notification: "notify",
  SessionEnd: "end",
};

function originFromTranscript(tp) {
  if (!tp) return "";
  const slug = path.basename(path.dirname(tp));
  const m = /^([a-zA-Z])--(.*)$/.exec(slug);
  if (!m) return "";
  // ponytail: a path containing its own "<drive>--" lookalike segment (only seen in
  // Claude's scratchpad temp dirs) is ambiguous and resolves to "", falling back to
  // data.cwd. Add a manifest lookup if real projects ever hit it.
  const root = m[1].toUpperCase() + ":" + path.sep;
  // The slug is the absolute path with every non-alphanumeric run replaced by "-",
  // so it is lossy ("__PROJECTS" and "-PROJECTS" both collapse) and a single greedy
  // walk can take a wrong branch it cannot undo. Search with backtracking instead.
  const walk = (dir, rest0) => {
    const rest = rest0.replace(/^-+/, "");
    if (!rest) return dir;
    let names;
    try {
      names = fs
        .readdirSync(dir, { withFileTypes: true })
        .filter((d) => d.isDirectory())
        .map((d) => d.name);
    } catch {
      return "";
    }
    const cands = [];
    const low = rest.toLowerCase();
    for (const n of names) {
      // The separator and any leading punctuation of the child collapse into the
      // SAME dash ("Desktop" + sep + "__PROJECTS" -> "Desktop-PROJECTS"), so match
      // the child with its own leading dashes stripped.
      const t = n.replace(/[^a-zA-Z0-9]+/g, "-").replace(/^-+/, "").toLowerCase();
      if (!t) continue;
      if (low === t) cands.push([n, ""]);
      else if (low.startsWith(t + "-")) cands.push([n, rest.slice(t.length + 1)]);
    }
    cands.sort((a, b) => a[1].length - b[1].length);
    for (const [name, next] of cands) {
      const hit = walk(path.join(dir, name), next);
      if (hit) return hit;
    }
    return "";
  };
  return walk(root, m[2]);
}

function push(input) {
  if (done) return;
  done = true;
  let data = {};
  try {
    data = JSON.parse(input || "{}");
  } catch {}
  const kind = KINDS[data.hook_event_name];
  const idle = data.notification_type === "idle_prompt" || /waiting for your input/i.test(data.message || "");
  if (!kind || idle) process.exit(0);
  // The session's ORIGIN folder, not the live cwd. cwd drifts whenever the AI is
  // sent to work on another folder, which would point the row -- and the click --
  // at a window the conversation never started in. ~/.claude/projects/<slug>/<id>.jsonl
  // keeps the origin path encoded in <slug>, so recover it from there.
  const cwd = originFromTranscript(data.transcript_path) || data.cwd || process.cwd();
  let mark = 0;
  try {
    mark = fs.statSync(data.transcript_path).size;
  } catch {}
  const rec = {
    cwd,
    project: path.basename(cwd),
    session_id: data.session_id || "",
    ts: Date.now(),
    kind,
    note: data.message || "",
    prompt: /^\s*</.test(data.prompt || "") ? "" : (data.prompt || "").trim().split(/\r?\n/)[0].slice(0, 120),
    transcript: data.transcript_path || "",
    mark,
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
