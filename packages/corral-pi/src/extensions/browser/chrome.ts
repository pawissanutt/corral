// One owned headless Chrome per browser call. It gets a fresh temporary profile and HOME, so no
// owner cookies, logins or extensions are visible, and speaks DevTools over --remote-debugging-pipe:
// when the host dies its pipe closes and Chrome exits. close() kills the process group, waits for
// the exit and removes the profile. Browser networking is outside the in-process network guard.
import { type ChildProcess, spawn } from "node:child_process";
import { accessSync, constants, mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import type { Readable, Writable } from "node:stream";

import { BrowserError } from "./target.ts";

const CANDIDATES = ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "chrome"];

function executable(file: string): boolean {
  try {
    accessSync(file, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

export function findBrowser(configured: string | undefined, path = process.env.PATH ?? ""): string {
  if (configured !== undefined) {
    if (executable(configured)) return configured;
    throw new BrowserError("browser_unavailable", `${configured} is not an executable file`);
  }
  for (const name of CANDIDATES) {
    for (const dir of path.split(delimiter)) if (dir && executable(join(dir, name))) return join(dir, name);
  }
  throw new BrowserError("browser_unavailable",
    "no Chrome or Chromium on PATH; set executable in [agent.capabilities.worker.browser]");
}

const ARGS = [
  "--headless=new", "--remote-debugging-pipe", "--no-first-run", "--no-default-browser-check",
  "--password-store=basic", "--use-mock-keychain", "--disable-extensions", "--disable-sync",
  "--disable-background-networking", "--disable-component-update", "--disable-default-apps",
  "--disable-breakpad", "--disable-crash-reporter", "--mute-audio", "--hide-scrollbars",
];

type Params = Record<string, unknown>;
interface Message { id?: number; method?: string; params?: Params; sessionId?: string; result?: Params; error?: { message?: string } }
interface Waiter { resolve(value: Params): void; reject(error: Error): void }

export interface LaunchOptions {
  readonly executable: string;
  readonly signal: AbortSignal;
  /** Parent of the temporary profile; tests point it at a private directory. */
  readonly tmpRoot?: string;
}

export class Browser {
  private next = 1;
  private readonly pending = new Map<number, Waiter>();
  private readonly events = new Map<string, Set<Waiter>>();
  private failure: Error | undefined;
  private readonly exited: Promise<void>;

  private constructor(readonly child: ChildProcess, readonly root: string, private readonly input: Writable, output: Readable) {
    let buffer = "";
    output.setEncoding("utf8");
    output.on("data", (chunk: string) => {
      buffer += chunk;
      for (let end = buffer.indexOf("\0"); end >= 0; end = buffer.indexOf("\0")) {
        const line = buffer.slice(0, end);
        buffer = buffer.slice(end + 1);
        this.dispatch(JSON.parse(line) as Message);
      }
    });
    input.on("error", () => {});
    this.exited = new Promise((resolve) => {
      child.once("exit", () => resolve());
      child.once("error", (error) => {
        this.fail(new BrowserError("browser_unavailable", `could not start the browser: ${error.message}`));
        resolve();
      });
    });
    child.once("exit", (code, signal) => this.fail(new BrowserError("browser_exited", `the browser exited (${signal ?? code})`)));
  }

  static launch(options: LaunchOptions): Browser {
    const root = mkdtempSync(join(options.tmpRoot ?? tmpdir(), "corral-browser-"));
    const home = join(root, "home");
    mkdirSync(home);
    const child = spawn(options.executable, [...ARGS, `--user-data-dir=${join(root, "profile")}`, "about:blank"], {
      stdio: ["ignore", "ignore", "ignore", "pipe", "pipe"],
      detached: true,
      env: {
        PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: home, XDG_CONFIG_HOME: join(home, ".config"),
        XDG_CACHE_HOME: join(home, ".cache"), XDG_DATA_HOME: join(home, ".local/share"), LANG: "C.UTF-8",
      },
    });
    const browser = new Browser(child, root, child.stdio[3] as Writable, child.stdio[4] as Readable);
    const abort = () => browser.fail(options.signal.reason instanceof Error ? options.signal.reason : new Error(String(options.signal.reason)));
    if (options.signal.aborted) abort();
    else options.signal.addEventListener("abort", abort, { once: true });
    return browser;
  }

  private dispatch(message: Message): void {
    if (message.id !== undefined) {
      const waiter = this.pending.get(message.id);
      this.pending.delete(message.id);
      if (message.error) waiter?.reject(new BrowserError("browser_protocol", message.error.message ?? "DevTools error"));
      else waiter?.resolve(message.result ?? {});
      return;
    }
    const key = `${message.sessionId ?? ""}:${message.method ?? ""}`;
    const waiters = this.events.get(key);
    this.events.delete(key);
    for (const waiter of waiters ?? []) waiter.resolve(message.params ?? {});
  }

  private fail(error: Error): void {
    this.failure ??= error;
    for (const waiter of this.pending.values()) waiter.reject(this.failure);
    for (const waiters of this.events.values()) for (const waiter of waiters) waiter.reject(this.failure);
    this.pending.clear();
    this.events.clear();
    this.kill();
  }

  send(method: string, params: Params = {}, sessionId?: string): Promise<Params> {
    if (this.failure) return Promise.reject(this.failure);
    const id = this.next++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.input.write(`${JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })}\0`);
    });
  }

  /** Resolves on the next `method` event of `sessionId`; register before triggering it. */
  nextEvent(method: string, sessionId: string): Promise<Params> {
    if (this.failure) return Promise.reject(this.failure);
    const key = `${sessionId}:${method}`;
    return new Promise((resolve, reject) => {
      const waiters = this.events.get(key) ?? new Set<Waiter>();
      waiters.add({ resolve, reject });
      this.events.set(key, waiters);
    });
  }

  private kill(): void {
    const pid = this.child.pid;
    if (pid === undefined || this.child.exitCode !== null || this.child.signalCode !== null) return;
    try {
      process.kill(-pid, "SIGKILL");
    } catch {
      this.child.kill("SIGKILL");
    }
  }

  /** Kills the browser's process group, waits for its exit and removes the temporary profile. */
  async close(): Promise<void> {
    this.fail(new BrowserError("browser_closed", "the browser call finished"));
    await this.exited;
    rmSync(this.root, { recursive: true, force: true });
  }
}
