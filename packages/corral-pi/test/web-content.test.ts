import { expect, test } from "bun:test";

import type { WebAccess } from "../src/extensions/web/client.ts";
import { webTools } from "../src/extensions/web/index.ts";
import { renderContent } from "../src/extensions/web/content.ts";
import { WebSnapshots } from "../src/extensions/web/snapshots.ts";

function fixture(body: string, content_type = "text/plain", failure?: Error) {
  let requests = 0;
  const access: WebAccess = {
    async fetch(url) { requests++; if (failure) throw failure; return { url, status: 200, content_type, body,
      bytes: Buffer.byteLength(body), truncated: false }; },
    async search() { return { configured: false, setup: "not configured", results: [] }; },
  };
  const tools = webTools(access);
  return {
    requests: () => requests,
    async call(name: string, params: Record<string, unknown>) {
      const tool = tools.find((tool) => tool.name === name);
      if (!tool) throw new Error(`missing tool ${name}`);
      return tool.execute("fixture", params, undefined, undefined, undefined as never);
    },
  };
}

async function page(web: ReturnType<typeof fixture>, name: string, params: Record<string, unknown>) {
  const result = await web.call(name, params);
  const text = result.content.map((part) => part.type === "text" ? part.text : "").join("");
  expect(Buffer.byteLength(text)).toBeLessThanOrEqual(24 * 1024);
  return JSON.parse(text);
}

const url = "https://docs.example.org/guide/";
function source(body: string, content_type = "text/html") {
  return { url, status: 200, content_type, body, bytes: Buffer.byteLength(body), truncated: false };
}

test("web fetch bounds the complete model result, even for escaped minified content", async () => {
  // Given a one-line page whose JSON encoding expands past its source byte count.
  const web = fixture('"\\\t漢🙂'.repeat(6000));
  // When the actual tool returns it, then metadata and body together fit the context budget.
  const result = await web.call("web_fetch", { url: "https://docs.example.org/" });
  const text = result.content.map((part) => part.type === "text" ? part.text : "").join("");
  expect(Buffer.byteLength(text)).toBeLessThanOrEqual(24 * 1024);
  expect(JSON.parse(text).output_truncated).toBeTrue();
  expect(result.details).toBeUndefined();
  expect(result.structuredContent).toBeUndefined();
});

test("clean HTML preserves docs and safe links without scripts, hidden content or inline image payloads", async () => {
  // Given a documentation page with boilerplate and payloads that should never reach the model.
  const web = fixture(`<html><head><style>STYLE_NOISE</style></head><body>
    <nav>NAV_NOISE</nav><main><h1>Guide</h1><p>Read <a href="../api">API</a>.</p>
    <pre><code class="language-ts">const x = 1;</code></pre><ul><li>First</li></ul>
    <table><tr><th>Name</th><th>Value</th></tr><tr><td>Port</td><td>8080</td></tr></table>
    <script>SCRIPT_NOISE</script><p hidden>HIDDEN_NOISE</p><p aria-hidden="true">ARIA_NOISE</p>
    <img src="data:image/png;base64,IMAGE_PAYLOAD" alt="Diagram">
    <a href="javascript:alert(1)">Unsafe link</a><iframe src="https://remote.example/">FRAME_NOISE</iframe>
    </main><footer>FOOTER_NOISE</footer></body></html>`, "text/html");
  // When fetched in the default mode, then only inert readable Markdown and useful structure remain.
  const result = await page(web, "web_fetch", { url });
  expect(result.method).toBe("main");
  for (const useful of ["# Guide", "[API](https://docs.example.org/api)", "```ts", "const x = 1;", "| Name", "| Port", "Diagram"]) {
    expect(result.body).toContain(useful);
  }
  expect(result.body).toMatch(/^-\s+First$/m);
  for (const noise of ["NOISE", "IMAGE_PAYLOAD", "javascript:", "data:image"]) expect(result.body).not.toContain(noise);
  expect(web.requests()).toBe(1);
});

test("article extraction and cleaned fragment fallback do not require explicit main containers", () => {
  // Given an ordinary article and a short fragment that is not Readability-shaped.
  const article = `<html><head><title>Article</title></head><body><div class="entry-content"><h1>Article</h1>
    <p>This article explains bounded web content, its benefits, and the implementation details. The reader should preserve these sentences.</p>
    <p>A second paragraph carries useful examples and documentation, without executable content or external resources.</p></div></body></html>`;
  const readable = renderContent(source(article), "markdown");
  expect(readable.method).toBe("article");
  expect(readable.body).toContain("A second paragraph");
  const fragment = renderContent(source("<p>Small fragment</p><script>UNWANTED</script>"), "markdown");
  expect(fragment.body).toBe("Small fragment");
});

test("raw mode remains bounded, while textual non-HTML content stays verbatim", async () => {
  const raw = await page(fixture("<main>" + "x".repeat(25000) + "</main>", "text/html"), "web_fetch", { url, format: "raw" });
  expect(raw.method).toBe("raw");
  expect(raw.body.startsWith("<main>")).toBeTrue();
  expect(raw.output_truncated).toBeTrue();
  for (const type of ["application/json", "text/markdown", "application/xml"]) {
    const body = '{"raw":"<tag>","spaces":  2}';
    const result = await page(fixture(body, type), "web_fetch", { url });
    expect(result.body).toBe(body);
    expect(result.method).toBe("text");
  }
});

test("snapshot pages reconstruct multibyte content without gaps or another network request", async () => {
  // Given escaped, multibyte content requiring multiple pages under the serialized cap.
  const body = '"\\\t漢🙂\ufffd'.repeat(3000);
  const web = fixture(body);
  const first = await page(web, "web_fetch", { url });
  let result = first;
  let rebuilt = result.body;
  let pages = 1;
  while (result.next_offset !== null && pages < 20) {
    const next = await page(web, "web_read", { content_id: first.content_id, offset: result.next_offset });
    expect(next.offset).toBe(result.next_offset);
    expect(next.output_bytes).toBeGreaterThan(0);
    rebuilt += next.body;
    result = next;
    pages++;
  }
  expect(result.next_offset).toBeNull();
  expect(pages).toBeGreaterThan(1);
  expect(rebuilt).toBe(body);
  expect(web.requests()).toBe(1);
  for (const range of [{ offset: 4 }, { offset: -1 }, { limit: 3 }]) {
    const bad = await page(web, "web_read", { content_id: first.content_id, ...range });
    expect(bad.error.code).toBe("web_page_range");
  }
  const eof = await page(web, "web_read", { content_id: first.content_id, offset: first.total_bytes });
  expect(eof.body).toBe("");
});

test("snapshot retention caps individual content, count and total bytes and never reuses missing IDs", () => {
  // Small injected budgets exercise exact resource limits rather than building large histories.
  const cache = new WebSnapshots({ entries: 2, bytes: 1024, contentBytes: 8 });
  const add = (body: string) => cache.add(source(body, "text/plain"), { body, method: "text" }, "markdown");
  const first = add("a漢🙂tail");
  const clipped = JSON.parse(cache.read(first));
  expect(clipped.body).toBe("a漢🙂");
  expect(clipped.snapshot_truncated).toBeTrue();
  expect(clipped.network_truncated).toBeFalse();
  expect(clipped.next_offset).toBeNull();
  add("second"); add("third");
  expect(() => cache.read(first)).toThrow("web_snapshot_missing");
  const byteCache = new WebSnapshots({ entries: 8, bytes: 1024, contentBytes: 600 });
  const old = byteCache.add(source("x".repeat(500)), { body: "x".repeat(500), method: "text" }, "markdown");
  const current = byteCache.add(source("y".repeat(500)), { body: "y".repeat(500), method: "text" }, "markdown");
  expect(() => byteCache.read(old)).toThrow("web_snapshot_missing");
  expect(JSON.parse(byteCache.read(current)).body).toBe("y".repeat(500));
  expect(() => new WebSnapshots().read(current)).toThrow("web_snapshot_missing");
});

test("metadata, source truncation and unsupported binary samples cannot bypass the output boundary", async () => {
  for (const type of ["application/pdf", "image/png", "application/octet-stream", ""]) {
    const web = fixture("\0\u0001\u0002binary", type);
    const result = await web.call("web_fetch", { url });
    expect(result.isError).toBeTrue();
    expect((await page(web, "web_fetch", { url })).error.code).toBe("web_unsupported_content");
  }
  const cache = new WebSnapshots();
  const id = cache.add({ ...source("done"), url: url + '"'.repeat(40000), content_type: '"'.repeat(40000), truncated: true },
    { body: "done", method: "text" }, "markdown");
  const text = cache.read(id);
  expect(Buffer.byteLength(text)).toBeLessThanOrEqual(24576);
  const result = JSON.parse(text);
  expect(result.url_truncated).toBeTrue();
  expect(result.network_truncated).toBeTrue();
  expect(result.output_truncated).toBeFalse();
  expect(result.truncated).toBeTrue();
  const error = await page(fixture("", "text/plain", new Error("\u0001".repeat(20000))), "web_fetch", { url });
  expect(error.error.code).toBe("web_request_failed");
  const missingMime = await page(fixture("<div>Useful</div><script>NOISE</script>", ""), "web_fetch", { url });
  expect(missingMime.body).toBe("Useful");
});
