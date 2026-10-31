// Test-only adapters. Canonical Store APIs provide the payloads; no v1 mirrors or SQL.
import { spawn, type ChildProcess } from "node:child_process";
import { within } from "./wait.ts";
import { ok, probeBinary, run, type StoreEvent } from "./workspace.ts";

export interface CanonicalEvent extends StoreEvent { readonly seq: number }
const TEST = "agent::bridge::fixture_store_probe::canonical_store_probe";
const PREFIX = "CORRAL_STORE_PROBE ";
let binary: string | undefined;
function probe(): string { return binary ??= probeBinary(); }
function env(root: string, mode: string, extra: Record<string, unknown> = {}): NodeJS.ProcessEnv {
  return { ...process.env, CORRAL_FIXTURE_STORE_PROBE: JSON.stringify({ root, mode, ...extra }) };
}
interface Snapshot { readonly launch_exits?: string; readonly result?: string; readonly text?: string; readonly messages?: StoreMessage[]; readonly entries?: StoreEntry[]; readonly envelopes?: StoreEnvelope[] }
function snapshot(root: string, mode: string, extra: Record<string, unknown> = {}): Snapshot {
  const result = run([probe(), "--ignored", "--exact", TEST, "--nocapture"], { env: env(root, mode, extra) });
  if (result.status !== 0) throw new Error(`canonical fixture probe: ${result.stderr}\n${result.stdout}`);
  const frame = result.stdout.split("\n").find(line => line.startsWith(PREFIX));
  if (!frame) throw new Error(`canonical fixture probe returned no frame: ${result.stdout}`);
  return JSON.parse(frame.slice(PREFIX.length)) as Snapshot;
}
export function launchExitDir(root: string): string {
  const path = snapshot(root, "paths").launch_exits;
  if (!path) throw new Error("canonical probe returned no launch-exit path");
  return path;
}
export function gateResult(root: string, task: string, run: string): string {
  const path = snapshot(root, "gate", { task, run }).result;
  if (!path) throw new Error("canonical probe returned no gate result path");
  return path;
}
export function artifactText(root: string, reference: string): string {
  const text = snapshot(root, "artifact", { reference }).text;
  if (text === undefined) throw new Error("canonical probe returned no artifact text");
  return text;
}
export interface StoreMessage { readonly seq: number; readonly kind: string; readonly body: string; readonly subject?: Record<string, unknown> }
export interface StoreReceipt { readonly via: string; readonly detail: Record<string, unknown>; readonly binding: { readonly session: string } }
interface StoreEntry { readonly message: { readonly seq: number }; readonly observed?: StoreReceipt | null; readonly admitted?: StoreReceipt | null }
export interface StoreEnvelope { readonly number: number; readonly members: number[]; readonly idempotency_key: string; readonly adapter: string; readonly payload: string; readonly payload_sha256: string; readonly binding: { readonly session: string; readonly endpoint?: string; readonly transcript?: string } }
export function inboxEnvelopes(root: string, recipient: string): StoreEnvelope[] {
  const envelopes = snapshot(root, "inbox", { recipient }).envelopes;
  if (!envelopes) throw new Error("canonical probe returned no envelopes");
  return envelopes;
}
export function inboxReceipt(root: string, recipient: string, seq: number, level: "observed" | "admitted" = "observed"): StoreReceipt | undefined {
  return snapshot(root, "inbox", { recipient }).entries?.find(e => e.message.seq === seq)?.[level] ?? undefined;
}
export function requireReceipt(root: string, recipient: string, seq: number, level: "observed" | "admitted" = "observed"): StoreReceipt {
  const receipt = inboxReceipt(root, recipient, seq, level);
  if (!receipt) throw new Error(`missing ${level} receipt for ${recipient}#${seq}`);
  return receipt;
}
export function inboxMessages(root: string, recipient: string): StoreMessage[] {
  const messages = snapshot(root, "inbox", { recipient }).messages;
  if (!messages) throw new Error("canonical probe returned no inbox messages");
  return messages;
}
export function storeEvents(corral: string, root: string): CanonicalEvent[] {
  return ok([corral, "--root", root, "export", "events", "--json"]).trim().split("\n").filter(Boolean).map(line => JSON.parse(line) as CanonicalEvent);
}

/** Captures a canonical sequence after subscribing and acknowledges it before any action.
 * Per-command writer-close notifications wake the real Store reader; no sleeps or polling.
 * The legacy raw-file follower remains independently tested for partial reads/replacements.
 */
export class StoreEventFollower {
  private readonly child: ChildProcess;
  private readonly seen: CanonicalEvent[] = [];
  private readonly waiters: { match: (e: CanonicalEvent) => boolean; resolve: (e: CanonicalEvent) => void; reject: (e: Error) => void }[] = [];
  private carry = Buffer.alloc(0);
  private stopped = false;
  private failure?: Error;
  private readyResolve!: () => void;
  private readyReject!: (error: Error) => void;
  private readonly ready: Promise<void>;
  cursor = 0;
  malformed = 0;

  static async open(root: string): Promise<StoreEventFollower> {
    const follower = new StoreEventFollower(root);
    try { await within(follower.ready, 90_000, "canonical event subscription"); return follower; }
    catch (error) { follower.close(); throw error; }
  }
  private constructor(root: string) {
    this.ready = new Promise((resolve, reject) => { this.readyResolve = resolve; this.readyReject = reject; });
    this.child = spawn(probe(), ["--ignored", "--exact", TEST, "--nocapture"], {
      env: env(root, "watch"), stdio: ["ignore", "pipe", "pipe"],
    });
    let stderr = "";
    this.child.stderr?.on("data", chunk => { stderr = (stderr + chunk).slice(-8000); });
    this.child.stdout?.on("data", chunk => this.frames(chunk));
    this.child.on("error", error => this.fail(error));
    this.child.on("exit", (code, signal) => {
      if (!this.stopped) this.fail(new Error(`canonical event probe exited ${code}/${signal}: ${stderr}`));
    });
  }
  private fail(error: Error): void {
    this.failure = error; this.readyReject(error);
    for (const waiter of this.waiters.splice(0)) waiter.reject(error);
  }
  private frames(chunk: Buffer): void {
    this.carry = Buffer.concat([this.carry, chunk]);
    for (let end = this.carry.indexOf(0x0a); end !== -1; end = this.carry.indexOf(0x0a)) {
      const line = this.carry.subarray(0, end).toString("utf8");
      this.carry = Buffer.from(this.carry.subarray(end + 1));
      if (!line.startsWith(PREFIX)) continue; // Rust test-harness framing, not event input.
      try {
        const frame = JSON.parse(line.slice(PREFIX.length)) as { ready?: number; event?: CanonicalEvent };
        if (typeof frame.ready === "number") { this.cursor = frame.ready; this.readyResolve(); }
        else if (frame.event) {
          const event = frame.event;
          if (event.seq <= this.cursor) throw new Error("non-increasing canonical event sequence");
          this.cursor = event.seq; this.seen.push(event);
          for (const waiter of [...this.waiters]) if (waiter.match(event)) {
            this.waiters.splice(this.waiters.indexOf(waiter), 1); waiter.resolve(event);
          }
        } else throw new Error("unknown canonical event frame");
      } catch (error) { this.malformed++; this.fail(error instanceof Error ? error : new Error(String(error))); }
    }
  }
  next(match: (event: CanonicalEvent) => boolean, ms = 90_000, what = "a store event"): Promise<CanonicalEvent> {
    if (this.failure) return Promise.reject(this.failure);
    const prior = this.seen.find(match);
    if (prior) return Promise.resolve(prior);
    const pending = new Promise<CanonicalEvent>((resolve, reject) => this.waiters.push({ match, resolve, reject }));
    return within(pending, ms, what);
  }
  close(): void { this.stopped = true; this.child.kill(); }
}
