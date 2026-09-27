const assert = require("assert");
const EventEmitter = require("events");
const fs = require("fs");
const Module = require("module");
const net = require("net");
const os = require("os");
const path = require("path");

const home = fs.mkdtempSync(path.join(os.tmpdir(), "bubble-ext-"));
process.env.USERPROFILE = process.env.HOME = home;
const bubble = path.join(home, ".claude", "bubble");
const request = path.join(bubble, "open.json");
const workspace = path.join(home, "Proj");
const opened = [];
const subscriptions = [];

const vscode = {
  window: { state: { focused: false }, onDidChangeWindowState: () => ({ dispose() {} }) },
  workspace: { workspaceFolders: [{ uri: { fsPath: workspace } }] },
  commands: { executeCommand: (cmd, id) => opened.push([cmd, id]) },
};
const load = Module._load;
Module._load = (name, ...rest) => (name === "vscode" ? vscode : load(name, ...rest));
net.connect = () => Object.assign(new EventEmitter(), { write() {}, destroy() {} });

function ask(cwd, ts = Date.now()) {
  fs.writeFileSync(request + ".tmp", JSON.stringify({ cwd, session_id: "s1", ts }));
  fs.renameSync(request + ".tmp", request);
  return new Promise((r) => setTimeout(r, 300));
}

(async () => {
  fs.mkdirSync(bubble, { recursive: true });
  fs.writeFileSync(request, JSON.stringify({ cwd: workspace, session_id: "s0", ts: Date.now() }));
  require("./extension.js").activate({ logUri: { fsPath: path.join(home, "logs", "x") }, subscriptions });
  assert.deepStrictEqual(opened, [["claude-vscode.editor.open", "s0"]], "a fresh request left before activation opens (window started by code -n)");
  assert.ok(!fs.existsSync(request), "the handling window claims the request");

  await ask(workspace + "2");
  assert.strictEqual(opened.length, 1, "a sibling folder sharing the name prefix is another window's");
  assert.ok(fs.existsSync(request), "requests for other windows are left alone");

  await ask(workspace, Date.now() - 60000);
  assert.strictEqual(opened.length, 1, "stale requests never replay");

  const sub = path.join(workspace, "sub");
  await ask(process.platform === "win32" ? sub.toLowerCase() : sub);
  assert.deepStrictEqual(opened[1], ["claude-vscode.editor.open", "s1"], "a session in a subfolder of the workspace opens here");
  assert.ok(!fs.existsSync(request));

  subscriptions.forEach((s) => s.dispose());
  fs.rmSync(home, { recursive: true, force: true });
  console.log("ok");
})();
