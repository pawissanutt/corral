// A stand-in `corral` executable for host tests: it records each call's argv, stdin and
// environment keys, and answers `agent call` with one committed receipt line.
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export interface FakeCall {
  readonly argv: string[];
  readonly stdin: string;
  readonly envKeys: string[];
  readonly envelope: {
    op: string; request_id: string; payload_sha256: string; payload: Record<string, unknown>;
    identity: Record<string, unknown>; auth: { host_cap: string; bind_nonce?: string };
  };
}

export interface FakeCorral {
  readonly path: string;
  calls(): FakeCall[];
  failWith(code: number): void;
  snapshot(value: unknown): void;
  /** Results merged, in order, into the next `turn_ended` receipts (corral's request decision). */
  turnEnded(results: readonly Record<string, unknown>[]): void;
}

const SCRIPT = `#!/usr/bin/env bun
import { appendFileSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
const dir = dirname(process.argv[1]);
const stdin = readFileSync(0, "utf8");
if (process.argv[2] === "run" && process.argv[3] === "inspect") {
  const inspection = join(dir, "inspection.json");
  if (!existsSync(inspection)) { process.stderr.write("No inspection supplied in this fixture\\n"); process.exit(1); }
  process.stdout.write(readFileSync(inspection, "utf8")); process.exit(0);
}
const reading = process.argv[2] === "show" || process.argv[3] === "state"
  || (process.argv[2] === "assistant" && process.argv[3] === "show");
if (!reading) appendFileSync(join(dir, "calls.jsonl"), JSON.stringify({ argv: process.argv.slice(2), stdin,
  envKeys: Object.keys(process.env) }) + "\\n");
const fail = join(dir, "fail");
if (existsSync(fail)) { process.stderr.write("fake corral refused\\n"); process.exit(Number(readFileSync(fail, "utf8"))); }
const snapshot = join(dir, "snapshot.json");
if (reading) {
  process.stdout.write(readFileSync(snapshot, "utf8"));
  process.exit(0);
}
const envelope = JSON.parse(stdin);
let asked = 0;
if (envelope.op === "ask") {
  const counter = join(dir, "asks");
  asked = (existsSync(counter) ? Number(readFileSync(counter, "utf8")) : 0) + 1;
  writeFileSync(counter, String(asked));
}
let ended = {};
const endings = join(dir, "turn-ended.json");
if (envelope.op === "event" && envelope.payload.kind === "turn_ended" && existsSync(endings)) {
  const queue = JSON.parse(readFileSync(endings, "utf8"));
  ended = queue.shift() ?? {};
  writeFileSync(endings, JSON.stringify(queue));
}
let progress;
if (envelope.op === "todo" && existsSync(snapshot)) {
  const state = JSON.parse(readFileSync(snapshot, "utf8"));
  progress = state.container?.todo ?? state.container?.state?.todo
    ?? state.runs?.find(run => run.id === envelope.identity.run)?.progress;
  // A session ledger's init opens the next plan at the next revision, as corral does.
  const holder = state.container?.state ?? state.container;
  if (holder && envelope.payload.op === "init") {
    const plan = "P" + (Number(String(progress?.plan ?? "P0").slice(1)) + 1);
    progress = { revision: (progress?.revision ?? 0) + 1, spec_revision: "spec", plan,
      phases: envelope.payload.phases.map(phase => ({ ...phase,
        steps: phase.steps.map(step => ({ ...step, status: { state: "pending" }, criteria: [] })) })) };
    holder.todo = progress;
    writeFileSync(snapshot, JSON.stringify(state));
  }
  if (progress && envelope.payload.op === "done") {
    const step = progress.phases.flatMap(phase => phase.steps).find(step => step.id === envelope.payload.step);
    if (step) {
      step.status = { state: "done" };
      progress.revision++;
      writeFileSync(snapshot, JSON.stringify(state));
    }
  }
}
process.stdout.write(JSON.stringify({ version: 1, request_id: envelope.request_id, op: envelope.op,
  payload_sha256: envelope.payload_sha256, identity: envelope.identity, state: "committed",
  result: { op: envelope.op, ...ended, ...(progress ? { revision: progress.revision, progress } : {}),
    ...(envelope.payload.kind === "budget" ? { admitted: true, exhausted: false } : {}),
    ...(envelope.payload.kind === "auxiliary" ? { session_id: envelope.payload.request,
      session_file: join(dir, "auxiliary-" + envelope.payload.request + ".jsonl") } : {}),
    ...(envelope.op === "hello" ? { herdr_seq_epoch_ms: Date.now(), questions: [] } : {}), ...(envelope.op === "ask"
    ? { key: envelope.identity.task + "/" + envelope.identity.run + "/Q" + asked, revision: 1 } : {}) },
  at: new Date().toISOString() }) + "\\n");
`;

export function writeFakeCorral(dir: string): FakeCorral {
  const path = join(dir, "corral");
  writeFileSync(path, SCRIPT, { mode: 0o700 });
  chmodSync(path, 0o700);
  const log = join(dir, "calls.jsonl");
  return {
    path,
    calls: () => {
      if (!existsSync(log)) return [];
      // A concurrent append can be visible before its newline; only parse committed lines.
      const lines = readFileSync(log, "utf8").split("\n");
      lines.pop();
      return lines.map((line) => {
        const call = JSON.parse(line) as Omit<FakeCall, "envelope">;
        return { ...call, envelope: JSON.parse(call.stdin) as FakeCall["envelope"] };
      });
    },
    failWith: (code) => writeFileSync(join(dir, "fail"), String(code)),
    snapshot: (value) => writeFileSync(join(dir, "snapshot.json"), JSON.stringify(value)),
    turnEnded: (results) => writeFileSync(join(dir, "turn-ended.json"), JSON.stringify(results)),
  };
}
