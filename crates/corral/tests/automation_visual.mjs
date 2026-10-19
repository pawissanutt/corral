// Real PTY -> xterm.js owner workflow. Run inside scripts/test-herdr.sh's private environment.
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { resolve, join, dirname } from "node:path";

const binary = resolve(process.argv[2]);
const evidence = resolve(process.argv[3]);
const cols = Number(process.argv[4] ?? 120);
const rows = cols === 80 ? 24 : 42;
assert.match(dirname(process.env.HOME), /^\/tmp\/corral-herdr-/);
await mkdir(evidence, { recursive: true });
const fixture = await mkdtemp(join(evidence, "fixture-"));
await mkdir(join(fixture, "app/.git"), { recursive: true });
const env = { ...process.env, TERM: "xterm-256color" };
for (const name of ["AI_AGENT", "PI_CODING_AGENT", "CORRAL_PM"]) delete env[name];
async function cli(...args) {
  const child = Bun.spawn([binary, "--root", fixture, ...args], {
    cwd: fixture, env, stdin: "ignore", stdout: "pipe", stderr: "pipe",
  });
  const [text, error, status] = await Promise.all([
    new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited,
  ]);
  assert.equal(status, 0, error);
  return text;
}
await cli("init");
await mkdir(join(fixture, ".corral/state"), { recursive: true });
await writeFile(join(fixture, ".corral/state/tour-seen"), "fixture\n");
const id = (await cli("new", "Finite owner workflow", "--project", "app")).trim();
const preview = () => cli("automation", "preview", "--task", id, "--dispatch")
  .then(text => JSON.parse(text.split("\nExact confirmation digest:")[0]));
const [script, style] = await Promise.all([
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js").then(r => r.text()),
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css").then(r => r.text()),
]);
const font = Buffer.from(await readFile("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")).toString("base64");
const html = `<!doctype html><meta charset="utf-8"><style>${style}
@font-face{font-family:EvidenceMono;src:url(data:font/ttf;base64,${font})}
body{margin:0;padding:12px;background:#111318} .xterm-viewport{overflow:hidden!important}</style>
<div id="terminal"></div><script>${script}</script><script>
window.ready=(async()=>{await document.fonts.load("15px EvidenceMono");
window.term=new Terminal({cols:${cols},rows:${rows},fontFamily:"EvidenceMono",fontSize:15,
lineHeight:1.2,scrollback:0,theme:{background:"#111318",foreground:"#d7dce2"}});
term.open(document.getElementById("terminal"));
window.replies=[];term.onData(data=>window.replies.push(data));})();</script>`;
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(html, { headers: { "content-type": "text/html" } }) });
const browser = new Bun.WebView({ width: 1500, height: 1000 });
let processPty;
let ptyGeneration = 0;
let stream = "";
let queue = Promise.resolve();
let evaluations = Promise.resolve();
let pending;
const scenes = [];
async function evaluate(source) {
  const previous = evaluations;
  let release;
  evaluations = new Promise(resolve => { release = resolve; });
  await previous;
  try {
    return await browser.evaluate(source);
  } finally {
    release();
  }
}
async function screen() {
  return evaluate(`Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(term.buffer.active.viewportY+i)?.translateToString(true)??"").join("\\n")`);
}
async function capture(name) {
  await queue;
  await evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
  const text = await screen();
  const measuredWidths = text.split("\n").map(line => Bun.stringWidth(line));
  const png = Buffer.from(await (await browser.screenshot()).arrayBuffer());
  assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
  await writeFile(join(evidence, `${name}.png`), png);
  await writeFile(join(evidence, `${name}.txt`), text);
  await writeFile(join(evidence, `${name}.ansi`), stream);
  scenes.push({ name, cols, rows, maxUnicodeWidth: Math.max(...measuredWidths),
    overflowLines: measuredWidths.flatMap((width, index) => width > cols ? [{ line: index + 1, width }] : []),
    renderer: "actual PTY / xterm.js 5.5.0" });
  return text;
}
function observe(check, trigger) {
  assert(!pending);
  return new Promise((resolveObserved, reject) => {
    const timer = setTimeout(() => { pending = undefined; reject(new Error(`Expected rendered state not observed: ${check}`)); }, 15000);
    pending = { check, resolve(text) { clearTimeout(timer); pending = undefined; resolveObserved(text); } };
    trigger();
  });
}
async function start(args, extra = {}) {
  const generation = ++ptyGeneration;
  await queue;
  stream = "";
  await evaluate("term.reset()");
  // script allocates the child controlling PTY after setsid; never steal an existing terminal.
  const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
  const command = [binary, "--root", fixture, ...args].map(quote).join(" ");
  const decoder = new TextDecoder();
  processPty = Bun.spawn(["script", "--quiet", "--return", "--command", command, "/dev/null"], {
    cwd: fixture, env: { ...env, ...extra }, terminal: { cols, rows,
      data(terminal, data) {
        if (generation !== ptyGeneration) return;
        const chunk = decoder.decode(data, { stream: true });
        stream += chunk;
        queue = queue.then(async () => {
          if (generation !== ptyGeneration) return;
          await evaluate(`new Promise(resolve=>term.write(${JSON.stringify(chunk).replaceAll("<", "\\u003c")},resolve))`);
          const replies = await evaluate("window.replies.splice(0)");
          if (generation !== ptyGeneration) return;
          for (const reply of replies) terminal.write(reply);
          const text = await screen();
          if (generation === ptyGeneration && pending?.check(text)) pending.resolve(text);
        });
      },
    },
  });
}
const input = text => processPty.terminal.write(text);
const selectedRow = text => {
  const line = text.split("\n").find(line => line.lastIndexOf("▸ ") > 3);
  return line?.slice(line.lastIndexOf("▸ ")).split(/ {2,}/)[0] ?? "";
};
async function selectRow(label, direction = "\x1b[B") {
  for (let moves = 0; moves < 80; moves++) {
    const before = selectedRow(await screen());
    if (before.includes(label)) return;
    await observe(text => {
      const after = selectedRow(text);
      return after !== "" && after !== before;
    }, () => input(direction));
  }
  throw new Error(`Settings row was not reached: ${label}`);
}
try {
  await browser.navigate(`http://127.0.0.1:${server.port}`);
  await evaluate("window.ready");
  const cell = await evaluate("term._core._renderService.dimensions.css.cell");
  await browser.cdp("Emulation.setDeviceMetricsOverride", {
    width: Math.ceil(cell.width * cols) + 24, height: Math.ceil(cell.height * rows) + 24,
    deviceScaleFactor: 1, mobile: false,
  });
  await observe(text => text.includes("autopilot: blocked (assist)"), () => start(["board"]));
  await capture("board-assist-status");
  input("\x03");
  assert.equal(await processPty.exited, 0);
  const first = await preview();
  await observe(text => text.includes("Type the full digest"), () => start([
    "automation", "confirm", "--task", id, "--dispatch", "--digest", first.id,
  ]));
  await capture("cli-owner-prompt");
  await observe(text => text.includes(`Confirmed ${first.id}`), () => input(first.id + "\r"));
  assert.equal(await processPty.exited, 0);
  await capture("cli-owner-confirmed");
  let state = JSON.parse(await readFile(join(fixture, ".corral/state/automation.json"), "utf8"));
  assert.equal(state.approval.authorization, first.id);
  assert.equal(state.approval.digest, first.grant_digest);

  await observe(text => text.includes("settings") && text.includes("sections"), () => start(["settings"]));
  await observe(text => text.includes("▸ Review"), () => input("6"));
  await selectRow("Preview finite grant");
  await selectRow("Grant tasks", "\x1b[A");
  await observe(text => text.includes("Grant tasks") && text.includes("› ▏"), () => input("\r"));
  await observe(text => text.includes(`› ${id}▏`), () => input(id));
  await observe(text => text.includes("Grant tasks") && text.includes(id) && !text.includes("› "), () => input("\r"));
  await selectRow("Delegate dispatch");
  await observe(text => text.split("\n").some(line => line.includes("Delegate dispatch") && line.includes("[on]")), () => input("\r"));
  await selectRow("Preview finite grant");
  await capture("tui-owner-selection");
  const second = await preview();
  await observe(text => text.includes("Confirmation digest") && text.includes(second.id), () => input("\r"));
  let text = await capture("tui-exact-preview");
  assert(text.includes(second.id) && text.includes(second.grant_digest));
  for (let pages = 0; !(await screen()).includes("End of finite scope"); pages++) {
    assert(pages < 40, "Finite scope end was not reached");
    const before = await screen();
    await observe(text => text !== before, () => input("\x1b[6~"));
  }
  await capture("tui-preview-end");
  await observe(text => text.includes("▸ Preview finite grant"), () => input("\x1b"));
  await capture("tui-preview-cancelled");
  await observe(text => text.includes("Confirmation digest") && text.includes(second.id), () => input("\r"));
  await observe(text => text.includes("confirmation digest does not match"), () => input("\r"));
  await capture("tui-empty-digest-refused");
  await observe(text => text.replace(/[\s│]/g, "").includes(`Confirmdigest:${second.id}`), () => input(second.id));
  await capture("tui-owner-typed-digest");
  await observe(text => text.includes("Finite grant confirmed"), () => input("\r"));
  await capture("tui-owner-confirmed");
  state = JSON.parse(await readFile(join(fixture, ".corral/state/automation.json"), "utf8"));
  assert.equal(state.approval.authorization, second.id);
  assert.equal(state.approval.digest, second.grant_digest);
  input("\x1b");
  assert.equal(await processPty.exited, 0);
  await observe(text => text.includes("autopilot: scoped"), () => start(["board"]));
  await capture("board-scoped-status");
  input("\x03");
  assert.equal(await processPty.exited, 0);
  for (const [label, marker] of [["pm", { CORRAL_PM: "1" }], ["agent", { AI_AGENT: "senpi" }]]) {
    const exact = await preview();
    await observe(text => text.includes("not a PM or agent"), () => start([
      "automation", "confirm", "--task", id, "--dispatch", "--digest", exact.id,
    ], marker));
    assert.notEqual(await processPty.exited, 0);
    await capture(`cli-${label}-refused`);
    await observe(text => text.includes("settings") && text.includes("sections"), () => start(["settings"], marker));
    await observe(text => text.includes("▸ Review"), () => input("6"));
    await selectRow("Preview finite grant");
    await observe(text => text.includes("Grant not previewed"), () => input("\r"));
    await capture(`tui-${label}-refused`);
    input("\x03");
    assert.equal(await processPty.exited, 0);
    const unchanged = JSON.parse(await readFile(join(fixture, ".corral/state/automation.json"), "utf8"));
    assert.equal(unchanged.approval.authorization, second.id);
  }
  await writeFile(join(evidence, "result.json"), JSON.stringify({
    status: "PASS", cols, rows, fixture, scenes, cleanup: "PTY processes exited; task-owned browser and server closed",
  }, null, 2));
  console.log(`PASS owner CLI/TUI workflow ${cols}x${rows}: ${scenes.length} scenes`);
} catch (error) {
  console.error(await capture("failure"));
  throw error;
} finally {
  ptyGeneration++;
  try {
    if (processPty && processPty.exitCode === null) { processPty.kill(); await processPty.exited; }
    await queue;
  } finally {
    try { await browser.close(); }
    finally { await server.stop(true); }
  }
}
