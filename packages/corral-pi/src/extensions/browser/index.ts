// Worker-only browser navigation and screenshots (T-574). Each call launches one fresh-profile
// headless browser, checks the top-level target before and after loading, and always cleans up.
import { lstatSync, mkdirSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, extname, join, relative, resolve } from "node:path";

import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

import type { Role } from "../../roles/types.ts";
import { Browser, findBrowser } from "./chrome.ts";
import { type BrowserSettings, BrowserSettingsSchema } from "./settings.ts";
import { BrowserError, browserTarget, checkLanded, inside } from "./target.ts";

export const BROWSER_TOOL_NAMES = ["browser_navigate", "browser_screenshot"] as const;
const TEXT_MAX = 64 * 1024;

/** The generation's browser settings when enabled; throws for another role or malformed settings. */
export function browserSettings(
  role: Role, enabled: readonly string[], settings: Readonly<Record<string, unknown>> | undefined,
): BrowserSettings | undefined {
  if (!enabled.includes("browser")) return undefined;
  if (role !== "worker") throw new Error(`browser is not available to the ${role} role`);
  const parsed = BrowserSettingsSchema.safeParse(settings?.browser);
  if (!parsed.success) throw new Error("capabilities.settings.browser is missing or malformed");
  return parsed.data;
}

interface Viewport { readonly width: number; readonly height: number; readonly fullPage: boolean }
interface Landed { readonly browser: Browser; readonly session: string; readonly url: string; readonly title: string }

export interface BrowserOptions {
  readonly cwd: string;
  /** Parent directory of each call's temporary profile. */
  readonly tmpRoot?: string;
}

async function visit<T>(
  settings: BrowserSettings, options: BrowserOptions, url: string, viewport: Viewport,
  signal: AbortSignal | undefined, act: (page: Landed) => Promise<T>,
): Promise<T> {
  const target = browserTarget(url, settings.origins, options.cwd);
  const executable = findBrowser(settings.executable);
  const timeout = AbortSignal.timeout(settings.timeout_ms);
  const reason = () => signal?.aborted
    ? new BrowserError("browser_interrupted", "the call was cancelled; the browser was stopped")
    : new BrowserError("browser_timeout", `no result within ${settings.timeout_ms} ms; the browser was stopped`);
  const stop = new AbortController();
  const abort = () => stop.abort(reason());
  for (const source of [signal, timeout]) {
    if (source?.aborted) abort();
    else source?.addEventListener("abort", abort, { once: true });
  }
  const browser = Browser.launch({ executable, signal: stop.signal, ...(options.tmpRoot ? { tmpRoot: options.tmpRoot } : {}) });
  try {
    const { targetId } = await browser.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await browser.send("Target.attachToTarget", { targetId, flatten: true });
    const session = String(sessionId);
    await browser.send("Page.enable", {}, session);
    await browser.send("Emulation.setDeviceMetricsOverride",
      { width: viewport.width, height: viewport.height, deviceScaleFactor: 1, mobile: false }, session);
    const loaded = browser.nextEvent("Page.loadEventFired", session);
    loaded.catch(() => {});
    const navigation = await browser.send("Page.navigate", { url: target.href }, session);
    if (typeof navigation.errorText === "string" && navigation.errorText) {
      throw new BrowserError("browser_navigation_failed", `${target.href}: ${navigation.errorText}`);
    }
    await loaded;
    const tree = await browser.send("Page.getFrameTree", {}, session) as { frameTree?: { frame?: { url?: string } } };
    const landed = tree.frameTree?.frame?.url ?? "";
    checkLanded(landed, settings.origins, options.cwd);
    const title = await browser.send("Runtime.evaluate", { expression: "document.title", returnByValue: true }, session);
    return await act({ browser, session, url: landed, title: String((title.result as { value?: unknown })?.value ?? "") });
  } finally {
    signal?.removeEventListener("abort", abort);
    timeout.removeEventListener("abort", abort);
    await browser.close();
  }
}

/**
 * A worktree-relative .png path inside the canonical worktree. The deepest existing ancestor is
 * resolved before any directory is created, so a symlink out of the worktree is refused untouched.
 */
function savePath(path: string, cwd: string): string {
  const root = realpathSync(cwd);
  const file = resolve(root, path);
  const refuse = (reason: string) => new BrowserError("browser_save_refused", `${path} ${reason}`);
  if (extname(file).toLowerCase() !== ".png") throw refuse("must end in .png");
  if (!inside(root, file)) throw refuse("is outside the worktree");
  let existing = dirname(file);
  while (lstatSync(existing, { throwIfNoEntry: false }) === undefined) existing = dirname(existing);
  let real: string;
  try {
    real = realpathSync(existing);
  } catch {
    throw refuse("passes through a dangling symlink");
  }
  if (!inside(root, real) || !statSync(real).isDirectory()) throw refuse("leaves the worktree through a symlink");
  mkdirSync(dirname(file), { recursive: true });
  const target = join(realpathSync(dirname(file)), basename(file));
  if (!inside(root, target) || lstatSync(target, { throwIfNoEntry: false })?.isSymbolicLink()) {
    throw refuse("leaves the worktree through a symlink");
  }
  return target;
}

const urlParam = Type.String({ minLength: 1, maxLength: 8192,
  description: "A configured dev origin URL (http(s)://host:port/...) or a worktree HTML file path" });

export function browserTools(settings: BrowserSettings, options: BrowserOptions): readonly ToolDefinition[] {
  const configured = settings.origins.length ? settings.origins.join(", ") : "none configured";
  return [
    {
      name: "browser_navigate",
      label: "Open page in browser",
      description: "Load a page in a fresh-profile headless browser (no cookies or logins) and return its final URL, "
        + `title and visible text. Targets: worktree .html files and these dev origins: ${configured}. `
        + "A page that ends on another target is refused. Page scripts run and their requests are not intercepted.",
      parameters: Type.Object({ url: urlParam }),
      async execute(_id, params: { url: string }, signal) {
        const page = await visit(settings, options, params.url, { width: 1280, height: 800, fullPage: false }, signal, async (landed) => {
          const text = await landed.browser.send("Runtime.evaluate",
            { expression: "document.body ? document.body.innerText : ''", returnByValue: true }, landed.session);
          const body = String((text.result as { value?: unknown })?.value ?? "");
          return { url: landed.url, title: landed.title, text: body.slice(0, TEXT_MAX), truncated: body.length > TEXT_MAX };
        });
        return { content: [{ type: "text", text: JSON.stringify(page) }], details: undefined };
      },
    },
    {
      name: "browser_screenshot",
      label: "Screenshot page",
      description: "Load a page like browser_navigate and return a PNG screenshot. With save_as (a worktree-relative "
        + ".png path) the image is also written locally, ready for `corral artifact add`; nothing is uploaded.",
      parameters: Type.Object({
        url: urlParam,
        width: Type.Optional(Type.Integer({ minimum: 200, maximum: 3840 })),
        height: Type.Optional(Type.Integer({ minimum: 200, maximum: 2160 })),
        full_page: Type.Optional(Type.Boolean()),
        save_as: Type.Optional(Type.String({ minLength: 1, maxLength: 1024 })),
      }),
      async execute(_id, params: { url: string; width?: number; height?: number; full_page?: boolean; save_as?: string }, signal) {
        const saved = params.save_as === undefined ? undefined : savePath(params.save_as, options.cwd);
        const viewport = { width: params.width ?? 1280, height: params.height ?? 800, fullPage: params.full_page === true };
        const shot = await visit(settings, options, params.url, viewport, signal, async (landed) => {
          let clip: Record<string, number> | undefined;
          if (viewport.fullPage) {
            const metrics = await landed.browser.send("Page.getLayoutMetrics", {}, landed.session) as
              { cssContentSize?: { width?: number; height?: number } };
            clip = { x: 0, y: 0, scale: 1, width: Math.min(metrics.cssContentSize?.width ?? viewport.width, 3840),
              height: Math.min(metrics.cssContentSize?.height ?? viewport.height, 16384) };
          }
          const image = await landed.browser.send("Page.captureScreenshot",
            { format: "png", ...(clip ? { clip, captureBeyondViewport: true } : {}) }, landed.session);
          return { url: landed.url, title: landed.title, data: String(image.data ?? "") };
        });
        const bytes = Buffer.from(shot.data, "base64");
        if (saved) writeFileSync(saved, bytes);
        const summary = { url: shot.url, title: shot.title, bytes: bytes.length, ...(saved ? { saved: relative(realpathSync(options.cwd), saved) } : {}) };
        return {
          content: [{ type: "text", text: JSON.stringify(summary) }, { type: "image", data: shot.data, mimeType: "image/png" }],
          details: undefined,
        };
      },
    },
  ];
}
