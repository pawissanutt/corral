// Which Pi session a host generation opens, and what of a resume reaches it (T-252 recovery).
// Generation 1 starts a fresh session in its container's session directory. A resumed
// generation continues the most recent session there, the one the lost generation wrote, so the
// conversation survives and tool-call request IDs (derived from the session ID) stay stable
// across generations. Pi gives the calls the lost host left open synthetic results only in what
// it sends the model; the file is never rewritten, and corral's recovery_notice says what became
// of each of those calls.
//
// That notice (and a later recovery_update) is never written into Pi's session file either: the
// recovery context below keeps only a placeholder line in the prompt Pi records, and puts the
// member's text back in place of it in every model request this host makes.
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ContextEvent, ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";

import type { RoleContext, RoleModule } from "../roles/types.ts";
import type { LoadedModules } from "./modules.ts";

export type SessionSdk = Pick<typeof Sdk, "SessionManager">;

export function openSession(sdk: SessionSdk, cwd: string, sessionDir: string, generation: number): SessionManager {
  return generation > 1
    ? sdk.SessionManager.continueRecent(cwd, sessionDir)
    : sdk.SessionManager.create(cwd, sessionDir);
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
