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
}

const SCRIPT = `#!/usr/bin/env bun
import { appendFileSync, existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
const dir = dirname(process.argv[1]);
const stdin = readFileSync(0, "utf8");
appendFileSync(join(dir, "calls.jsonl"), JSON.stringify({ argv: process.argv.slice(2), stdin,
  envKeys: Object.keys(process.env) }) + "\\n");
const fail = join(dir, "fail");
if (existsSync(fail)) { process.stderr.write("fake corral refused\\n"); process.exit(Number(readFileSync(fail, "utf8"))); }
const envelope = JSON.parse(stdin);
process.stdout.write(JSON.stringify({ version: 1, request_id: envelope.request_id, op: envelope.op,
  payload_sha256: envelope.payload_sha256, identity: envelope.identity, state: "committed",
  result: { op: envelope.op, ...(envelope.op === "ask"
    ? { key: envelope.identity.task + "/" + envelope.identity.run + "/Q1" } : {}) },
  at: new Date().toISOString() }) + "\\n");
`;

export function writeFakeCorral(dir: string): FakeCorral {
  const path = join(dir, "corral");
  writeFileSync(path, SCRIPT, { mode: 0o700 });
  chmodSync(path, 0o700);
  const log = join(dir, "calls.jsonl");
  return {
    path,
    calls: () => existsSync(log)
      ? readFileSync(log, "utf8").trim().split("\n").map((line) => {
        const call = JSON.parse(line) as Omit<FakeCall, "envelope">;
        return { ...call, envelope: JSON.parse(call.stdin) as FakeCall["envelope"] };
      })
      : [],
    failWith: (code) => writeFileSync(join(dir, "fail"), String(code)),
  };
}
