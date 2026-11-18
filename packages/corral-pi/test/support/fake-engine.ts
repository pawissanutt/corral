import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

interface EngineReply {
  readonly result?: unknown;
  readonly stdout?: string;
  readonly stderr?: string;
  readonly code?: number;
}

export interface FakeEngine {
  readonly path: string;
  /** The complete argv of each invocation, including global flags. */
  calls(): string[][];
}

/** An executable engine fixture. Reply keys are a command ("done") or subcommand ("artifact add"). */
export function writeFakeEngine(directory: string, replies: Readonly<Record<string, EngineReply>> = {}): FakeEngine {
  const path = join(directory, "corral-next");
  const log = join(directory, "engine-argv.jsonl");
  writeFileSync(path, `#!${process.execPath}
import { appendFileSync } from "node:fs";
const argv = process.argv.slice(2);
appendFileSync(${JSON.stringify(log)}, JSON.stringify(argv) + "\\n");
let i = 0;
while (i < argv.length) {
  if (argv[i] === "--root") i += 2;
  else if (argv[i] === "--json") i++;
  else break;
}
const command = argv[i];
const subcommand = command + " " + argv[i + 1];
const replies = ${JSON.stringify(replies)};
const reply = Object.hasOwn(replies, subcommand) ? replies[subcommand]
  : Object.hasOwn(replies, command) ? replies[command] : {};
if (reply.stderr !== undefined) process.stderr.write(reply.stderr);
if (reply.stdout !== undefined) process.stdout.write(reply.stdout);
else if ((reply.code ?? 0) === 0) {
  process.stdout.write(JSON.stringify(reply.result ?? (command === "list" || subcommand === "artifact list" ? [] : {})) + "\\n");
}
process.exitCode = reply.code ?? 0;
`, { mode: 0o700 });
  return { path, calls: () => existsSync(log)
    ? readFileSync(log, "utf8").trimEnd().split("\n").map((line) => JSON.parse(line) as string[]) : [] };
}
