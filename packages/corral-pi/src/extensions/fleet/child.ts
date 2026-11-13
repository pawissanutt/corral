import { randomUUID } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type * as Sdk from "@earendil-works/pi-coding-agent";
import type { ThinkingLevel } from "@earendil-works/pi-agent-core";
import type { ExtensionAPI, ModelRuntime, ToolDefinition } from "@earendil-works/pi-coding-agent";
import type { BuildContext, ChildQuestion, ChildSpec, ChildUsage } from "../../build/types.ts";
import { resourceLoaderOptions, restrictedSettings } from "../../host/restrict.ts";
import { classifyBuildWrite } from "../../roles/build/guard.ts";
import type { HelperEvent } from "../subagent/inspection.ts";
import { observeHelperEvent } from "../subagent/session.ts";
import type { ResolvedModel } from "./profiles.ts";
import { Type } from "typebox";

export interface ChildHooks {
  onEvent(event: HelperEvent): void;
  onToolStart(name: string, args: unknown): void;
  onTurn(): void;
  ask(question: ChildQuestion): Promise<string>;
}
export interface ChildRunner {
  run(spec: ChildSpec, model: ResolvedModel, hooks: ChildHooks, signal: AbortSignal): ChildHandle;
}
export interface ChildHandle {
  readonly done: Promise<{ report: string; failed?: string }>;
  steer(text: string): Promise<void>;
  setModel(provider: string, model: string, effort?: string): Promise<"applied" | "deferred">;
  usage(): ChildUsage;
}
export interface SdkRunnerOptions {
  readonly sdk: typeof Sdk;
  readonly models: ModelRuntime;
  readonly context: BuildContext;
  readonly agentDir: string;
  readonly localTools: readonly ToolDefinition[];
  readonly shellRefusal?: (command: string) => string | undefined;
  readonly now?: () => number;
}

const BASE_TOOLS = ["read", "edit", "write", "bash", "grep", "find", "ls"];
const DELEGATION: Record<string, true> = { task: true, agent_message: true, agent_cancel: true, explore: true, oracle: true, corral_expert: true, subagent: true, delegate: true };
const SYSTEM = "You are a write-capable build agent working one slice in a worktree shared with other agents. "
  + "Do only the supplied brief. Stay inside your owned files; if an outside edit was necessary, say so in your report. "
  + "Do not overwrite other agents' work. No Git writes and no delegation. "
  + "Finish with a short report of what changed, what was checked with each command and result, and what was not checked.";

// Inspect command words, including quoted shell bodies, without executing shell expansion.
function gitWrite(command: string): boolean {
  const words = command.replace(/\\\n/g, "").replace(/["']/g, "").replace(/\\([^\s])/g, "$1").split(/\s+|[;&|()<>`]/).filter(Boolean);
  for (let i = 0; i < words.length; i++) {
    if (!/(^|\/)git$/.test(words[i]!)) continue;
    let j = i + 1;
    while (words[j]?.startsWith("-")) {
      const option = words[j++]!;
      if (["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env"].includes(option)) j++;
      if (option === "--") break;
    }
    const verb = words[j];
    if (verb && ["commit", "checkout", "switch", "stash", "reset", "rebase", "merge", "cherry-pick", "worktree", "push"].includes(verb)) return true;
    if (verb === "branch" && words.slice(j + 1).some(word => /^-[a-zA-Z]*[dD]/.test(word) || word === "--delete")) return true;
  }
  return false;
}
function thinking(effort?: string): ThinkingLevel {
  if (effort === undefined) return "off";
  if (!["off", "minimal", "low", "medium", "high", "xhigh", "max"].includes(effort)) throw new Error(`Unknown child effort: ${effort}`);
  return effort as ThinkingLevel;
}

export function createSdkRunner(options: SdkRunnerOptions): ChildRunner {
  return { run(spec, initialModel, hooks, signal) {
    const { sdk } = options;
    const cwd = options.context.session.worktree;
    const allowed = new Set([...BASE_TOOLS, ...options.localTools.filter(tool => !DELEGATION[tool.name]).map(tool => tool.name), "ask_owner"]);
    let session: Sdk.AgentSession | undefined;
    let pending: ResolvedModel | undefined;
    let turns = 0;
    let complete = false;
    let cached: ChildUsage = { input: 0, output: 0, turns: 0 };
    const usage = (): ChildUsage => {
      if (session && !complete) {
        const stats = session.getSessionStats();
        cached = { input: stats.tokens.input, output: stats.tokens.output, cost: stats.cost,
          contextPercent: session.getContextUsage()?.percent ?? undefined, turns };
      }
      return { ...cached };
    };
    const apply = async (choice: ResolvedModel) => {
      const model = options.models.getModel(choice.provider, choice.model);
      if (!model) throw new Error(`model_unavailable: ${choice.provider}/${choice.model}`);
      await session!.setModel(model);
      session!.setThinkingLevel(thinking(choice.effort));
    };
    const { promise: ready, resolve: readyResolve, reject: readyReject } = Promise.withResolvers<void>();
    // Setup errors also belong to done; avoid an unobserved rejection when nobody steers.
    void ready.catch(() => {});
    const done = (async () => {
      let unsubscribe = () => {};
      const abort = () => { if (session) void session.abort().catch(() => {}); };
      try {
        signal.throwIfAborted();
        const model = options.models.getModel(initialModel.provider, initialModel.model);
        if (!model) throw new Error(`model_unavailable: ${initialModel.provider}/${initialModel.model}`);
        const level = thinking(initialModel.effort);
        const extension = (pi: ExtensionAPI) => {
          for (const tool of options.localTools) if (allowed.has(tool.name) && tool.name !== "ask_owner") pi.registerTool(tool);
          pi.registerTool({ name: "ask_owner", label: "Ask the owner", description: "Ask the owner one blocking question with two to four concrete options.",
            parameters: Type.Object({ question: Type.String({ minLength: 1 }), options: Type.Array(Type.Object({ label: Type.String({ minLength: 1 }), detail: Type.Optional(Type.String()) }), { minItems: 2, maxItems: 4 }) }),
            async execute(_id, params: { question: string; options: { label: string; detail?: string }[] }) {
              const answer = await hooks.ask({ ...params, askedAt: (options.now ?? Date.now)() });
              return { content: [{ type: "text", text: answer }], details: {} };
            } });
          pi.on("tool_call", event => {
            let reason: string | undefined;
            if (!allowed.has(event.toolName) || DELEGATION[event.toolName] || event.parentToolCallId !== undefined) reason = "Build children cannot delegate or invoke tools outside their loadout";
            else if (event.toolName === "edit" || event.toolName === "write") {
              const path = event.input.path;
              if (typeof path !== "string") reason = "Build writes require a path";
              else { const decision = classifyBuildWrite(path, cwd); if (!decision.allowed) reason = decision.reason; }
            } else if (event.toolName === "bash") {
              const command = event.input.command;
              reason = typeof command !== "string" ? "Bash requires a command" : options.shellRefusal?.(command) ?? (gitWrite(command) ? "Build children cannot run Git write commands" : undefined);
            }
            if (reason) return { block: true, reason };
            // SDK tool_execution_start precedes its blocking hook, so count starts only here.
            hooks.onToolStart(event.toolName, event.input);
            return undefined;
          });
          pi.on("turn_start", () => { turns++; hooks.onTurn(); });
          // Await the mutation before the SDK constructs the following model request.
          pi.on("turn_end", async () => { const choice = pending; pending = undefined; if (choice) await apply(choice); });
        };
        const dir = join(options.context.dir, "children");
        mkdirSync(dir, { recursive: true, mode: 0o700 });
        const file = join(dir, `${spec.id}.jsonl`);
        writeFileSync(file, JSON.stringify({ type: "session", version: sdk.CURRENT_SESSION_VERSION, id: randomUUID(), timestamp: new Date().toISOString(), cwd }) + "\n", { flag: "wx", mode: 0o600 });
        const services = await sdk.createAgentSessionServices({ cwd, agentDir: join(options.agentDir, "fleet"), modelRuntime: options.models,
          settingsManager: sdk.SettingsManager.inMemory({ ...restrictedSettings(), retry: { enabled: false }, compaction: { enabled: false } }),
          resourceLoaderOptions: { ...resourceLoaderOptions([extension]), agentsFilesOverride: () => ({ agentsFiles: [] }), systemPromptOverride: () => SYSTEM } });
        ({ session } = await sdk.createAgentSessionFromServices({ services, sessionManager: sdk.SessionManager.open(file), model,
          tools: [...allowed], thinkingLevel: level, scopedModels: [{ model, thinkingLevel: level }] }));
        unsubscribe = session.subscribe(event => observeHelperEvent(event, hooks.onEvent));
        signal.addEventListener("abort", abort, { once: true });
        signal.throwIfAborted();
        await session.bindExtensions({ mode: "print", abortHandler: abort });
        // Make steering available only after the initial prompt has been submitted.
        const prompting = session.prompt(`${spec.brief}\n\nOwned files (repository-relative):\n${spec.owns.length ? spec.owns.map(path => `- ${path}`).join("\n") : "No explicit file ownership restrictions."}`, { source: "rpc" });
        readyResolve();
        await prompting;
        if (pending && !signal.aborted) { const choice = pending; pending = undefined; await apply(choice); }
        const failed = session.messages.findLast(message => message.role === "assistant" && ["error", "aborted"].includes(message.stopReason));
        return { report: (session.getLastAssistantText() ?? "").slice(0, 32768), ...(failed ? { failed: failed.role === "assistant" ? failed.errorMessage ?? "Child response interrupted" : "Child response interrupted" } : {}) };
      } catch (error) {
        readyReject(error);
        return { report: session?.getLastAssistantText()?.slice(0, 32768) ?? "", failed: error instanceof Error ? error.message : String(error) };
      } finally {
        signal.removeEventListener("abort", abort);
        unsubscribe();
        try { usage(); } finally { complete = true; session?.dispose(); }
      }
    })();
    return { done, usage,
      async steer(text) { await ready; if (complete) throw new Error("Child is no longer running"); await session!.steer(text); },
      async setModel(provider, model, effort) {
        if (!options.models.getModel(provider, model)) throw new Error(`model_unavailable: ${provider}/${model}`);
        thinking(effort);
        await ready;
        if (complete) throw new Error("Child is no longer running");
        const choice = { profile: `${provider}/${model}`, provider, model, effort };
        if (session!.isStreaming) { pending = choice; return "deferred"; }
        await apply(choice); return "applied";
      },
    };
  } };
}
