const assert = require("assert");
const fs = require("fs");
const os = require("os");
const path = require("path");

const src = fs.readFileSync(path.join(__dirname, "bubble-hook.js"), "utf8");
eval(src.slice(src.indexOf("function originFromTranscript"), src.indexOf("function push")));

const root = path.join(os.homedir(), ".claude", "projects");
let matched = 0;
let unresolved = 0;

for (const slug of fs.readdirSync(root)) {
  const dir = path.join(root, slug);
  if (!fs.statSync(dir).isDirectory()) continue;
  const files = fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl"));
  if (!files.length) continue;
  const tp = path.join(dir, files[0]);

  let origin = "";
  for (const line of fs.readFileSync(tp, "utf8").split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      const d = JSON.parse(line);
      if (d.cwd) {
        origin = d.cwd;
        break;
      }
    } catch {}
  }
  if (!origin) continue;

  const got = originFromTranscript(tp);
  if (!got) {
    unresolved++;
    continue;
  }
  assert.strictEqual(
    got.toLowerCase(),
    origin.toLowerCase(),
    `resolved the wrong folder for ${slug}`
  );
  matched++;
}

assert.ok(matched > 0, "no transcripts resolved — the slug walk is broken");
console.log(`origin resolution: ${matched} matched, ${unresolved} fell back to cwd`);
