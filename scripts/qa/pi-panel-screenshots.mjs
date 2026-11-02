// Rasterize the owned production-host PTY proof, preserving its actual resize/input output.
// Usage: bun scripts/qa/pi-panel-screenshots.mjs <qa-pi-panel-proof.py output directory>
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const directory = resolve(process.argv[2]);
const result = JSON.parse(await readFile(join(directory, "result.json"), "utf8"));
assert.equal(result.status, "PASS");
assert.equal(result.candidate, execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim(), "Capture must match the current candidate");
const requested = new Map([
  ["scoped-agents-wide", "Scoped agents"], ["scoped-agents-narrow", "Scoped agents"],
  ["existing-transcript-wide", "Transcript"], ["existing-transcript-narrow", "Transcript"],
  ["real-job-running", "armed locally"], ["real-job-line-match", "match observed"], ["real-job-exited", "exit 7"],
]);
const [js, css] = await Promise.all([
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"),
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css"),
]);
assert(js.ok && css.ok, "Pinned terminal renderer assets unavailable");
const [script, style] = await Promise.all([js.text(), css.text()]);
const font = Buffer.from(await readFile("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")).toString("base64");
let html;
const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } }) });
const browser = new Bun.WebView({ width: 1600, height: 1100 });
const captures = [];
const sha = bytes => createHash("sha256").update(bytes).digest("hex");
try {
  for (const scene of result.scenes.filter(scene => requested.has(scene.name))) {
    const ansi = await readFile(join(directory, scene.name + ".ansi"));
    assert.equal(ansi.length, scene.bytes);
    const operations = [];
    let offset = 0;
    for (const resize of result.resizes.filter(resize => resize.offset < ansi.length)) {
      operations.push({ text: ansi.subarray(offset, resize.offset).toString("utf8") }, { columns: resize.columns, rows: resize.rows });
      offset = resize.offset;
    }
    operations.push({ text: ansi.subarray(offset).toString("utf8") });
    html = `<!doctype html><meta charset="utf-8"><style>${style}
      @font-face{font-family:ProofMono;src:url(data:font/ttf;base64,${font})}
      body{margin:0;padding:16px;background:#191b20;overflow:hidden}
      #terminal{width:1568px;height:1068px}.xterm-viewport{overflow:hidden!important}</style>
      <div id="terminal"></div><script>${script}</script><script>
      window.ready=(async()=>{
        await document.fonts.load("15px ProofMono");
        const term=window.term=new Terminal({cols:${result.initial_size.columns},rows:${result.initial_size.rows},
          fontFamily:"ProofMono",fontSize:15,lineHeight:1.2,disableStdin:true,scrollback:0,
          theme:{background:"#191b20",foreground:"#e0e3e9"}});
        term.open(document.getElementById("terminal"));
        for(const op of ${JSON.stringify(operations).replaceAll("<", "\\u003c")}) {
          if(op.text!==undefined) await new Promise(resolve=>term.write(op.text,resolve));
          else term.resize(op.columns,op.rows);
        }
        await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
      })()</script>`;
    await browser.navigate(`http://127.0.0.1:${server.port}/`);
    await browser.evaluate("window.ready");
    assert.equal(await browser.evaluate("term.cols"), scene.columns);
    assert.equal(await browser.evaluate("term.rows"), scene.rows);
    const visible = await browser.evaluate("Array.from({length:term.rows},(_,i)=>term.buffer.active.getLine(term.buffer.active.viewportY+i)?.translateToString(true)??'').join('\\n')");
    assert(visible.includes(requested.get(scene.name)), `${scene.name}: expected actual terminal state is not visible`);
    const cell = await browser.evaluate("term._core._renderService.dimensions.css.cell");
    const width = Math.ceil(cell.width * scene.columns) + 32, height = Math.ceil(cell.height * scene.rows) + 32;
    await browser.evaluate(`Object.assign(document.getElementById("terminal").style,{width:"${width - 32}px",height:"${height - 32}px"})`);
    await browser.cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    const png = Buffer.from(await (await browser.screenshot()).arrayBuffer());
    assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
    assert.equal(png.readUInt32BE(16), width); assert.equal(png.readUInt32BE(20), height);
    await writeFile(join(directory, scene.name + ".png"), png);
    captures.push({ ...scene, candidate: result.candidate, width, height, ansi_sha256: sha(ansi), png_sha256: sha(png), renderer: "xterm.js 5.5.0; actual owned PTY ANSI and resize sequence" });
  }
  assert.equal(captures.length, requested.size);
} finally { await browser.close(); await server.stop(true); }
await writeFile(join(directory, "screenshots.json"), JSON.stringify({ candidate: result.candidate, captures }, null, 2) + "\n");
console.log(`PASS: ${captures.length} candidate-linked production-host terminal screenshots`);
