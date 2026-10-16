// Replay actual herdr ANSI captures through xterm.js and capture the browser surface.
// Usage: bun scripts/qa/stage-screenshots.mjs <live-evidence-directory>
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const directory = resolve(process.argv[2] ?? ".omo/evidence/T-067/live-120");
const result = JSON.parse(await readFile(join(directory, "result.json"), "utf8"));
assert(result.status === "PASS" || process.argv.includes("--failed"), "Use --failed to inspect failed captures");
const [scriptResponse, styleResponse] = await Promise.all([
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"),
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css"),
]);
assert(scriptResponse.ok && styleResponse.ok, "Pinned xterm renderer assets must be available");
const [script, style] = await Promise.all([scriptResponse.text(), styleResponse.text()]);
const font = Buffer.from(await readFile("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")).toString("base64");
let html = "";
const server = Bun.serve({
  hostname: "127.0.0.1", port: 0,
  fetch: () => new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } }),
});
const columns = result.columns;
const width = Math.ceil(columns * 9.1) + 32;
const height = 30 * 18 + 32;
const browser = new Bun.WebView({ width, height });
const captures = [];
try {
  await browser.navigate(`http://127.0.0.1:${server.port}/`);
  await browser.cdp("Emulation.setDeviceMetricsOverride", {
    width, height, deviceScaleFactor: 1, mobile: false,
  });
  for (const scene of result.scenes) {
    let ansi = await readFile(join(directory, `${scene}.ansi`), "utf8");
    if (ansi.trimStart().startsWith("{")) {
      const value = JSON.parse(ansi);
      ansi = value.result?.read?.text ?? value.result?.text ?? value.read?.text;
      assert.equal(typeof ansi, "string", "Extract ANSI, not the RPC envelope");
    }
    const rows = ansi.split(/\r?\n/);
    while (rows.length && rows.at(-1).replace(/\x1b\[[0-9;]*m/g, "").trim() === "") rows.pop();
    ansi = rows.join("\r\n");
    assert(rows.length <= 30, "Capture must fit the original 30-row terminal");
    html = `<!doctype html><meta charset="utf-8"><style>${style}
      @font-face{font-family:StageMono;src:url(data:font/ttf;base64,${font})}
      body{margin:0;padding:16px;background:#111318;overflow:hidden;font-family:StageMono}
      #terminal{width:${width - 32}px;height:540px}
      .xterm-viewport{overflow:hidden!important}</style><div id="terminal"></div>
      <script>${script}</script><script>
      window.ready = (async () => {
      await document.fonts.load("15px StageMono");
      const term = window.term = new Terminal({cols:${columns},rows:30,fontFamily:"StageMono",fontSize:15,
        lineHeight:1.2,letterSpacing:0,disableStdin:true,scrollback:0,
        theme:{background:"#111318",foreground:"#d7dce2"}});
      term.open(document.getElementById("terminal"));
      await new Promise(resolve => term.write("\\x1b[?25l\\x1b[?7l" + ${JSON.stringify(ansi).replaceAll("<", "\\u003c")},
        () => requestAnimationFrame(() => requestAnimationFrame(resolve))));
      })();
      </script>`;
    await browser.navigate(`http://127.0.0.1:${server.port}/`);
    await browser.evaluate("window.ready");
    assert((await browser.evaluate("term.buffer.active.getLine(0).translateToString(true)")).includes("corral"),
      "The original board header must not scroll out of the capture");
    const metrics = await browser.evaluate(`({
      viewport: [innerWidth, innerHeight], dpr: devicePixelRatio,
      terminal: [term.element.scrollWidth, term.element.scrollHeight],
      cell: term._core._renderService.dimensions.css.cell,
    })`);
    const actualWidth = Math.ceil(metrics.cell.width * columns) + 32;
    const actualHeight = Math.ceil(metrics.cell.height * 30) + 32;
    await browser.evaluate(`Object.assign(document.getElementById("terminal").style, {
      width: "${actualWidth - 32}px", height: "${actualHeight - 32}px",
    })`);
    await browser.cdp("Emulation.setDeviceMetricsOverride", {
      width: actualWidth, height: actualHeight, deviceScaleFactor: 1, mobile: false,
    });
    await browser.evaluate("new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))");
    console.log(scene, JSON.stringify({ width: actualWidth, height: actualHeight, cell: metrics.cell }));
    const screen = await browser.screenshot();
    const path = join(directory, `${scene}.png`);
    const bytes = Buffer.from(await screen.arrayBuffer());
    assert.equal(bytes.readUInt32BE(16), actualWidth, "PNG width must match viewport");
    assert.equal(bytes.readUInt32BE(20), actualHeight, "PNG height must match viewport");
    await writeFile(path, bytes);
    captures.push({ scene, path, width: actualWidth, height: actualHeight, metrics,
      renderer: "xterm.js 5.5.0; actual herdr ANSI" });
  }
} finally {
  await browser.close();
  await server.stop(true);
}
await writeFile(join(directory, "screenshots.json"), JSON.stringify(captures, null, 2) + "\n");
console.log(`Captured ${captures.length} real xterm scenes at ${columns} columns.`);
