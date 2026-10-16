// Render actual herdr ANSI through xterm.js, at each scene's original terminal dimensions.
import assert from "node:assert/strict";
import { readFile, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";

const directory = resolve(process.argv[2] ?? ".omo/evidence/T-024/live");
const result = JSON.parse(await readFile(join(directory, "result.json"), "utf8"));
assert.equal(result.status, "PASS", "Only successful current acceptance captures are evidence");
const [js, css] = await Promise.all([
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/lib/xterm.js"),
  fetch("https://cdn.jsdelivr.net/npm/@xterm/xterm@5.5.0/css/xterm.css"),
]);
assert(js.ok && css.ok);
const [script, style] = await Promise.all([js.text(), css.text()]);
const font = Buffer.from(await readFile("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf")).toString("base64");
let html;
const server = Bun.serve({ hostname: "127.0.0.1", port: 0,
  fetch: () => new Response(html, { headers: { "content-type": "text/html; charset=utf-8" } }) });
const browser = new Bun.WebView({ width: 1600, height: 1000 });
const captures = [];
try {
  for (const scene of result.scenes.filter(({ name }) => name.includes("gallery") || name.includes("pinned") || name.includes("filter") || name.includes("recent"))) {
    const ansi = await readFile(join(directory, `${scene.name}.ansi`), "utf8");
    const lines = ansi.split(/\r?\n/);
    while (lines.length && lines.at(-1).replace(/\x1b\[[0-9;]*m/g, "").trim() === "") lines.pop();
    assert(lines.length <= scene.rows, `${scene.name} must fit original terminal height`);
    html = `<!doctype html><meta charset="utf-8"><style>${style}
    @font-face{font-family:GalleryMono;src:url(data:font/ttf;base64,${font})}
    body{margin:0;padding:16px;background:#111318;overflow:hidden}
    #terminal{width:1568px;height:968px}.xterm-viewport{overflow:hidden!important}</style>
    <div id="terminal"></div><script>${script}</script><script>
    window.ready=(async()=>{
      await document.fonts.load("15px GalleryMono");
      const term=window.term=new Terminal({cols:${scene.columns},rows:${scene.rows},
        fontFamily:"GalleryMono",fontSize:15,lineHeight:1.2,letterSpacing:0,disableStdin:true,scrollback:0,
        theme:{background:"#111318",foreground:"#d7dce2"}});
      term.open(document.getElementById("terminal"));
      await new Promise(resolve=>term.write("\\x1b[?25l\\x1b[?7l"+${JSON.stringify(lines.join("\r\n")).replaceAll("<", "\\u003c")},
        ()=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
    })()</script>`;
    await browser.navigate(`http://127.0.0.1:${server.port}/`);
    await browser.evaluate("window.ready");
    const cell = await browser.evaluate("term._core._renderService.dimensions.css.cell");
    const width = Math.ceil(cell.width * scene.columns) + 32;
    const height = Math.ceil(cell.height * scene.rows) + 32;
    await browser.evaluate(`Object.assign(document.getElementById("terminal").style,{width:"${width - 32}px",height:"${height - 32}px"})`);
    await browser.cdp("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await browser.evaluate("new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)))");
    assert((await browser.evaluate("term.buffer.active.getLine(2).translateToString(true)")).includes("Gallery /"),
      "Actual gallery header must remain in its original frame");
    const png = Buffer.from(await (await browser.screenshot()).arrayBuffer());
    assert.equal(png.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
    assert.equal(png.readUInt32BE(16), width);
    assert.equal(png.readUInt32BE(20), height);
    await writeFile(join(directory, `${scene.name}.png`), png);
    captures.push({ ...scene, width, height, renderer: "xterm.js 5.5.0 / actual isolated herdr ANSI" });
  }
} finally {
  await browser.close();
  await server.stop(true);
}
await writeFile(join(directory, "screenshots.json"), JSON.stringify(captures, null, 2) + "\n");
console.log(`PASS: ${captures.length} actual xterm gallery screenshots`);
