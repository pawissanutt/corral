import { readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { SessionInput } from "../bridge/protocol.ts";

/** Lives beside the engine's run files (or in the build/chat directory). */
export function readSessionRecord(dir: string): Record<string, unknown> {
  try { return JSON.parse(readFileSync(join(dir, "session.json"), "utf8")) as Record<string, unknown>; }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
}

export class SessionRecord {
  private readonly path: string;
  readonly transcript: string | undefined;
  readonly legacyAnthropicApi: boolean;
  paused: boolean;
  held: SessionInput[];

  constructor(private readonly dir: string, private readonly id: string,
    private readonly role: string, private readonly worktree: string, fresh = false) {
    this.path = join(dir, "session.json");
    const saved = fresh ? {} : readSessionRecord(dir);
    this.transcript = typeof saved.sessionFile === "string" ? saved.sessionFile : undefined;
    const route = saved.route as { provider?: unknown; connection?: unknown } | undefined;
    this.legacyAnthropicApi = role === "build" && route?.provider === "anthropic" && route.connection === "anthropic-api";
    this.paused = saved.paused === true;
    this.held = Array.isArray(saved.held_input) ? saved.held_input as SessionInput[] : [];
  }


  save(sessionFile: string, paused = this.paused, held = this.held): void {
    const temporary = join(this.dir, `.session-${process.pid}.tmp`);
    // Build also stores its checkout and exit metadata in this file; retain its latest values.
    const record = { ...readSessionRecord(this.dir), id: this.id, role: this.role, worktree: this.worktree, sessionFile,
      pid: process.pid, paused, held_input: held };
    try {
      writeFileSync(temporary, `${JSON.stringify(record)}\n`, { mode: 0o600 });
      renameSync(temporary, this.path);
    } finally { rmSync(temporary, { force: true }); }
    this.paused = paused;
    this.held = held;
  }
}
