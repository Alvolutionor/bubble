const assert = require("assert");
const fs = require("fs");
const path = require("path");

const src = fs.readFileSync(path.join(__dirname, "main.js"), "utf8");

const defined = new Set([
  ...[...src.matchAll(/(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g)].map((m) => m[1]),
  // arrow functions bound to a const/let, e.g. `const cleanup = () => {...}`
  ...[...src.matchAll(/(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?\(/g)].map(
    (m) => m[1]
  ),
]);

const BUILTIN_OR_IMPORTED = new Set([
  "invoke", "getCurrentWindow", "listen", "require", "String", "Number",
  "Math", "Date", "JSON", "Promise", "Array", "Object", "Map", "setTimeout",
  "setInterval", "clearTimeout", "requestAnimationFrame", "parseInt",
  "parseFloat", "isNaN", "assert", "if", "for", "while", "switch", "catch",
  "return", "typeof", "void", "new", "await", "of", "in",
]);

const called = new Set(
  [...src.matchAll(/(?<![.\w$])([A-Za-z_$][\w$]*)\s*\(/g)].map((m) => m[1])
);

const missing = [...called].filter(
  (n) => !defined.has(n) && !BUILTIN_OR_IMPORTED.has(n)
);

assert.deepStrictEqual(
  missing,
  [],
  `main.js calls function(s) that are never defined: ${missing.join(", ")}`
);

for (const required of ["expand", "collapse", "toggle", "render", "refresh"]) {
  assert.ok(defined.has(required), `main.js is missing ${required}()`);
}

console.log(`main.js: ${defined.size} functions defined, no dangling calls`);
