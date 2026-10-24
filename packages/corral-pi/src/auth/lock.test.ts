// Cross-process exclusion of the credential store (review F1). Every holder is a real child
// process importing the production store; coordination is by exact stdout lines and stdin
// releases with bounded deadlines, never by timing. `corral agent install` never copies this file.
import { afterEach, describe, expect, test } from "bun:test";
import { type ChildProcess, spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { FileCredentialStore } from "./store.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const STORE = JSON.stringify(join(HERE, "store.ts"));
const DEADLINE_MS = 20_000;
const temps: string[] = [];
const children: ChildProcess[] = [];

afterEach(() => {
  for (const child of children.splice(0)) if (child.exitCode === null) child.kill("SIGKILL");
  for (const dir of temps.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function temp(): string {
  const dir = mkdtempSync(join(tmpdir(), "corral-pi-lock-"));
  temps.push(dir);
  return dir;
}

interface Child {
  readonly process: ChildProcess;
  line(text: string): Promise<void>;
  release(): void;
  exited(): Promise<number | null>;
  output(): string;
}

function child(script: string, env: Record<string, string> = {}): Child {
  const proc = spawn(process.execPath, ["--no-env-file", "-e", script], {
    cwd: join(HERE, "../.."),
    env: { PATH: process.env.PATH ?? "", HOME: process.env.HOME ?? "", PI_OFFLINE: "1", PI_TELEMETRY: "0", ...env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  children.push(proc);
  let out = "";
  const waiters: (() => void)[] = [];
  const exit = new Promise<number | null>((resolve) => proc.on("close", resolve));
  const record = (chunk: Buffer) => {
    out += chunk.toString();
    for (const wake of waiters.splice(0)) wake();
  };
  proc.stdout?.on("data", record);
  proc.stderr?.on("data", record);
  void exit.then(() => {
    for (const wake of waiters.splice(0)) wake();
  });
  return {
    process: proc,
    async line(text) {
      const deadline = Date.now() + DEADLINE_MS;
      while (!out.split("\n").includes(text)) {
        if (proc.exitCode !== null) throw new Error(`child exited before ${text}:\n${out}`);
        const remaining = deadline - Date.now();
        if (remaining <= 0) throw new Error(`no ${text} within ${DEADLINE_MS} ms:\n${out}`);
        await new Promise<void>((resolve) => {
          const timer = setTimeout(resolve, remaining);
          waiters.push(() => {
            clearTimeout(timer);
            resolve();
          });
        });
      }
    },
    release() {
      proc.stdin?.write("go\n");
    },
    exited: () => exit,
    output: () => out,
  };
}

const credential = (label: string, expires: string) =>
  `({ type: "oauth", access: "access-${label}", refresh: "refresh-${label}", expires: ${expires}, accountId: "synthetic" })`;

const waitForRelease = `await new Promise((resolve) => process.stdin.once("data", resolve));`;

function stored(dir: string): Record<string, { refresh: string; access: string }> {
  return JSON.parse(readFileSync(join(dir, "auth.json"), "utf8"));
}

describe("the credential lock across processes", () => {
  test("a second writer waits while the holder is inside modify and then sees its credential", async () => {
    // Given process A inside a modify callback, holding the lock:
    const dir = join(temp(), "auth");
    const log = join(dir, "..", "order.log");
    const holder = child(`
      const { appendFileSync } = await import("node:fs");
      const { FileCredentialStore } = await import(${STORE});
      await new FileCredentialStore(${JSON.stringify(dir)}).modify("openai-codex", async () => {
        appendFileSync(${JSON.stringify(log)}, "A-enter\\n");
        console.log("A_IN");
        ${waitForRelease}
        appendFileSync(${JSON.stringify(log)}, "A-exit\\n");
        return ${credential("A", "Date.now() + 3600000")};
      });
      console.log("A_DONE");
      process.exit(0);
    `);
    await holder.line("A_IN");

    // When process B contends for the same credential:
    const contender = child(`
      const { appendFileSync } = await import("node:fs");
      const { FileCredentialStore } = await import(${STORE});
      const store = new FileCredentialStore(${JSON.stringify(dir)}, { onContended: () => console.log("B_CONTENDED") });
      await store.modify("openai-codex", async (current) => {
        appendFileSync(${JSON.stringify(log)}, \`B-enter \${current?.refresh}\\n\`);
        return ${credential("B", "Date.now() + 3600000")};
      });
      console.log("B_DONE");
      process.exit(0);
    `);
    await contender.line("B_CONTENDED");
    holder.release();

    // Then B entered only after A left, saw A's write, and nothing was lost:
    expect(await holder.exited()).toBe(0);
    expect(await contender.exited()).toBe(0);
    expect(readFileSync(log, "utf8")).toBe("A-enter\nA-exit\nB-enter refresh-A\n");
    expect(stored(dir)["openai-codex"]?.refresh).toBe("refresh-B");
    expect(existsSync(join(dir, "auth.json.lock"))).toBeTrue();
  }, 60_000);

  test("a holder killed inside the lock frees it without stale recovery or a lost credential", async () => {
    const dir = join(temp(), "auth");
    const seeded = new FileCredentialStore(dir);
    await seeded.modify("openai-codex", async () => ({
      type: "oauth", access: "access-seed", refresh: "refresh-seed", expires: Date.now() + 3_600_000,
    }));
    const holder = child(`
      const { FileCredentialStore } = await import(${STORE});
      await new FileCredentialStore(${JSON.stringify(dir)}).modify("openai-codex", async () => {
        console.log("A_IN");
        ${waitForRelease}
        return ${credential("A", "Date.now() + 3600000")};
      });
    `);
    await holder.line("A_IN");
    holder.process.kill("SIGKILL");
    await holder.exited();

    let contended = false;
    const after = new FileCredentialStore(dir, { onContended: () => { contended = true; } });
    const result = await after.modify("openai-codex", async (current) => ({ ...current!, access: "access-after" }));
    expect(contended).toBeFalse();
    expect([result?.type, (result as { refresh?: string }).refresh, (result as { access?: string }).access])
      .toEqual(["oauth", "refresh-seed", "access-after"]);
    expect(existsSync(join(dir, "auth.json.lock"))).toBeTrue();
  }, 60_000);

  test("two Pi refresh callers refresh an expired credential exactly once", async () => {
    // Given an expired credential and two hosts that both need a token:
    const root = temp();
    const dir = join(root, "corral/pi-native/auth");
    const seeded = new FileCredentialStore(dir);
    await seeded.modify("openai-codex", async () => ({
      type: "oauth", access: "access-old", refresh: "refresh-old", expires: Date.now() - 1000,
    }));
    const caller = (label: string, hold: boolean, hooks: string) => child(`
      let refreshes = 0;
      globalThis.fetch = async (input, init) => {
        refreshes++;
        const grant = new URLSearchParams(String(init?.body));
        console.log("${label}_REFRESH " + grant.get("refresh_token"));
        ${hold ? waitForRelease : ""}
        const claims = Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "synthetic" } }))
          .toString("base64").replace(/=+$/, "");
        return new Response(JSON.stringify({ access_token: "e30." + claims + ".${label}", refresh_token: "refresh-${label}",
          expires_in: 3600 }), { headers: { "content-type": "application/json" } });
      };
      const { FileCredentialStore } = await import(${STORE});
      const { ModelRuntime } = await import("@earendil-works/pi-coding-agent");
      const store = new FileCredentialStore(${JSON.stringify(dir)}, ${hooks});
      const models = await ModelRuntime.create({ credentials: store, modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
      const resolved = await models.getAuth("openai-codex");
      console.log("${label}_AUTH " + refreshes + " " + JSON.stringify(resolved?.auth));
      process.exit(0);
    `, { XDG_DATA_HOME: root });

    // When A is mid-refresh and B asks for auth at the same time:
    const first = caller("A", true, "{}");
    await first.line("A_REFRESH refresh-old");
    const second = caller("B", false, `{ onContended: () => console.log("B_CONTENDED") }`);
    await second.line("B_CONTENDED");
    first.release();
    expect(await first.exited()).toBe(0);
    expect(await second.exited()).toBe(0);

    // Then only A refreshed, B used A's rotated credential, and the file holds it:
    const authOf = (output: string, label: string) => output.split("\n").find((l) => l.startsWith(`${label}_AUTH `)) ?? "";
    const a = authOf(first.output(), "A").split(" ");
    const b = authOf(second.output(), "B").split(" ");
    expect([a[1], b[1]]).toEqual(["1", "0"]);
    expect(b.slice(2).join(" ")).toBe(a.slice(2).join(" "));
    expect(second.output()).not.toContain("B_REFRESH");
    expect(stored(dir)["openai-codex"]?.refresh).toBe("refresh-A");
  }, 60_000);
});
