import { spawn } from "node:child_process";
import { constants } from "node:os";
import { writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import type { SessionEntry } from "@earendil-works/pi-coding-agent";

export type CodeHandoffRequest = { kind: "new"; seed?: readonly SessionEntry[]; draft?: string } | { kind: "resume"; id: string };
export type CodeHandoff = (request: CodeHandoffRequest) => Promise<void>;

export interface CodeBootstrap { seed?: string; draft?: string }
// The private parent/child IPC channel carries this data, never argv, env or terminal input.
export interface CodeLaunch extends CodeBootstrap {
  cwd: string;
  args: readonly string[];
}

export const codeWorkerFlag = "--internal-code-host";

export function serializeCodeSeed(entries: readonly SessionEntry[], draft = ""): string {
  const checkSize = (size: number) => {
    if (size > 32 * 1024 * 1024) throw new Error("The copied conversation exceeds the 32 MiB handoff limit.");
  };
  let size = Buffer.byteLength(draft);
  checkSize(size);
  return entries.map(entry => {
    const line = JSON.stringify(entry) + "\n";
    size += Buffer.byteLength(line);
    checkSize(size);
    return line;
  }).join("");
}

export function writeCodeSeed(sessionDir: string, cwd: string, version: number, seed: string): string {
  mkdirSync(sessionDir, { recursive: true, mode: 0o700 });
  const id = crypto.randomUUID();
  const file = join(sessionDir, `code-${id}.jsonl`);
  const header = { type: "session", version, id, timestamp: new Date().toISOString(), cwd };
  writeFileSync(file, `${JSON.stringify(header)}\n${seed}`, { flag: "wx", mode: 0o600 });
  return file;
}

export async function receiveCodeBootstrap(): Promise<CodeBootstrap> {
  if (!process.send || !process.connected) throw new Error("The Code host requires its terminal supervisor.");
  return await new Promise((resolve, reject) => {
    const disconnected = () => reject(new Error("The Code supervisor disconnected during launch."));
    process.once("disconnect", disconnected);
    process.once("message", (message: CodeBootstrap) => {
      process.off("disconnect", disconnected);
      resolve(message);
    });
    process.send!({ type: "ready" }, undefined, undefined, error => { if (error) reject(error); });
  });
}

export async function sendCodeLaunch(next: CodeLaunch): Promise<void> {
  if (!process.send || !process.connected) throw new Error("The Code supervisor is no longer connected.");
  await new Promise<void>((resolve, reject) => {
    const disconnected = () => finish(new Error("The Code supervisor disconnected during handoff."));
    const accepted = (message: unknown) => {
      if (message === "handoff accepted") finish();
      else if (message === "handoff refused") finish(new Error("The Code supervisor is stopping."));
    };
    const finish = (error?: Error | null) => {
      process.off("disconnect", disconnected);
      process.off("message", accepted);
      if (error) reject(error); else resolve();
    };
    process.once("disconnect", disconnected);
    process.on("message", accepted);
    process.send!({ type: "handoff", next }, undefined, undefined, error => { if (error) finish(error); });
  });
}

export async function superviseCode(command: readonly string[], first: CodeLaunch): Promise<number> {
  let launch: CodeLaunch | undefined = first;
  while (launch) {
    const current: CodeLaunch = launch;
    let next: CodeLaunch | undefined;
    let stopping = false;
    let receivedReady = false;
    const [file, ...prefix] = command;
    if (!file) throw new Error("No executable for the Code host.");
    const child = spawn(file, [...prefix, codeWorkerFlag, ...current.args], {
      cwd: current.cwd, stdio: ["inherit", "inherit", "inherit", "ipc"], serialization: "json",
    });
    const exited = new Promise<number>((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (code, signal) => resolve(signal ? 128 + constants.signals[signal] : code ?? 2));
    });
    child.on("message", (message: { type?: string; next?: CodeLaunch }) => {
      if (message?.type === "ready" && !receivedReady) {
        receivedReady = true;
        child.send({ seed: current.seed, draft: current.draft }, () => {});
      } else if (message?.type === "handoff") {
        const accepted = !stopping && receivedReady && !next && message.next;
        if (accepted) next = accepted;
        child.send(accepted ? "handoff accepted" : "handoff refused", () => {});
      }
    });
    const signals = ["SIGINT", "SIGTERM", "SIGHUP"] as const;
    const forward = signals.map(signal => () => {
      stopping = true;
      child.kill(signal);
    });
    signals.forEach((signal, i) => process.on(signal, forward[i]!));
    let status: number;
    try { status = await exited; }
    finally { signals.forEach((signal, i) => process.off(signal, forward[i]!)); }
    launch = status === 0 && !stopping ? next : undefined;
    if (!launch) return status;
  }
  return 0;
}
