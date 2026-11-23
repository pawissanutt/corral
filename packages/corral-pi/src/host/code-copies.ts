import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { z } from "zod";
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

function copyEntries(source: readonly SessionEntry[], step: () => void = () => {}): SessionEntry[] {
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
        let ancestor = entry.firstKeptEntryId === original.id ? null : original.parentId;
        while (ancestor !== null) {
          step();
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

// Pi's persisted types are exported, but there is no SDK runtime JSONL schema.
// Validate the fields consumed by Pi and retain the original JSON after checking it.
const s = z.string(), n = z.number(), b = z.boolean();
const optS = s.optional(), optN = n.optional(), optB = b.optional();
const obj = z.custom<Record<string, unknown>>(value => value !== null && typeof value === "object" && !Array.isArray(value));
const text = z.object({ type: z.literal("text"), text: s, textSignature: optS });
const image = z.object({ type: z.literal("image"), mimeType: s, data: s });
const thinking = z.object({ type: z.literal("thinking"), thinking: s, thinkingSignature: optS, redacted: optB });
const call = z.object({ type: z.literal("toolCall"), id: s, name: s, arguments: obj, thoughtSignature: optS, namespace: optS, argumentsError: optS });
const visual = z.array(z.discriminatedUnion("type", [text, image]));
const editable = {
  user: z.union([s, visual]), assistant: z.array(z.discriminatedUnion("type", [text, thinking, call])),
  toolResult: visual, custom: z.union([s, visual]),
};
const usage = z.object({ input: n, output: n, cacheRead: n, cacheWrite: n, cacheWrite1h: optN, reasoning: optN, totalTokens: n,
  cost: z.object({ input: n, output: n, cacheRead: n, cacheWrite: n, total: n, totalFromProvider: optB }),
});
const tool = z.object({ name: s, description: s, parameters: obj, constrainedSampling: z.union([
  z.literal(false), z.object({ type: z.literal("json_schema"), strict: z.enum(["prefer", "require"]) }),
  z.object({ type: z.literal("grammar"), variants: z.record(s, s) }),
]).optional() });
const system = z.object({ role: z.literal("system"), timestamp: n, content: z.union([s, z.array(text)]),
  sections: z.record(s, s.nullable()).optional(), toolsAdded: z.array(tool).optional(), toolsRemoved: z.array(z.object({ name: s })).optional(),
});
const messageSchemas = new Map<string, z.ZodType>(Object.entries({
  user: z.object({ timestamp: n, content: editable.user }), system,
  assistant: z.object({ timestamp: n, content: editable.assistant, api: s, provider: s, model: s, usage,
    responseModel: optS, responseId: optS, providerThinkingLevel: optS, errorMessage: optS, rawStopReason: optS,
    durationMs: optN, endTurn: optB, thinkingLevel: z.enum(["off", "minimal", "low", "medium", "high", "xhigh", "max"]).optional(),
    stopReason: z.enum(["pending", "stop", "length", "toolUse", "error", "aborted", "deferred"]),
    deferred: z.object({ provider: s, modelId: s, api: s, id: s, expiresAt: optN, pollAfterMs: optN }).optional(),
    diagnostics: z.array(z.object({ type: s, timestamp: n, details: obj.optional(), error: z.object({
      name: optS, message: optS, stack: optS, code: z.union([s, n]).optional(),
    }).optional() })).optional(),
  }),
  toolResult: z.object({ timestamp: n, content: editable.toolResult, toolCallId: s, toolName: s, isError: b,
    durationMs: optN, usage: usage.optional(), nestedCalls: z.object({ complete: b, calls: z.array(z.object({
      id: s, name: s, status: z.enum(["ok", "error", "unfinished"]), arguments: obj.optional(),
      argumentsBytes: optN, durationMs: optN, error: optS,
    })) }).optional(),
  }),
  bashExecution: z.object({ timestamp: n, command: s, output: s, cancelled: b, truncated: b, exitCode: optN, excludeFromContext: optB }),
}));
const entrySchemas = new Map<string, z.ZodType>(Object.entries({
  compaction: z.object({ summary: s, tokensBefore: n, firstKeptEntryId: s, systemMessage: system.optional(), fromHook: optB, usage: usage.optional() }),
  branch_summary: z.object({ fromId: s, summary: s, fromHook: optB, usage: usage.optional() }),
  model_change: z.object({ provider: s, modelId: s }),
  thinking_level_change: z.object({ thinkingLevel: s }),
  label: z.object({ targetId: s, label: optS }),
}));

// Bound even opaque JSON (e.g. tool arguments), which Pi may stringify recursively.
function boundedJson(value: unknown, spend: () => boolean, depth = 0): boolean {
  if (!spend() || depth > 64 || (typeof value === "number" && !Number.isFinite(value))) return false;
  if (Array.isArray(value)) return value.length <= 16_384 && value.every(v => boundedJson(v, spend, depth + 1));
  if (value !== null && typeof value === "object") {
    for (const key of Object.keys(value)) if (!boundedJson((value as Record<string, unknown>)[key], spend, depth + 1)) return false;
  }
  return true;
}

export async function readCodeImportSeed(path: string): Promise<SessionEntry[]> {
  // SessionManager.open can rewrite legacy sessions; migrate only a private parse.
  const raw = await readImportFile(path);
  const invalid = (reason: string) => new Error(`Invalid Pi session JSONL: ${reason}`);
  const parsed: Record<string, unknown>[] = [];
  let nodes = 1_000_000;
  const spend = () => --nodes >= 0;
  const isTimestamp = (value: unknown) => typeof value === "string" && Number.isFinite(Date.parse(value));
  for (let offset = 0, lineNumber = 1; offset < raw.length; lineNumber++) {
    const newline = raw.indexOf("\n", offset);
    const end = newline < 0 ? raw.length : newline;
    const line = raw.slice(offset, end).trim();
    offset = end + 1;
    if (!line) continue;
    let entry: unknown;
    try { entry = JSON.parse(line); } catch { throw invalid(`line ${lineNumber} is not JSON`); }
    if (!entry || typeof entry !== "object" || typeof (entry as Record<string, unknown>).type !== "string") {
      throw invalid(`line ${lineNumber} is not a session entry`);
    }
    if (parsed.length >= 20_001 || !boundedJson(entry, spend)) throw invalid(`line ${lineNumber} exceeds JSON value limits`);
    parsed.push(entry as Record<string, unknown>);
  }
  const [header, ...body] = parsed;
  const version = header?.version === undefined ? 1 : header.version;
  if (header?.type !== "session" || typeof header.id !== "string" || typeof header.cwd !== "string"
    || !isTimestamp(header.timestamp) || typeof version !== "number" || !Number.isInteger(version)
    || version < 1 || version > CURRENT_SESSION_VERSION) throw invalid("missing or unsupported session header");
  if (body.some(entry => entry.type === "session" || !isTimestamp(entry.timestamp))) {
    throw invalid("invalid entry or duplicate header");
  }
  const fileEntries = parsed as unknown as FileEntry[];
  migrateSessionEntries(fileEntries);

  const seen = new Map<string, SessionEntry>();
  const entries = fileEntries.slice(1) as SessionEntry[];
  // Validation and ID remapping must not do quadratic work on hostile trees.
  let budget = 1_000_000;
  const step = () => { if (--budget < 0) throw invalid("entry ancestry work limit exceeded"); };
  for (const entry of entries) {
    if (typeof entry.id !== "string" || !entry.id || seen.has(entry.id)
      || (entry.parentId !== null && !seen.has(entry.parentId))) throw invalid("invalid entry ancestry");
    const ancestor = (target: string) => {
      if (!seen.has(target)) return false;
      for (let id = entry.parentId; id !== null; id = seen.get(id)!.parentId) {
        step();
        if (id === target) return true;
      }
      return false;
    };
    const bad = () => invalid(`invalid ${entry.type} payload`);
    const schema = entrySchemas.get(entry.type);
    if (schema && !schema.safeParse(entry).success) throw bad();
    switch (entry.type) {
      case "message": {
        if (!entry.message || typeof entry.message.role !== "string") throw bad();
        const schema = messageSchemas.get(entry.message.role);
        if (schema && !schema.safeParse(entry.message).success) throw bad();
        break;
      }
      case "context_edit": {
        if (typeof entry.targetId !== "string" || !ancestor(entry.targetId)) throw bad();
        const target = seen.get(entry.targetId);
        const role = target?.type === "custom_message" ? "custom" : target?.type === "message" ? target.message.role : undefined;
        if (role !== "user" && role !== "assistant" && role !== "toolResult" && role !== "custom") throw bad();
        if (entry.replacement !== null && (!obj.safeParse(entry.replacement).success
          || !editable[role].safeParse(entry.replacement?.content).success)) throw bad();
        break;
      }
      case "label": if (!seen.has(entry.targetId)) throw bad(); break;
      case "compaction": if (entry.firstKeptEntryId !== entry.id && !ancestor(entry.firstKeptEntryId)) throw bad(); break;
    }
    seen.set(entry.id, entry);
  }
  return copyEntries(entries, step);
}
