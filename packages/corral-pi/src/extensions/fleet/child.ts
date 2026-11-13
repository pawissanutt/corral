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

/** Tokenize shell words without expanding them, retaining nested command substitutions separately. */
function shellCommands(source: string): string[][] | undefined {
  const commands: string[][] = [];
  let at = 0;
  const parse = (stop?: ")" | "`"): boolean => {
    let words: string[] = [];
    let word = "";
    let active = false;
    let quote: "'" | '"' | undefined;
    const flushWord = () => { if (active) words.push(word); word = ""; active = false; };
    const flushCommand = () => { flushWord(); if (words.length) commands.push(words); words = []; };
    while (at < source.length) {
      const char = source[at++]!;
      if (quote === "'") {
        if (char === "'") quote = undefined; else word += char;
        continue;
      }
      if (char === "\\") {
        const next = source[at++];
        if (next === undefined) return false;
        if (next !== "\n") {
          word += quote === '"' && !['$', '`', '"', "\\"].includes(next) ? `\\${next}` : next;
          active = true;
        }
        continue;
      }
      if (char === "$" && source[at] === "(") {
        at++;
        if (!parse(")")) return false;
        word += "$()"; active = true;
        continue;
      }
      if (char === "`" && !(stop === "`" && quote === undefined)) {
        if (!parse("`")) return false;
        word += "``"; active = true;
        continue;
      }
      if (quote === '"') {
        if (char === '"') quote = undefined; else word += char;
        continue;
      }
      if (char === stop) { flushCommand(); return true; }
      if (char === "'" || char === '"') { quote = char; active = true; continue; }
      if (char === "(") {
        flushCommand();
        if (!parse(")")) return false;
        continue;
      }
      if (char === ")") return false;
      if (char === "#" && !active) { while (at < source.length && source[at] !== "\n") at++; continue; }
      if (char === "\n" || ";|&<>".includes(char)) { flushCommand(); continue; }
      if (/\s/.test(char)) { flushWord(); continue; }
      word += char; active = true;
    }
    if (quote || stop) return false;
    flushCommand();
    return true;
  };
  return parse() ? commands : undefined;
}

const READ_ONLY_GIT: Record<string, true> = { status: true, diff: true, log: true, show: true, blame: true, grep: true, "ls-files": true, "rev-parse": true, "cat-file": true };
const GIT_VALUE_OPTIONS = ["-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env"];
const GIT_GLOBAL_FLAGS = ["--no-pager", "--paginate", "--literal-pathspecs", "--glob-pathspecs", "--noglob-pathspecs", "--icase-pathspecs", "--no-optional-locks", "--no-replace-objects", "--bare", "--no-lazy-fetch"];

function listingArgs(args: readonly string[], kind: "branch" | "tag"): boolean {
  const patterns = args.includes("--list");
  const flags = ["--list", "--ignore-case", "--column", "--no-column", "--color", "--no-color", ...(kind === "branch" ? ["--all", "--remotes", "--verbose", "--show-current", "--no-abbrev"] : [])];
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (arg === "--") return patterns;
    if (!arg.startsWith("-")) { if (!patterns) return false; continue; }
    if (flags.includes(arg) || (kind === "branch" && /^-[arv]+$/.test(arg)) || /^--(?:color|column)=/.test(arg)) continue;
    if (/^--(?:format|sort|points-at)=/.test(arg)) continue;
    if (["--format", "--sort", "--points-at"].includes(arg)) { if (args[++i] === undefined) return false; continue; }
    if (/^--(?:no-)?(?:contains|merged)(?:=|$)/.test(arg)) {
      if (!arg.includes("=") && args[i + 1] !== undefined && !args[i + 1]!.startsWith("-")) i++;
      continue;
    }
    return false;
  }
  return true;
}

/** Fail closed for unknown verbs, global options and mutating variants of listing commands. */
function readOnlyGit(words: readonly string[]): boolean {
  let at = 0;
  while (words[at]?.startsWith("-")) {
    const option = words[at++]!;
    if (GIT_VALUE_OPTIONS.includes(option)) { if (words[at++] === undefined) return false; continue; }
    if (GIT_VALUE_OPTIONS.some(name => option.startsWith(`${name}=`))
      || /^-(?:C|c).+/.test(option) || GIT_GLOBAL_FLAGS.includes(option)) continue;
    if (["--version", "-v", "--help", "-h"].includes(option)) return at === words.length;
    if (option === "--") break;
    return false;
  }
  const verb = words[at++];
  if (!verb) return false;
  const args = words.slice(at);
  // These switches turn otherwise observational commands into file writes or executable filters.
  const options = args.slice(0, args.indexOf("--") < 0 ? args.length : args.indexOf("--"));
  if (options.some(arg => /^--(?:output|ext-diff|textconv|filters)(?:=|$)/.test(arg)
    || (verb === "grep" && (/^--open-files-in-pager(?:=|$)/.test(arg) || /^-[^-]*O/.test(arg))))) return false;
  if (Object.hasOwn(READ_ONLY_GIT, verb)) return true;
  if (verb === "branch" || verb === "tag") return listingArgs(args, verb);
  if (verb === "worktree") return args[0] === "list" && args.slice(1).every(arg => ["--porcelain", "--verbose", "-v", "-z"].includes(arg));
  if (verb === "stash") return args[0] === "list";
  if (verb === "remote") {
    const remote = args.filter(arg => arg !== "-v" && arg !== "--verbose");
    if (!remote.length) return true;
    if (remote[0] === "show") return remote.includes("-n") && remote.slice(1).every(arg => arg === "-n" || !arg.startsWith("-"));
    return remote[0] === "get-url" && remote.slice(1).every(arg => ["--all", "--push"].includes(arg) || !arg.startsWith("-"));
  }
  if (verb === "config") {
    const list = args.includes("--list");
    const get = args.includes("--get");
    if (list === get) return false;
    let keys = 0;
    for (let i = 0; i < args.length; i++) {
      const arg = args[i]!;
      if (!arg.startsWith("-")) { keys++; continue; }
      if (["--get", "--list", "--global", "--system", "--local", "--worktree", "--includes", "--no-includes", "--show-origin", "--show-scope", "--null", "-z", "--name-only"].includes(arg)) continue;
      if (/^--(?:file|type)=/.test(arg)) continue;
      if (["--file", "-f", "--type"].includes(arg)) { if (args[++i] === undefined) return false; continue; }
      return false;
    }
    return get ? keys === 1 : keys === 0;
  }
  return false;
}

function gitWrite(command: string, depth = 0): boolean {
  if (depth > 16) return true;
  const commands = shellCommands(command);
  if (!commands) return /(?:^|[^a-zA-Z0-9_-])git(?:$|[^a-zA-Z0-9_-])/.test(command);
  for (const words of commands) {
    let at = 0;
    // These shell prefixes do not change which executable follows them.
    while (words[at] && (/^[a-zA-Z_][a-zA-Z0-9_]*=/.test(words[at]!) || ["!", "{", "if", "then", "elif", "else", "do"].includes(words[at]!))) at++;
    while (["env", "command", "builtin", "exec", "sudo", "nohup"].includes(words[at]?.split("/").at(-1) ?? "")) {
      at++;
      while (words[at]?.startsWith("-")) {
        const option = words[at++]!;
        if (["-u", "--unset", "-C", "--chdir", "-g", "--group", "--user"].includes(option)) at++;
        if (option === "--") break;
      }
      while (words[at] && /^[a-zA-Z_][a-zA-Z0-9_]*=/.test(words[at]!)) at++;
    }
    const executable = words[at] ?? "";
    if (/(^|\/)git$/.test(executable) && !readOnlyGit(words.slice(at + 1))) return true;
    if (/(^|\/)(?:ba|da|k|z)?sh$/.test(executable) && /^-[^-]*c/.test(words[at + 1] ?? "")
      && gitWrite(words[at + 2] ?? "", depth + 1)) return true;
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
            } else if (event.toolName === "bash" || event.toolName === "job_start" || Object.hasOwn(event.input, "command")) {
              // Local command tools, including background jobs, share the same child boundary.
              const command = (event.input as Record<string, unknown>).command;
              reason = typeof command !== "string" ? "Shell tools require a command" : options.shellRefusal?.(command) ?? (gitWrite(command) ? "Build children cannot run Git write commands" : undefined);
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
