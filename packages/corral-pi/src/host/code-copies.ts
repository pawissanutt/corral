import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { open } from "node:fs/promises";
import {
  CURRENT_SESSION_VERSION, migrateSessionEntries,
  type FileEntry, type SessionEntry, type SessionManager,
} from "@earendil-works/pi-coding-agent";

type CopySource = Pick<SessionManager, "getEntries" | "getEntry" | "getBranch">;

function userText(entry: SessionEntry): string | undefined {
  if (entry.type !== "message" || entry.message.role !== "user") return;
  const content = entry.message.content;
  return typeof content === "string" ? content : content.flatMap(part => part.type === "text" ? [part.text] : []).join("");
}

export function getCodeForkChoices(manager: CopySource): { id: string; text: string }[] {
  // Like Pi, offer user messages on inactive branches too, if there is text to edit.
  return manager.getEntries().flatMap(entry => {
    const text = userText(entry);
    return text ? [{ id: entry.id, text }] : [];
  });
}

export function getCodeForkSeed(manager: CopySource, entryId: string): { seed: SessionEntry[]; selectedText: string } {
  const entry = manager.getEntry(entryId);
  const selectedText = entry && userText(entry);
  if (!entry || !selectedText) throw new Error("Select a user message with text to fork before it.");
  return { seed: copyEntries(entry.parentId === null ? [] : manager.getBranch(entry.parentId)), selectedText };
}

export function getCodeCloneSeed(manager: CopySource): SessionEntry[] {
  return copyEntries(manager.getBranch());
}

function shouldCopy(entry: SessionEntry, idMap: ReadonlyMap<string, string>): boolean {
  switch (entry.type) {
    case "message":
      return ["user", "assistant", "toolResult", "bashExecution", "system"].includes(entry.message.role);
    case "context_edit":
    case "label":
      return idMap.has(entry.targetId);
    case "compaction":
    case "branch_summary":
    case "model_change":
    case "thinking_level_change":
      return true;
    default:
      // Extensions can hydrate host state on resume. Never seed their entries.
      return false;
  }
}

function copyEntries(source: readonly SessionEntry[]): SessionEntry[] {
  const ids = new Map<string, string>();
  const nearest = new Map<string, string | null>();
  const originals = new Map(source.map(entry => [entry.id, entry]));
  const result: SessionEntry[] = [];
  for (const original of source) {
    const parentId = original.parentId === null ? null : nearest.get(original.parentId) ?? null;
    if (!shouldCopy(original, ids)) {
      nearest.set(original.id, parentId);
      continue;
    }
    const entry = structuredClone(original);
    entry.id = randomUUID();
    entry.parentId = parentId;
    ids.set(original.id, entry.id);
    nearest.set(original.id, entry.id);
    switch (entry.type) {
      case "context_edit":
      case "label":
        entry.targetId = ids.get(entry.targetId)!;
        break;
      case "compaction": {
        delete entry.details;
        // If the old boundary was filtered, move forward on this branch to the
        // next retained entry. The compaction itself means retain none.
        let firstKept = entry.id;
        let ancestor = original.parentId;
        while (ancestor !== null) {
          const ancestorId = ids.get(ancestor);
          if (ancestorId) firstKept = ancestorId;
          if (ancestor === entry.firstKeptEntryId) break;
          ancestor = originals.get(ancestor)?.parentId ?? null;
        }
        entry.firstKeptEntryId = ancestor !== null ? firstKept : entry.id;
        break;
      }
      case "branch_summary":
        delete entry.details;
        // The summarized branch may not be in the copy; this field is provenance only.
        entry.fromId = ids.get(entry.fromId) ?? entry.id;
        break;
      case "message":
        if (entry.message.role === "bashExecution") delete entry.message.fullOutputPath;
        if (entry.message.role === "toolResult") delete entry.message.details;
        break;
    }
    result.push(entry);
  }
  const last = source.at(-1);
  if (last && result.length) {
    const active = nearest.get(last.id) ?? null;
    if (result.at(-1)?.id !== active) {
      // Pi reopens at the last on-disk entry. A filtered leaf can otherwise
      // make a sibling branch active. A fresh nameless entry anchors the leaf.
      result.push({ type: "session_info", id: randomUUID(), parentId: active, timestamp: last.timestamp });
    }
  }
  return result;
}

const IMPORT_BYTE_LIMIT = 32 * 1024 * 1024;

async function readImportFile(path: string): Promise<string> {
  // Open nonblocking before checking the actual target, including through symlinks.
  const file = await open(path, constants.O_RDONLY | constants.O_NONBLOCK);
  try {
    const stat = await file.stat();
    if (!stat.isFile()) throw new Error("Pi session import requires a regular file");
    const tooLarge = () => new Error("Pi session import exceeds the 32 MiB limit");
    if (stat.size > IMPORT_BYTE_LIMIT) throw tooLarge();
    let buffer = Buffer.allocUnsafe(Math.max(4096, stat.size + 1));
    let total = 0;
    while (true) {
      if (total === buffer.length) {
        const grown = Buffer.allocUnsafe(Math.min(buffer.length * 2, IMPORT_BYTE_LIMIT + 1));
        buffer.copy(grown);
        buffer = grown;
      }
      const { bytesRead } = await file.read(buffer, total, buffer.length - total, null);
      if (!bytesRead) return buffer.toString("utf8", 0, total);
      total += bytesRead;
      if (total > IMPORT_BYTE_LIMIT) throw tooLarge();
    }
  } finally {
    await file.close();
  }
}

export async function readCodeImportSeed(path: string): Promise<SessionEntry[]> {
  // SessionManager.open can rewrite legacy sessions; migrate only a private parse.
  const raw = (await readImportFile(path)).trim().split(/\r?\n/);
  const invalid = (reason: string) => new Error(`Invalid Pi session JSONL: ${reason}`);
  const parsed: Record<string, unknown>[] = [];
  for (const [index, line] of raw.entries()) {
    if (!line.trim()) continue;
    let entry: unknown;
    try { entry = JSON.parse(line); } catch { throw invalid(`line ${index + 1} is not JSON`); }
    if (!entry || typeof entry !== "object" || typeof (entry as Record<string, unknown>).type !== "string") {
      throw invalid(`line ${index + 1} is not a session entry`);
    }
    parsed.push(entry as Record<string, unknown>);
  }
  const [header, ...body] = parsed;
  const version = header?.version ?? 1;
  if (header?.type !== "session" || typeof header.id !== "string" || typeof header.cwd !== "string"
    || typeof header.timestamp !== "string" || typeof version !== "number" || !Number.isInteger(version)
    || version < 1 || version > CURRENT_SESSION_VERSION) throw invalid("missing or unsupported session header");
  if (body.some(entry => entry.type === "session" || typeof entry.timestamp !== "string")) {
    throw invalid("invalid entry or duplicate header");
  }
  const fileEntries = parsed as unknown as FileEntry[];
  migrateSessionEntries(fileEntries);

  const seen = new Set<string>();
  const entries = fileEntries.slice(1) as SessionEntry[];
  for (const entry of entries) {
    if (typeof entry.id !== "string" || !entry.id || seen.has(entry.id)
      || (entry.parentId !== null && !seen.has(entry.parentId))) throw invalid("invalid entry ancestry");
    if (entry.type === "message" && (!entry.message || typeof entry.message.role !== "string"
      || (entry.message.role === "user" && typeof entry.message.content !== "string" && !Array.isArray(entry.message.content))
      || (["assistant", "toolResult"].includes(entry.message.role) && !Array.isArray((entry.message as { content?: unknown }).content)))) {
      throw invalid("invalid message");
    }
    seen.add(entry.id);
  }
  return copyEntries(entries);
}
