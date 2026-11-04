// Owner-only write dialogs. No model tool exposes decide, and no authorization is cached.
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { RoleContext } from "../types.ts";
import { canonicalJson, CorralCallError, sha256Hex, toolRequestId } from "../../tools/corral.ts";
import { classifyWrite, scopeCandidates, writeRefusal, type Classification, type PathContext } from "./paths.ts";
import { utf8Prefix } from "./owner-action.ts";

const CHOICES = { once: "Allow once", hour: "Allow for 1 hour", session: "Allow for this session", deny: "Deny" } as const;
type UI = Pick<ExtensionContext, "hasUI"> & {
  readonly ui: Pick<ExtensionContext["ui"], "notify" | "select">;
};
const block = (tag: string, text = "Ask the owner or use a scratch worktree.") => ({
  block: true as const, reason: `assistant rule: ${tag}; nothing was written. ${text}`,
});

export class PermissionBroker {
  private queue: Promise<void> = Promise.resolve();
  private denied = new Set<string>();
  private deniedTurn: number | undefined;
  private ignored = new Set<string>();
  constructor(private readonly context: RoleContext, private readonly paths?: PathContext) {}

  private async classify(path: string): Promise<Classification> {
    const c = this.context;
    return classifyWrite(path, this.paths ?? { workspace: c.workspaceRoot ?? c.cwd, cwd: c.cwd, session: c.identity.assistant ?? "",
      scratch: await c.scratchRoots?.() ?? [],
      enabled: c.capabilities?.enabled.includes("write_permissions") === true,
      settings: c.writePermission?.settings });
  }

  async notice(ctx: UI): Promise<void> {
    const port = this.context.writePermission;
    if (!port) return;
    try {
      const receipt = await port.call(toolRequestId(port.sessionId(), `wp-list:${randomBytes(16).toString("hex")}`), { action: "list" });
      if (receipt.state !== "committed") throw new Error(JSON.stringify(receipt.result));
      const result = receipt.result as { grants?: { id: string; scope: { shown: string }; lifetime: string }[] };
      if (result.grants?.length) ctx.ui.notify(`corral: active write permissions: ${result.grants.map((g) => `${g.id} ${g.scope.shown} (${g.lifetime})`).join(", ")}`, "info");
    } catch (error) { ctx.ui.notify(`corral: write permissions unavailable: ${String(error)}`, "warning"); }
  }

  async authorize(tool: string, input: Record<string, unknown>, toolCall: string, ctx: UI) {
    const path = input.path;
    if (typeof path !== "string") return block("fixed_rule");
    let initial: Classification;
    try { initial = await this.classify(path); } catch { return block("corral_error"); }
    for (const entry of initial.ignored ?? []) {
      if (!this.ignored.has(entry)) { this.ignored.add(entry); ctx.ui.notify(`corral: ignored broad or unresolved write_permissions allow entry ${entry}`, "warning"); }
    }
    if (initial.decision === "refuse") {
      return initial.rule === "capability_off"
        ? { block: true as const, reason: `${writeRefusal(initial.shown, this.context.workspaceRoot ?? this.context.cwd,
          this.context.identity.assistant ?? "")} [capability_off]` }
        : block(initial.rule);
    }
    if (initial.decision === "allow") {
      if (initial.rule === "config") this.context.writePermission?.emit("write_permission_used", { rule: "config", target: initial.shown });
      return undefined;
    }
    const port = this.context.writePermission;
    if (!port) return block("corral_error");
    const turn = port.turn();
    // Serialize the entire check/ask/decide. Queued invocations check only after the previous
    // decision; the uncontended path makes exactly one check, never a redundant second one.
    const previous = this.queue;
    const { promise, resolve: release } = Promise.withResolvers<void>();
    this.queue = promise;
    try {
      await previous;
      if (turn.signal.aborted) return block("aborted");
      if (this.deniedTurn !== turn.id) { this.denied.clear(); this.deniedTurn = turn.id; }
      const current = await this.classify(path);
      if (current.canonical !== initial.canonical || current.rule === "fixed_rule") return block("target_changed");
      if (this.denied.has(current.canonical ?? current.shown)) return block("owner_denied");
      if (current.decision === "refuse") return block(current.rule);
      if (current.decision === "allow") return undefined;
      const target = { shown: current.shown, canonical: current.canonical };
      const call = async (id: string, payload: Record<string, unknown>) => {
        for (let attempt = 0; ; attempt++) {
          try { return await port.call(id, payload); }
          catch (error) {
            if (attempt !== 0 || !(error instanceof CorralCallError) || !["call_timeout", "call_failed"].includes(error.code)) throw error;
          }
        }
      };
      const check = await call(toolRequestId(port.sessionId(), `wp-check:${randomBytes(16).toString("hex")}`), { action: "check", tool, target });
      if (turn.signal.aborted) return block("aborted");
      if (check.state !== "committed") return block("corral_error");
      const checked = check.result as { grant?: string };
      const unchanged = async () => {
        const latest = await this.classify(path);
        return latest.canonical === current.canonical && latest.rule !== "fixed_rule";
      };
      if (checked.grant) {
        if (!await unchanged()) return block("target_changed");
        port.emit("write_permission_used", { rule: "grant", grant: checked.grant });
        return undefined;
      }
      if (!ctx.hasUI) return block("no_ui", "The owner can configure write_permissions.allow directories.");
      const candidates = scopeCandidates(current.shown, this.paths?.home);
      const operation = tool === "edit" ? { kind: "edit", replacements: Array.isArray(input.edits) ? input.edits.length : 1 }
        : { kind: existsSync(current.shown) ? "overwrite" : "new_file" };
      const preview = utf8Prefix((tool === "write" ? String(input.content ?? "") : JSON.stringify(input.edits ?? input)).split("\n").slice(0, 40).join("\n"), 4096);
      const choices = candidates.length ? Object.values(CHOICES) : [CHOICES.once, CHOICES.deny];
      const destination = current.canonical === current.shown ? "" : `\nResolved destination: ${current.canonical}`;
      const answer = await ctx.ui.select(`${tool} ${current.shown}${destination}\n${operation.kind.replace("_", " ")}${"replacements" in operation ? ` (${operation.replacements} replacements)` : ""}\n${preview}`, choices, { signal: turn.signal, timeout: 300_000 });
      if (turn.signal.aborted) return block("aborted");
      const choice = (Object.keys(CHOICES) as (keyof typeof CHOICES)[]).find((key) => CHOICES[key] === answer);
      if (!choice || !choices.includes(CHOICES[choice])) return block("no_answer");
      let scope: { shown: string; canonical: string } | undefined;
      if (choice === "hour" || choice === "session") {
        const label = (c: { shown: string; canonical: string }) => c.shown === c.canonical ? c.shown : `${c.shown} → ${c.canonical}`;
        const selected = await ctx.ui.select("Directory scope", candidates.map(label), { signal: turn.signal, timeout: 300_000 });
        if (turn.signal.aborted) return block("aborted");
        scope = candidates.find((c) => label(c) === selected);
        if (!scope) return block("no_answer");
      }
      if (!await unchanged()) return block("target_changed");
      const generation = this.context.identity.generation;
      const id = randomBytes(16).toString("hex");
      const receipt = await call(toolRequestId(port.sessionId(), `wp:${generation}:${id}`), {
        action: "decide", invocation: { generation, id, tool_call: toolCall }, tool, operation, target,
        digest: sha256Hex(canonicalJson({ tool, target: current.canonical, input })),
        preview_sha256: sha256Hex(preview), choice, ...(scope ? { scope } : {}),
      });
      if (turn.signal.aborted) return block("aborted");
      if (receipt.state !== "committed") {
        const result = receipt.result as { code?: string };
        return block(result?.code === "grants_full" ? "grants_full" : "corral_error");
      }
      if (choice === "deny") { this.denied.add(current.canonical ?? current.shown); return block("owner_denied"); }
      if ((receipt.result as { authorized?: boolean }).authorized !== true) return block("corral_error");
      if (!await unchanged()) return block("target_changed");
      return undefined;
    } catch { return block(turn.signal.aborted ? "aborted" : "corral_error"); }
    finally { release(); }
  }
}
