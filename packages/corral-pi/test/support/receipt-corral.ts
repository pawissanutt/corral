// A stand-in `corral` executable that keeps canonical receipts the way corral does for these tests:
// one stored receipt per request ID (a repeat with the same op and digest replays it, another
// digest is refused), refusals store nothing, owner inputs are journaled, and receipt_lookup reads
// the store. Per-op rules simulate a refusal, a confirm-class action or an acknowledgement lost
// after commit. Every envelope and every applied effect is logged.
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { PLANNING_FIXTURE } from "./planning-state.ts";

export interface Envelope {
  readonly op: string;
  readonly request_id: string;
  readonly payload_sha256: string;
  readonly payload: Record<string, unknown>;
  readonly identity: Record<string, unknown> & { parent?: { tool_call_id: string; cell: string; call: string } };
}

export interface Effect {
  readonly op: string;
  readonly request_id: string;
  readonly payload: Record<string, unknown>;
  readonly parent: Envelope["identity"]["parent"] | null;
}

export interface Rule {
  /** Applies only to payloads whose listed fields equal these values. */
  readonly match?: Record<string, unknown>;
  readonly reject?: { readonly code: string; readonly reason: string };
  /** Commit, then exit without a response, as a killed executor after its transaction would. */
  readonly loseAck?: boolean;
  /** Canonical backend outcomes returned to real host adapters by inspector regressions. */
  readonly receipt?: { readonly state: "committed" | "uncertain"; readonly result: Record<string, unknown>; readonly replayed?: boolean };
  /** Refuse with confirmation_required until the owner's answer is journaled for the request. */
  readonly confirm?: boolean;
}

export interface ReceiptCorral {
  readonly path: string;
  calls(): Envelope[];
  effects(): Effect[];
  rules(rules: Record<string, Rule | readonly Rule[]>): void;
  snapshot(value: unknown): void;
}

const SCRIPT = `#!/usr/bin/env bun
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
const dir = dirname(process.argv[1]);
${PLANNING_FIXTURE}
const json = (name, fallback) => existsSync(join(dir, name)) ? JSON.parse(readFileSync(join(dir, name), "utf8")) : fallback;
const stdin = readFileSync(0, "utf8");
if (process.argv[2] !== "agent" || process.argv[3] !== "call") {
  process.stdout.write(JSON.stringify(process.argv[2] === "assistant" ? planningFixtureRead() : json("snapshot.json", {})));
  process.exit(0);
}
const envelope = JSON.parse(stdin);
appendFileSync(join(dir, "calls.jsonl"), JSON.stringify(envelope) + "\\n");
const at = new Date().toISOString();
const base = { version: 1, request_id: envelope.request_id, op: envelope.op, payload_sha256: envelope.payload_sha256,
  identity: envelope.identity, at };
const emit = (value) => { process.stdout.write(JSON.stringify(value) + "\\n"); process.exit(0); };
const store = join(dir, "receipts");
mkdirSync(store, { recursive: true });
const stored = (id) => existsSync(join(store, id + ".json")) ? JSON.parse(readFileSync(join(store, id + ".json"), "utf8")) : undefined;
if (envelope.op === "receipt_lookup") {
  const target = envelope.payload.request_id;
  const found = stored(target);
  const lookup = { by: envelope.request_id, request_id: target, found: found !== undefined };
  emit(found ? { ...found, lookup } : { ...base, state: "committed", stored: false, result: { found: false }, lookup });
}
if (envelope.op === "event" && envelope.payload.kind === "owner_input") {
  const counter = join(dir, "inputs");
  const n = (existsSync(counter) ? Number(readFileSync(counter, "utf8")) : 0) + 1;
  writeFileSync(counter, String(n));
  if (envelope.payload.answers && envelope.payload.text === "y") appendFileSync(join(dir, "answers"), envelope.payload.answers + "\\n");
  emit({ ...base, state: "committed", result: { kind: "owner_input", input: "A-1/g1/i" + n } });
}
if (envelope.op === "assistant_plan" && envelope.payload.action === "read") {
  emit({ ...base, state: "committed", result: planningFixtureRead().workflow });
}
if (envelope.op === "event" && envelope.payload.kind === "assistant_boundary") {
  emit({ ...base, state: "committed", result: planningFixtureEvent(envelope) });
}
if (envelope.op === "hello" || envelope.op === "event") {
  emit({ ...base, state: "committed", result: { op: envelope.op, kind: envelope.payload.kind,
    ...(envelope.op === "hello" ? { herdr_seq_epoch_ms: Date.now(), questions: [] } : {}),
    ...(envelope.payload.kind === "budget" ? { admitted: true, exhausted: false } : {}) } });
}
const previous = stored(envelope.request_id);
if (previous) {
  if (previous.op !== envelope.op || previous.payload_sha256 !== envelope.payload_sha256) {
    emit({ ...base, state: "rejected", result: { code: "payload_mismatch", reason: "the request ID names another request" } });
  }
  emit({ ...previous, replayed: true });
}
const rule = [json("rules.json", {})[envelope.op] ?? []].flat()
  .find((r) => Object.entries(r.match ?? {}).every(([k, v]) => envelope.payload[k] === v)) ?? {};
const answered = existsSync(join(dir, "answers")) && readFileSync(join(dir, "answers"), "utf8").split("\\n").includes(envelope.request_id);
if (rule.confirm && !answered) {
  emit({ ...base, state: "rejected", result: { code: "confirmation_required", reason: "the owner must confirm",
    command: "corral " + envelope.op + " " + String(envelope.payload.target ?? envelope.payload.kind ?? "") } });
}
if (rule.reject) emit({ ...base, state: "rejected", result: rule.reject });
const receipt = { ...base, state: "committed", result: { op: envelope.op, outcome: "applied",
  ...(envelope.op === "ask" ? { key: envelope.identity.task + "/" + envelope.identity.run + "/Q-" + envelope.request_id.slice(3, 9), revision: 1 } : {}) }, ...rule.receipt };
try {
  writeFileSync(join(store, envelope.request_id + ".json"), JSON.stringify(receipt), { flag: "wx" });
} catch {
  emit({ ...stored(envelope.request_id), replayed: true });
}
if (receipt.state === "committed" && receipt.replayed !== true) appendFileSync(join(dir, "effects.jsonl"), JSON.stringify({ op: envelope.op, request_id: envelope.request_id,
  payload: envelope.payload, parent: envelope.identity.parent ?? null }) + "\\n");
if (rule.loseAck) { process.stderr.write("fixture executor lost its acknowledgement after commit\\n"); process.exit(9); }
emit(receipt);
`;

function lines<T>(path: string): T[] {
  if (!existsSync(path)) return [];
  const all = readFileSync(path, "utf8").split("\n");
  all.pop();
  return all.map((line) => JSON.parse(line) as T);
}

export function writeReceiptCorral(dir: string): ReceiptCorral {
  const path = join(dir, "corral");
  writeFileSync(path, SCRIPT, { mode: 0o700 });
  chmodSync(path, 0o700);
  return {
    path,
    calls: () => lines<Envelope>(join(dir, "calls.jsonl")),
    effects: () => lines<Effect>(join(dir, "effects.jsonl")),
    rules: (rules) => writeFileSync(join(dir, "rules.json"), JSON.stringify(rules)),
    snapshot: (value) => writeFileSync(join(dir, "snapshot.json"), JSON.stringify(value)),
  };
}
