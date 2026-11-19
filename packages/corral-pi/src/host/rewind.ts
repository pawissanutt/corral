import { appendFileSync, existsSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import type { AgentSession, SessionManager } from "@earendil-works/pi-coding-agent";
import { BridgeRefusal, type RewindInput } from "../bridge/protocol.ts";
import { CHECKPOINT_ENTRY, type CheckpointRecord, type Checkpoints, type RewindPoint } from "../capabilities/checkpoints.ts";

interface RewindRecord {
  readonly turn: number;
  readonly checkpoint: string;
  readonly rescue_ref: string;
  readonly from_head: string;
  readonly head: string;
  readonly at?: string;
}

type RewindCheckpoint = CheckpointRecord & { readonly at?: string };

interface RewindTarget {
  readonly entry: string;
  readonly point: RewindPoint;
}

function checkpoints(manager: SessionManager) {
  return manager.getBranch().flatMap(entry => entry.type === "custom" && entry.customType === CHECKPOINT_ENTRY
    ? [{ entry: entry.id, point: entry.data as CheckpointRecord, at: entry.timestamp }] : []);
}

function historyFile(file: string): string { return `${file}.rewinds`; }

export function listRewinds(manager: SessionManager): { checkpoints: RewindCheckpoint[]; rewinds: RewindRecord[] } {
  const file = manager.getSessionFile();
  const history = file && historyFile(file);
  const rewinds: RewindRecord[] = history && existsSync(history)
    ? readFileSync(history, "utf8").trimEnd().split("\n").filter(Boolean).map(line => JSON.parse(line)) : [];
  return {
    checkpoints: checkpoints(manager).map(({ point, at }) => ({
      ...point, ...(typeof at === "string" ? { at } : {}),
    })),
    rewinds,
  };
}

export function rewindPoint(manager: SessionManager, turn: number): RewindTarget {
  const target = checkpoints(manager).findLast(({ point }) => point.turn === turn && point.phase === "start");
  const point = target?.point;
  if (!target || !point?.commit || !point.head) {
    throw new BridgeRefusal("checkpoint_unavailable", `turn ${turn} has no saved start checkpoint; use /rewind --list`);
  }
  return { entry: target.entry, point: { ...point, commit: point.commit, head: point.head } };
}

/** Restore Git only after the retained transcript is ready to replace the old one. */
export function restoreRewind(session: AgentSession, snapshots: Checkpoints,
  { entry, point }: RewindTarget) {
  const manager = session.sessionManager;
  const file = manager.getSessionFile();
  const header = manager.getHeader();
  if (!file || !header) throw new Error("no saved Pi transcript to rewind");
  const cut = `${file}.${crypto.randomUUID()}.tmp`;
  try {
    const retained = [header, ...manager.getBranch(entry)].map(item => JSON.stringify(item)).join("\n");
    writeFileSync(cut, `${retained}\n`, { flag: "wx", mode: 0o600 });
    const rescue = snapshots.restore(point);
    const result = { turn: point.turn, checkpoint: point.id, rescue_ref: rescue.ref,
      from_head: rescue.head, head: point.head, at: new Date().toISOString() };
    try {
      renameSync(cut, file);
      manager.setSessionFile(file);
      session.refreshContext();
    } catch (error) {
      throw new Error(`rewind restored the worktree but could not reload the transcript; saved work is at ${rescue.ref}: ${String(error)}`);
    }
    return result;
  } finally {
    rmSync(cut, { force: true });
  }
}

export function recordRewind(file: string, rewind: RewindRecord): void {
  appendFileSync(historyFile(file), `${JSON.stringify(rewind)}\n`, { mode: 0o600 });
}

export function parseRewindCommand(args: string): RewindInput {
  const value = args.trim();
  if (value === "--list") return { list: true };
  const turn = Number(value);
  if (/^\d+$/.test(value) && Number.isSafeInteger(turn) && turn > 0) return { turn };
  throw new Error("Usage: /rewind <turn> or /rewind --list");
}
