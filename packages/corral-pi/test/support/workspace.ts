// Helpers for the cross-lane fixture: the real corral binaries built from this checkout, a
// throwaway workspace, an events.jsonl follower and a secret scan.
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, existsSync, openSync, readSync, readdirSync, readFileSync, realpathSync, statSync, watch } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

import { PACKAGE_ROOT } from "./fixture-install.ts";
import { within } from "./wait.ts";

export const REPO = join(PACKAGE_ROOT, "../..");

export interface Ran {
  readonly status: number | null;
  readonly stdout: string;
  readonly stderr: string;
}

export function run(argv: string[], options: { cwd?: string; env?: NodeJS.ProcessEnv } = {}): Ran {
  const [command, ...args] = argv;
  if (!command) throw new Error("empty argv");
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? REPO, env: options.env ?? process.env, stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8", maxBuffer: 64 * 1024 * 1024,
  });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

export function ok(argv: string[], options?: Parameters<typeof run>[1]): string {
  const result = run(argv, options);
  if (result.status !== 0) throw new Error(`${argv.join(" ")} exited ${result.status}: ${result.stderr.slice(-2000)}`);
  return result.stdout;
}

/** The private root scripts/test-herdr.sh made (HOME is `<root>/home`), never the owner's. */
export function privateRoot(): string | undefined {
  const home = process.env.HOME;
  if (!home || basename(home) !== "home") return undefined;
  const root = dirname(home);
  return /^corral-herdr-[A-Za-z0-9]+$/.test(basename(root)) && existsSync(join(root, "server.log")) ? root : undefined;
}

export function privateHerdr(): boolean {
  return privateRoot() !== undefined && process.env.CORRAL_HERDR_SESSION === "corral-test"
    && run(["herdr", "--session", "corral-test", "workspace", "list"]).status === 0;
}

interface Artifact {
  readonly reason: string;
  readonly target?: { name: string; kind: string[] };
  readonly profile?: { test: boolean };
  readonly executable?: string | null;
}

function artifacts(args: string[]): Artifact[] {
  return ok(["cargo", ...args, "--message-format=json"])
    .split("\n").filter((line) => line.startsWith("{")).map((line) => JSON.parse(line) as Artifact)
    .filter((entry) => entry.reason === "compiler-artifact" && entry.executable);
}

/** The corral binary as corral's own test builds make it: `hermetic-harness` never probes omp/omo/claude. */
export function corralBinary(): string {
  const built = artifacts(["build", "-p", "corral", "--bin", "corral", "--features", "corral-ops/hermetic-harness"])
    .find((entry) => entry.target?.name === "corral" && entry.target.kind.includes("bin"));
  if (!built?.executable) throw new Error("cargo built no corral binary");
  return built.executable;
}

/** The corral-ops unit-test binary, whose ignored `fresh_process_probe` is a fresh corral process. */
export function probeBinary(): string {
  const built = artifacts(["test", "-p", "corral-ops", "--lib", "--no-run"])
    .find((entry) => entry.target?.name === "corral_ops" && entry.profile?.test);
  if (!built?.executable) throw new Error("cargo built no corral-ops test binary");
  return built.executable;
}

export const sha256 = (data: string | Buffer) => createHash("sha256").update(data).digest("hex");

/** `agent::launch::generation_dir` for a worker run. */
export function generationDir(workspace: string, task: string, run: string, generation: number): string {
  const runtime = process.env.XDG_RUNTIME_DIR ? join(process.env.XDG_RUNTIME_DIR, "corral")
    : join(tmpdir(), `corral-${process.getuid?.()}`);
  const store = `st-${sha256(realpathSync(join(workspace, ".corral"))).slice(0, 16)}`;
  return join(runtime, store, sha256(`${task}_${run}`).slice(0, 16), `g${generation}`);
}

export interface StoreEvent {
  readonly kind: string;
  readonly task?: string;
  readonly detail?: Record<string, unknown>;
}

/** Follows `.corral/events.jsonl` from its current end; `next` waits for a matching event. */
export class EventFollower {
  private offset: number;
  private carry = "";
  private readonly seen: StoreEvent[] = [];
  private readonly waiters: { match: (event: StoreEvent) => boolean; resolve: (event: StoreEvent) => void }[] = [];
  private readonly watcher;

  constructor(private readonly path: string) {
    this.offset = statSync(path).size;
    this.watcher = watch(path, () => this.drain());
  }

  private drain(): void {
    const size = statSync(this.path).size;
    if (size < this.offset) this.offset = 0;
    if (size === this.offset) return;
    const buffer = Buffer.alloc(size - this.offset);
    const fd = openSync(this.path, "r");
    try {
      readSync(fd, buffer, 0, buffer.length, this.offset);
    } finally {
      closeSync(fd);
    }
    this.offset = size;
    const lines = (this.carry + buffer.toString("utf8")).split("\n");
    this.carry = lines.pop() ?? "";
    for (const line of lines.filter(Boolean)) {
      const event = JSON.parse(line) as StoreEvent;
      this.seen.push(event);
      for (const waiter of [...this.waiters]) {
        if (waiter.match(event)) {
          this.waiters.splice(this.waiters.indexOf(waiter), 1);
          waiter.resolve(event);
        }
      }
    }
  }

  next(match: (event: StoreEvent) => boolean, ms = 90_000): Promise<StoreEvent> {
    const prior = this.seen.find(match);
    if (prior) return Promise.resolve(prior);
    const pending = new Promise<StoreEvent>((resolve) => this.waiters.push({ match, resolve }));
    this.drain();
    return within(pending, ms, "a store event");
  }

  close(): void {
    this.watcher.close();
  }
}

/** Waits for `name` to appear in `dir` (which must exist), subscribing before checking. */
export function fileAppears(dir: string, name: string, ms = 90_000): Promise<void> {
  let watcher: ReturnType<typeof watch> | undefined;
  const appeared = new Promise<void>((resolve) => {
    watcher = watch(dir, () => {
      if (existsSync(join(dir, name))) resolve();
    });
    if (existsSync(join(dir, name))) resolve();
  });
  return within(appeared, ms, `${name} in ${dir}`).finally(() => watcher?.close());
}

function files(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? files(path) : entry.isFile() ? [path] : [];
  });
}

export interface Secrets {
  readonly plain: readonly string[];
  readonly digests: readonly string[];
}

/** Every place under `roots` (and every extra text) holding a secret or a preimage of a digest. */
export function secretLeaks(roots: readonly string[], extra: Record<string, string>, secrets: Secrets): string[] {
  const leaks: string[] = [];
  const check = (where: string, text: string) => {
    for (const secret of secrets.plain) if (text.includes(secret)) leaks.push(`${where}: plain secret`);
    for (const token of text.match(/[0-9a-f]{64}/g) ?? []) {
      if (secrets.digests.includes(sha256(token))) leaks.push(`${where}: preimage of a stored digest`);
    }
  };
  for (const root of roots) for (const path of files(root)) check(path, readFileSync(path).toString("latin1"));
  for (const [where, text] of Object.entries(extra)) check(where, text);
  return leaks;
}
