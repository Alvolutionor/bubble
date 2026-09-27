const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { execFileSync } = require("child_process");

const home = fs.mkdtempSync(path.join(os.tmpdir(), "bubble-hook-"));
const inbox = path.join(home, ".claude", "bubble", "inbox");
const transcript = path.join(home, "t.jsonl");
fs.writeFileSync(transcript, "{}\n");

function run(payload) {
  fs.rmSync(inbox, { recursive: true, force: true });
  execFileSync(process.execPath, [path.join(__dirname, "bubble-hook.js")], {
    input: JSON.stringify(payload),
    env: { ...process.env, USERPROFILE: home, HOME: home },
  });
  const files = fs.existsSync(inbox) ? fs.readdirSync(inbox) : [];
  assert.ok(files.length <= 1);
  return files.map((f) => JSON.parse(fs.readFileSync(path.join(inbox, f), "utf8")))[0];
}

const base = { session_id: "s1", cwd: "/work/proj", transcript_path: transcript };

const start = run({ ...base, hook_event_name: "UserPromptSubmit", prompt: "  fix it\nplease" });
assert.strictEqual(start.kind, "start");
assert.strictEqual(start.prompt, "fix it");
assert.strictEqual(start.mark, 3);
assert.strictEqual(start.session_id, "s1");
assert.strictEqual(start.project, "proj");

assert.strictEqual(run({ ...base, hook_event_name: "Stop" }).kind, "stop");
const notified = run({ ...base, hook_event_name: "UserPromptSubmit", prompt: "<task-notification>\n<task-id>a</task-id>" });
assert.strictEqual(notified.prompt, "");
const perm = run({ ...base, hook_event_name: "Notification", message: "Claude needs your permission to use Bash" });
assert.strictEqual(perm.kind, "notify");
assert.strictEqual(perm.note, "Claude needs your permission to use Bash");
assert.strictEqual(run({ ...base, hook_event_name: "SessionEnd" }).kind, "end");

assert.strictEqual(run({ ...base, hook_event_name: "Notification", message: "Claude is waiting for your input" }), undefined);
assert.strictEqual(run({ ...base, hook_event_name: "Notification", notification_type: "idle_prompt" }), undefined);
assert.strictEqual(run({ ...base, hook_event_name: "PreToolUse" }), undefined);

fs.rmSync(home, { recursive: true, force: true });
console.log("bubble-hook: event kinds ok");
