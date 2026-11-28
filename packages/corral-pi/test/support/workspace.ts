import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { closeSync, existsSync, openSync, readSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { PACKAGE_ROOT } from "./fixture-install.ts";
import { within } from "./wait.ts";

export const REPO = join(PACKAGE_ROOT, "../..");

export interface Ran {
  readonly status: number | null;
  readonly stdout: string;
  readonly stderr: string;
}

export function run(argv: string[], options: { cwd?: string; env?: NodeJS.ProcessEnv; timeout?: number } = {}): Ran {
  const [command, ...args] = argv;
  if (!command) throw new Error("empty argv");
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? REPO, env: options.env ?? process.env, stdio: ["ignore", "pipe", "pipe"],
    encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: options.timeout, killSignal: "SIGKILL",
  });
  return { status: result.status, stdout: result.stdout ?? "", stderr: result.stderr ?? "" };
}

export function ok(argv: string[], options?: Parameters<typeof run>[1]): string {
  const result = run(argv, options);
  if (result.status !== 0) throw new Error(`${argv.join(" ")} exited ${result.status}: ${result.stderr.slice(-2000)}`);
  return result.stdout;
}

export const sha256 = (data: string | Buffer) => createHash("sha256").update(data).digest("hex");

export interface StoreEvent {
  readonly kind: string;
  readonly task?: string;
  readonly detail?: Record<string, unknown>;
}

/**
 * Follows events.jsonl. `tail -F` from the construction-time size is the notification source: it
 * follows the name across replacement and pushes every appended byte through a pipe, where Bun's
 * `fs.watch` drops notifications under load. Each push drains the file itself, so the reads, partial
 * lines and inode handling do not depend on what tail printed.
 */
export class EventFollower {
  private offset: number;
  private carry = Buffer.alloc(0);
  private readonly seen: StoreEvent[] = [];
  malformed = 0;
  private readonly waiters: { match: (event: StoreEvent) => boolean; resolve: (event: StoreEvent) => void }[] = [];
  private readonly tail: ChildProcess;

  private inode: number;

  constructor(private readonly path: string,
    private readonly readChunk: (fd: number, buffer: Buffer, offset: number, length: number, position: number) => number = readSync) {
    const stat = statSync(path);
    this.offset = stat.size;
    this.inode = stat.ino;
    this.tail = spawn("tail", ["-c", `+${stat.size + 1}`, "-F", path], { stdio: ["ignore", "pipe", "ignore"] });
    this.tail.stdout?.on("data", () => this.drain());
  }

  private drain(): void {
    if (!existsSync(this.path)) return;
    const stat = statSync(this.path);
    if (stat.ino !== this.inode || stat.size < this.offset) {
      this.inode = stat.ino;
      this.offset = 0;
      this.carry = Buffer.alloc(0);
    }
    const size = stat.size;
    if (size === this.offset) return;
    const buffer = Buffer.alloc(size - this.offset);
    const fd = openSync(this.path, "r");
    let read = 0;
    try {
      while (read < buffer.length) {
        const count = this.readChunk(fd, buffer, read, buffer.length - read, this.offset + read);
        if (count === 0) break;
        read += count;
      }
    } finally {
      closeSync(fd);
    }
    this.offset += read;
    let bytes = Buffer.concat([this.carry, buffer.subarray(0, read)]);
    const lines: string[] = [];
    for (let end = bytes.indexOf(0x0a); end !== -1; end = bytes.indexOf(0x0a)) {
      lines.push(bytes.subarray(0, end).toString("utf8"));
      bytes = bytes.subarray(end + 1);
    }
    this.carry = Buffer.from(bytes);
    for (const line of lines.filter(Boolean)) {
      let event: StoreEvent;
      try {
        event = JSON.parse(line) as StoreEvent;
      } catch {
        this.malformed++;
        continue;
      }
      this.seen.push(event);
      for (const waiter of [...this.waiters]) {
        if (waiter.match(event)) {
          this.waiters.splice(this.waiters.indexOf(waiter), 1);
          waiter.resolve(event);
        }
      }
    }
  }

  next(match: (event: StoreEvent) => boolean, ms = 90_000, what = "a store event"): Promise<StoreEvent> {
    const prior = this.seen.find(match);
    if (prior) return Promise.resolve(prior);
    const pending = new Promise<StoreEvent>((resolve) => this.waiters.push({ match, resolve }));
    this.drain();
    return within(pending, ms, what);
  }

  close(): void {
    this.tail.kill();
  }
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
export function secretLeaks(
  roots: readonly string[], extra: Record<string, string>, secrets: Secrets,
  read: (path: string) => Buffer = readFileSync,
): string[] {
  const leaks: string[] = [];
  const check = (where: string, text: string) => {
    for (const secret of secrets.plain) if (text.includes(secret)) leaks.push(`${where}: plain secret`);
    for (const token of text.match(/[0-9a-f]{64}/g) ?? []) {
      if (secrets.digests.includes(sha256(token))) leaks.push(`${where}: preimage of a stored digest`);
    }
  };
  for (const root of roots) for (const path of files(root)) {
    try { check(path, read(path).toString("latin1")); }
    catch (error) {
      // Live store-holder records may disappear after enumeration. Other I/O failures still fail QA.
      if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
    }
  }
  for (const [where, text] of Object.entries(extra)) check(where, text);
  return leaks;
}
