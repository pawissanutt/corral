// Which Pi session a host generation opens, and what of a resume reaches it (T-252 recovery).
// An explicit resume opens the recorded session, even for generation 1 of a new review child.
// Recovery without an exact recorded path is refused, so the
// conversation survives and tool-call request IDs (derived from the session ID) stay stable
// across generations. Pi gives the calls the lost host left open synthetic results only in what
// it sends the model; the file is never rewritten, and corral's recovery_notice says what became
// of each of those calls.
//
// That notice (and a later recovery_update) is never written into Pi's session file either: the
// recovery context below keeps only a placeholder line in the prompt Pi records, and puts the
// member's text back in place of it in every model request this host makes.
import { openSync, closeSync, fstatSync, readSync } from "node:fs";

import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ContextEvent, ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";

import type { RoleContext, RoleModule } from "../roles/types.ts";
import type { LoadedModules } from "./modules.ts";

export type SessionSdk = Pick<typeof Sdk, "SessionManager" | "CURRENT_SESSION_VERSION">;

export class SessionResumeRefused extends Error {
  constructor(path: string, cause: unknown) {
    super(`session_resume_refused: ${path}: ${cause instanceof Error ? cause.message : String(cause)}`, { cause });
  }
}

export function openSession(
  sdk: SessionSdk, cwd: string, sessionDir: string, generation: number, resumeSession?: string, freshSession = false,
  resumeLeaf?: string,
): SessionManager {
  if (freshSession) return sdk.SessionManager.create(cwd, sessionDir);
  if (resumeSession !== undefined) {
    try {
      // Pi initializes a missing or empty explicit file as a new session. Refuse those
      // cases before opening, and reject incompatible headers before Pi can migrate them.
      const fd = openSync(resumeSession, "r");
      let sessionId: string;
      try {
        if (!fstatSync(fd).isFile()) throw new Error("not a regular session file");
        const bytes = Buffer.alloc(64 * 1024);
        const length = readSync(fd, bytes, 0, bytes.length, 0);
        const first = bytes.subarray(0, length).toString("utf8").split("\n")[0] ?? "";
        const header: unknown = JSON.parse(first);
        if (typeof header !== "object" || header === null || !("type" in header) || header.type !== "session"
          || !("id" in header) || typeof header.id !== "string" || header.id.length === 0
          || !("version" in header) || typeof header.version !== "number"
          || !Number.isSafeInteger(header.version) || header.version < 1 || header.version > sdk.CURRENT_SESSION_VERSION) {
          throw new Error("incompatible Pi session header");
        }
        sessionId = header.id;
      } finally {
        closeSync(fd);
      }
      const session = sdk.SessionManager.open(resumeSession, sessionDir, cwd);
      if (session.getSessionId() !== sessionId) throw new Error("the recorded Pi session was not opened");
      // A rewind continues from its checkpoint's entry; the abandoned turns stay in the file.
      if (resumeLeaf !== undefined) {
        if (!session.getEntry(resumeLeaf)) throw new Error(`the rewind's session entry ${resumeLeaf} is not in the session`);
        session.branch(resumeLeaf);
      }
      return session;
    } catch (error) {
      throw new SessionResumeRefused(resumeSession, error);
    }
  }
  if (generation > 1) throw new SessionResumeRefused(sessionDir, "no exact recorded session path");
  return sdk.SessionManager.create(cwd, sessionDir);
}

/** Inbox message kinds whose text reaches the model as context only. */
export const CONTEXT_ONLY_KINDS: ReadonlySet<string> = new Set(["recovery_notice", "recovery_update"]);

export interface HeldMember {
  /** The line Pi's session records in the member's place. */
  readonly placeholder: string;
  /** The member as corral sent it: its `[corral <key>#<seq> <kind>]` header and body. */
  readonly text: string;
}

/**
 * Splits an inbox envelope (`[corral envelope <key>@env<n>]`, then members each opening with
 * `[corral <key>#<seq> <kind>]`) into the text Pi records and its context-only members, or
 * `undefined` when it carries none.
 */
export function holdContextOnly(text: string): { text: string; held: HeldMember[] } | undefined {
  const head = /^\[corral envelope (\S+)@env\d+\]/.exec(text);
  const key = head?.[1];
  if (!key) return undefined;
  const marker = `\n\n[corral ${key}#`;
  const starts: number[] = [];
  for (let at = text.indexOf(marker); at >= 0; at = text.indexOf(marker, at + marker.length)) starts.push(at);
  const first = starts[0];
  if (first === undefined) return undefined;
  const held: HeldMember[] = [];
  let recorded = text.slice(0, first);
  starts.forEach((start, index) => {
    const member = text.slice(start + 2, starts[index + 1] ?? text.length);
    const header = /^\[corral \S+#(\d+) ([a-z_]+)\]\n/.exec(member);
    const kind = header?.[2];
    if (header && kind && CONTEXT_ONLY_KINDS.has(kind)) {
      const placeholder = `[corral ${key}#${header[1]} ${kind}: given to this host as context only; `
        + "Pi's session keeps this line instead]";
      held.push({ placeholder, text: member });
      recorded += `\n\n${placeholder}`;
    } else {
      recorded += `\n\n${member}`;
    }
  });
  return held.length > 0 ? { text: recorded, held } : undefined;
}

type ContextMessage = ContextEvent["messages"][number];

/**
 * Upstream's `input` transform records each context-only member as its placeholder before Pi
 * writes the prompt, and the `context` event restores the member in every model request. The
 * delivery module has already recorded its corral_delivery entry over the text corral sent.
 */
export class RecoveryContext {
  private readonly held = new Map<string, string>();

  extend(pi: ExtensionAPI): void {
    pi.on("input", (event) => {
      const split = holdContextOnly(event.text);
      if (!split) return { action: "continue" as const };
      for (const member of split.held) this.held.set(member.placeholder, member.text);
      return { action: "transform" as const, text: split.text, ...(event.images ? { images: event.images } : {}) };
    });
    pi.on("context", (event) => {
      if (this.held.size === 0) return undefined;
      return { messages: event.messages.map((message) => this.restore(message)) };
    });
  }

  private expand(text: string): string {
    let out = text;
    for (const [placeholder, member] of this.held) out = out.split(placeholder).join(member);
    return out;
  }

  private restore(message: ContextMessage): ContextMessage {
    if (!("role" in message) || message.role !== "user") return message;
    const content = message.content;
    if (typeof content === "string") {
      const expanded = this.expand(content);
      return expanded === content ? message : { ...message, content: expanded };
    }
    let changed = false;
    const parts = content.map((part) => {
      if (typeof part === "string" || part.type !== "text") return part;
      const expanded = this.expand(part.text);
      if (expanded === part.text) return part;
      changed = true;
      return { ...part, text: expanded };
    });
    return changed ? { ...message, content: parts } : message;
  }
}

/** The loaded modules with the recovery context registered ahead of the role's own extension. */
export function withRecoveryContext(modules: LoadedModules, recovery = new RecoveryContext()): LoadedModules {
  const role = modules.role;
  const localTools = role.localTools?.bind(role);
  const extended: RoleModule = {
    role: role.role,
    upstreamTools: role.upstreamTools,
    corralOps: role.corralOps,
    tools: (context: RoleContext) => role.tools(context),
    ...(role.localToolNames ? { localToolNames: role.localToolNames } : {}),
    ...(localTools ? { localTools } : {}),
    contract: (context: RoleContext) => role.contract(context),
    extend: (pi: ExtensionAPI, context: RoleContext) => {
      recovery.extend(pi);
      role.extend?.(pi, context);
    },
  };
  return { ...modules, role: extended };
}
