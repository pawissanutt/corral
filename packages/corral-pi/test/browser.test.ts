// Worker browser (T-574): configured dev origins and worktree HTML load in a fresh profile with no
// carried cookies, other top-level targets fail, a finished or cancelled call leaves no browser
// process or profile behind, and a browser whose host dies exits with it.
import { afterAll, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import { type IncomingMessage, type Server, type ServerResponse, createServer } from "node:http";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";

import { findBrowser } from "../src/extensions/browser/chrome.ts";
import { browserSettings, browserTools } from "../src/extensions/browser/index.ts";
import { browserTarget, checkLanded } from "../src/extensions/browser/target.ts";
import { within } from "./support/wait.ts";

const work = mkdtempSync(join(tmpdir(), "corral-pi-browser-"));
const cwd = join(work, "worktree");
const profiles = join(work, "profiles");
mkdirSync(join(cwd, "site"), { recursive: true });
mkdirSync(profiles);
writeFileSync(join(cwd, "site", "index.html"), "<title>Worktree Page</title><body><h1>Local fixture</h1></body>");
writeFileSync(join(cwd, "notes.txt"), "not html");
writeFileSync(join(work, "outside.html"), "<title>Outside</title>");
symlinkSync(join(work, "outside.html"), join(cwd, "escape.html"));

const chrome = (() => {
  try {
    return findBrowser(undefined);
  } catch {
    return undefined;
  }
})();
if (!chrome) console.log("browser.test: no Chrome/Chromium on PATH; live browser checks skipped");

const cookies: (string | undefined)[] = [];
let hung: () => void = () => {};
const hangReached = new Promise<void>((resolve) => { hung = resolve; });
let strayHits = 0;
const html = (response: ServerResponse, body: string, headers: Record<string, string> = {}) =>
  response.writeHead(200, { "content-type": "text/html", ...headers }).end(body);
async function listen(handler: (request: IncomingMessage, response: ServerResponse) => void): Promise<[Server, number]> {
  const server = createServer(handler);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return [server, typeof address === "object" && address ? address.port : 0];
}
const [stray, strayPort] = await listen((_request, response) => {
  strayHits += 1;
  html(response, "<title>Stray</title>");
});
const [dev, devPort] = await listen((request, response) => {
  const path = new URL(request.url ?? "/", "http://dev").pathname;
  if (path === "/away") return void response.writeHead(302, { location: `http://127.0.0.1:${strayPort}/` }).end();
  if (path === "/hang") return hung();
  if (path === "/favicon.ico") return void response.writeHead(404).end();
  cookies.push(request.headers.cookie);
  html(response, "<title>Dev Home</title><body>dev server body</body>",
    { "set-cookie": "owner_session=secret; Path=/; Max-Age=3600" });
});
const origin = `http://127.0.0.1:${devPort}`;
const settings = { origins: [origin], timeout_ms: 20_000 };
const tools = browserTools(settings, { cwd, tmpRoot: profiles });
const tool = (name: string): ToolDefinition => tools.find((entry) => entry.name === name) as ToolDefinition;
const run = (name: string, params: Record<string, unknown>, signal?: AbortSignal) =>
  tool(name).execute("call-1", params as never, signal, undefined, undefined as never);
const text = (result: Awaited<ReturnType<typeof run>>) => {
  const first = result.content[0];
  return JSON.parse(first?.type === "text" ? first.text : "{}") as Record<string, unknown>;
};

/** Live processes whose environment carries `marker` (each call's private HOME lives under it). */
function owned(marker: string): number[] {
  return readdirSync("/proc").filter((pid) => /^\d+$/.test(pid)).filter((pid) => {
    try {
      return readFileSync(`/proc/${pid}/environ`, "utf8").includes(marker);
    } catch {
      return false;
    }
  }).map(Number);
}

/** Waits for the observable state "no owned process and no profile left", with a deadline. */
async function cleanedUp(marker: string, root: string): Promise<void> {
  const deadline = Date.now() + 10_000;
  while (owned(marker).length || readdirSync(root).length) {
    if (Date.now() > deadline) throw new Error(`browser leftovers: ${owned(marker).join(",")} ${readdirSync(root).join(",")}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}

afterAll(() => {
  dev.closeAllConnections();
  dev.close();
  stray.close();
  rmSync(work, { recursive: true, force: true });
});

test("top-level targets are only configured origins and canonical worktree HTML files", () => {
  // Given one configured dev origin and a worktree with HTML, text and an escaping symlink.
  const accepted = [`${origin}/`, `${origin}/a/b?q=1`, "site/index.html", join(cwd, "site/index.html"),
    pathToFileURL(join(cwd, "site/index.html")).href];
  // When resolving targets, then accepted ones keep their origin or canonical file URL.
  for (const input of accepted) expect(() => browserTarget(input, settings.origins, cwd)).not.toThrow();
  expect(browserTarget("site/index.html", settings.origins, cwd).protocol).toBe("file:");
  for (const input of [`http://127.0.0.1:${strayPort}/`, `https://127.0.0.1:${devPort}/`, `http://user:pw@127.0.0.1:${devPort}/`,
    "https://example.org/", "data:text/html,hi", "javascript:alert(1)", "about:blank", "chrome://settings",
    "notes.txt", "escape.html", join(work, "outside.html"), "../outside.html", "missing.html"]) {
    // Then everything else is refused before any browser starts.
    expect(() => browserTarget(input, settings.origins, cwd)).toThrow(/browser_target_refused/);
  }
  expect(readdirSync(profiles)).toEqual([]);
});

test("configured origins match their canonical URL origin, before launch and after landing", () => {
  // Given origins as configured with default or zero-padded ports and long-form IPv6.
  const configured = ["http://localhost:80", "https://localhost:443", "http://localhost:05173", "http://[0:0:0:0:0:0:0:1]:5173"];
  // When targeting and landing on the URLs those origins name.
  for (const href of ["http://localhost/", "https://localhost/a", "http://localhost:5173/", "http://[::1]:5173/x"]) {
    // Then each is allowed, and the canonical forms Rust records are their own URL origin.
    expect(() => browserTarget(href, configured, cwd)).not.toThrow();
    expect(() => checkLanded(href, configured, cwd)).not.toThrow();
    expect(() => browserTarget(href, [new URL(href).origin], cwd)).not.toThrow();
  }
  for (const canonical of ["http://localhost", "https://localhost", "http://localhost:5173", "http://[::1]:5173",
    "http://[2001:db8::1:0:0:1]", "http://[1:0:0:2::3]", "http://[::ffff:7f00:1]"]) {
    expect(new URL(canonical).origin).toBe(canonical);
  }
  // And equivalence never broadens the set: other ports, schemes and hosts stay refused.
  for (const href of ["http://localhost:8080/", "https://localhost:80/", "http://localhost:443/", "http://[::2]:5173/"]) {
    expect(() => browserTarget(href, configured, cwd)).toThrow(/browser_target_refused/);
    expect(() => checkLanded(href, configured, cwd)).toThrow(/browser_target_refused/);
  }
});

test("a save_as escaping the worktree is refused without creating anything outside it", async () => {
  // Given worktree symlinks to an outside directory and to a missing outside path.
  const outside = mkdtempSync(join(work, "outside-"));
  symlinkSync(outside, join(cwd, "linked"));
  symlinkSync(join(work, "missing-outside"), join(cwd, "dangling"));
  // When screenshots try to save through them, before any browser or target check.
  for (const save of ["linked/new-directory/page.png", "dangling/new-directory/page.png", "../escape/page.png", "shots/page.txt"]) {
    await expect(run("browser_screenshot", { url: "missing.html", save_as: save })).rejects.toThrow(/browser_save_refused/);
  }
  // Then no directory was created outside the worktree.
  expect(readdirSync(outside)).toEqual([]);
  expect(existsSync(join(work, "missing-outside"))).toBeFalse();
  expect(existsSync(join(work, "escape"))).toBeFalse();
  expect(readdirSync(profiles)).toEqual([]);
});

test("browser settings are worker-only and must be resolved by corral", () => {
  // Given launch settings for each role.
  const resolved = { browser: settings };
  // When the host reads them, then only the worker receives a browser and malformed settings fail.
  expect(browserSettings("worker", ["browser"], resolved)).toEqual(settings);
  expect(browserSettings("worker", [], resolved)).toBeUndefined();
  expect(() => browserSettings("reviewer", ["browser"], resolved)).toThrow(/not available to the reviewer/);
  expect(() => browserSettings("pm", ["browser"], resolved)).toThrow(/not available to the pm/);
  expect(() => browserSettings("worker", ["browser"], { browser: { origins: ["ftp://x"], timeout_ms: 1 } })).toThrow(/malformed/);
  expect(() => browserSettings("worker", ["browser"], undefined)).toThrow(/malformed/);
});

test("navigate and screenshot use fresh profiles and attach screenshots locally", async () => {
  if (!chrome) return;
  // Given a dev server that sets a long-lived cookie on every page load.
  // When navigating twice to the configured origin.
  const first = text(await run("browser_navigate", { url: `${origin}/` }));
  const second = text(await run("browser_navigate", { url: `${origin}/` }));
  // Then each call sees the page, but no cookie survives into the next fresh profile.
  expect([first.url, first.title, first.truncated]).toEqual([`${origin}/`, "Dev Home", false]);
  expect(String(second.text)).toContain("dev server body");
  expect(cookies).toEqual([undefined, undefined]);
  // When screenshotting worktree HTML with a local save path.
  const shot = await run("browser_screenshot", { url: "site/index.html", width: 640, height: 480, save_as: "shots/nested/page.png" });
  // Then the PNG is returned inline and written inside the worktree, and nothing is left running.
  const image = shot.content[1];
  expect(image?.type).toBe("image");
  const png = Buffer.from(image?.type === "image" ? image.data : "", "base64");
  expect(png.subarray(1, 4).toString()).toBe("PNG");
  const summary = text(shot);
  expect([summary.title, summary.saved, summary.bytes]).toEqual(["Worktree Page", "shots/nested/page.png", png.length]);
  expect(readFileSync(join(cwd, "shots/nested/page.png")).equals(png)).toBe(true);
  await expect(run("browser_screenshot", { url: "site/index.html", save_as: "../out.png" })).rejects.toThrow(/browser_save_refused/);
  await cleanedUp(profiles, profiles);
}, 60_000);

test("unconfigured targets fail, including a redirect away from a configured origin", async () => {
  if (!chrome) return;
  // Given an unconfigured server.
  // When the worker targets it directly, then the call fails without a request reaching it.
  await expect(run("browser_navigate", { url: `http://127.0.0.1:${strayPort}/` })).rejects.toThrow(/browser_target_refused/);
  expect(strayHits).toBe(0);
  // When a configured page redirects to it, then the landed page is refused and not returned.
  await expect(run("browser_screenshot", { url: `${origin}/away` })).rejects.toThrow(/landed on http:\/\/127\.0\.0\.1/);
  await cleanedUp(profiles, profiles);
}, 60_000);

test("a cancelled call stops its browser and removes its profile", async () => {
  if (!chrome) return;
  // Given a page load the dev server never answers.
  const controller = new AbortController();
  const call = run("browser_navigate", { url: `${origin}/hang` }, controller.signal);
  call.catch(() => {});
  // When the request reaches the server and the call is cancelled.
  await within(hangReached, 20_000, "the hanging page request");
  // Then, while it runs, the browser's profile and HOME are that call's own fresh directory.
  const main = owned(profiles).map((pid) => ({
    argv: readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0"),
    env: readFileSync(`/proc/${pid}/environ`, "utf8").split("\0"),
  })).find(({ argv }) => argv.includes("--remote-debugging-pipe"));
  const [root] = readdirSync(profiles);
  expect(main?.argv).toContain(`--user-data-dir=${join(profiles, String(root), "profile")}`);
  expect(main?.env).toContain(`HOME=${join(profiles, String(root), "home")}`);
  controller.abort();
  // Then the call fails as interrupted and the browser and its profile are gone.
  await expect(call).rejects.toThrow(/browser_interrupted/);
  await cleanedUp(profiles, profiles);
}, 60_000);

test("a browser exits with its host when the host process dies", async () => {
  if (!chrome) return;
  // Given a host process that launched a browser and reported its pid.
  const root = mkdtempSync(join(work, "host-"));
  const script = join(work, "host.ts");
  writeFileSync(script, `import { Browser } from ${JSON.stringify(fileURLToPath(new URL("../src/extensions/browser/chrome.ts", import.meta.url)))};
const browser = Browser.launch({ executable: ${JSON.stringify(chrome)}, signal: new AbortController().signal, tmpRoot: ${JSON.stringify(root)} });
await browser.send("Browser.getVersion");
console.log("READY " + browser.child.pid);
setInterval(() => {}, 1 << 30);
`);
  const host = spawn(process.execPath, [script], { stdio: ["ignore", "pipe", "inherit"] });
  const ready = new Promise<number>((resolve) => {
    let out = "";
    host.stdout?.on("data", (chunk: Buffer) => {
      out += chunk.toString();
      const match = /READY (\d+)/.exec(out);
      if (match?.[1]) resolve(Number(match[1]));
    });
  });
  const pid = await within(ready, 20_000, "the host's browser");
  expect(owned(root)).toContain(pid);
  // When the host is killed without any cleanup of its own.
  const exited = new Promise((resolve) => host.once("exit", resolve));
  host.kill("SIGKILL");
  await exited;
  // Then the browser sees its DevTools pipe close and its whole process tree exits.
  const deadline = Date.now() + 10_000;
  while (owned(root).length) {
    if (Date.now() > deadline) throw new Error(`orphaned browser processes ${owned(root).join(",")}`);
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
}, 60_000);
