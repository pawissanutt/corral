// Flag-based launches read ordinary workspace settings.
import { existsSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { basename, dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { TOML } from "bun";

import type { AwsBinding } from "../auth/aws.ts";
import type { CapabilityGuidance } from "../capabilities/guidance.ts";
import { type ProviderId, isAwsRegion, isProviderId } from "../auth/providers.ts";
import { DEFAULT_MAX_PARALLEL } from "../build/types.ts";
import type { CapabilitySet, HostIdentity } from "../roles/types.ts";
import { WebSettingsSchema } from "../extensions/web/settings.ts";
import { McpSettingsSchema } from "./mcp-settings.ts";
import { JobSettingsSchema } from "../extensions/jobs/settings.ts";
import { LlamaCppSettingsSchema, type LlamaCppSettings } from "../auth/local.ts";
import type { SummaryProfile } from "./summary.ts";

export type UiMode = "rich" | "plain";

export interface HostCapabilities extends CapabilitySet {
  readonly llama_cpp?: LlamaCppSettings;
}

export interface ModelChoice {
  readonly provider: ProviderId;
  readonly model: string;
}

export interface HostConfig {
  readonly identity: HostIdentity;
  readonly pane: string | null;
  /** Flag launches have a run directory and public local socket, not an inherited binding. */
  readonly local: { dir: string; id: string; socket: string; headless: boolean };
  readonly cwd: string;
  readonly root: string;
  readonly corral: string;
  readonly assets: string;
  readonly runtimeDir: string;
  readonly sessionDir: string;
  readonly freshSession?: boolean;
  /** The workspace's own terminal conversation, not a daemon-managed chat. */
  readonly terminalChat?: true;
  readonly todo?: boolean;
  readonly capabilities?: HostCapabilities;
  /** The snapshot's settings and project-text trust; absent from a corral that predates them. */
  readonly guidance?: CapabilityGuidance;
  readonly model: string;
  readonly provider?: ProviderId;
  readonly fallback?: readonly ModelChoice[];
  /** The non-secret Amazon Bedrock binding corral read at dispatch; absent means none. */
  readonly aws?: AwsBinding;
  readonly effort?: string;
  readonly promptFile?: string;
  readonly quickSummary?: boolean;
  readonly summary?: SummaryProfile;
  readonly ui?: UiMode;
}

type Json = Record<string, unknown>;

export const modelFlags = {
  model: { type: "string" }, provider: { type: "string" }, effort: { type: "string" },
  fallback: { type: "string", multiple: true },
  "aws-profile": { type: "string" }, "aws-region": { type: "string" },
} as const;

export class RemovedModelOption extends Error {}

export function rejectRemovedModelOptions(argv: readonly string[]): void {
  for (const [index, value] of argv.entries()) {
    const [flag, inline] = value.split("=", 2);
    if (flag === "--connection") {
      const provider = (inline ?? argv[index + 1]) === "openai-api" ? "openai" : "anthropic-api";
      throw new RemovedModelOption(`--connection was removed; use --model ${provider}/<model> or --provider ${provider}`);
    }
    if (flag === "--paid-route") throw new RemovedModelOption("--paid-route was removed; use --fallback <provider>/<model>");
  }
}

export function modelSettings(values: { model?: string; provider?: string; effort?: string; fallback?: readonly string[];
  "aws-profile"?: string; "aws-region"?: string }, settings?: Json) {
  if (values.model === undefined && values.provider === undefined) {
    const model = (settings?.startup as { model?: unknown } | undefined)?.model;
    if (model !== undefined) {
      if (typeof model !== "string" || !/^[^/]+\/[^\s\0]+$/.test(model) || !isProviderId(model.split("/", 1)[0])) {
        throw new Error(".corral-next/pi.toml: startup.model must be a known <provider>/<model>");
      }
      values = { ...values, model };
    }
  }
  const prefix = values.model?.split("/", 1)[0];
  const qualified = values.model?.includes("/") && isProviderId(prefix) ? prefix : undefined;
  const provider = values.provider ?? qualified ?? "openai-codex";
  if (!isProviderId(provider) || (qualified && qualified !== provider)) throw new Error("invalid --provider or conflicting --model provider");
  const model = values.model ?? (provider === "openai-codex" ? "gpt-6.1-sol"
    : provider === "anthropic" ? "claude-opus-4-8" : undefined);
  if (!model || /\s|\0/.test(model) || model === `${provider}/`) throw new Error(`pass --model <${provider}/id>`);
  const region = values["aws-region"];
  const profile = values["aws-profile"];
  let aws: AwsBinding | undefined;
  if (profile !== undefined || region !== undefined) {
    if (!profile || !/^[A-Za-z0-9_.+=,@-]{1,64}$/.test(profile) || !region || !isAwsRegion(region)) {
      throw new Error("--aws-profile and --aws-region require a valid AWS profile and region");
    }
    aws = { profile, region };
  }
  const selector = `${provider}/${model.startsWith(`${provider}/`) ? model.slice(provider.length + 1) : model}`;
  const fallback = values.fallback?.map(value => {
    const match = /^([^/]+)\/([^\s\0]+)$/.exec(value);
    if (!match || !isProviderId(match[1])) throw new Error("--fallback requires a known <provider>/<model>");
    if (value === selector) throw new Error(`--fallback ${value} matches the launch model`);
    return { provider: match[1], model: match[2]! };
  });
  return { provider, model, ...(values.effort ? { effort: values.effort } : {}),
    ...(aws ? { aws } : {}), ...(fallback?.length ? { fallback } : {}) };
}

const COMMON_ABILITIES = ["ask_user", "todo", "queue_control", "context_files", "skills", "compaction", "tool_search"];
const WORK_ABILITIES = ["codemode", "subagents", "oracle", "background_shell"];
const ABILITIES: Readonly<Record<"worker" | "reviewer" | "assistant" | "build", readonly string[]>> = {
  worker: [...COMMON_ABILITIES, ...WORK_ABILITIES, "fleet", "lsp", "session_export"],
  reviewer: [...COMMON_ABILITIES, "session_export"],
  assistant: [...COMMON_ABILITIES, ...WORK_ABILITIES, "fleet", "lsp"],
  build: [...COMMON_ABILITIES, ...WORK_ABILITIES, "lsp"],
};

function readPiSettings(root: string): Json {
  const settingsPath = join(root, ".corral-next", "pi.toml");
  try { return TOML.parse(readFileSync(settingsPath, "utf8")) as Json; }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  return {};
}

export function workspaceSettings(root: string, role: keyof typeof ABILITIES, run?: string, raw = readPiSettings(root)) {
  const enabled = [...ABILITIES[role]];
  if (role === "worker" && run) enabled.push("checkpoints");
  if (raw.bash_rules !== undefined) enabled.push("bash_rules");
  if (raw.hooks !== undefined && role === "worker") enabled.push("hooks");
  if (raw.browser !== undefined && role !== "reviewer") enabled.push("browser");
  if (role === "assistant" && raw.expert !== undefined) enabled.push("expert");
  const web = raw.web === undefined ? undefined : WebSettingsSchema.parse(raw.web);
  if (web) enabled.push("web.enabled");
  const mcp = raw.mcp === undefined ? undefined : McpSettingsSchema.parse(raw.mcp);
  if (mcp) enabled.push("mcp.enabled");
  const local = raw.llama_cpp === undefined ? undefined : LlamaCppSettingsSchema.parse(raw.llama_cpp);
  if (local) enabled.push("llama_cpp.enabled");
  const guidance: CapabilityGuidance = {
    projectText: true,
    settings: {
      ...Object.fromEntries(["lsp", "browser", "hooks", "bash_rules"].flatMap(key =>
        raw[key] && typeof raw[key] === "object" ? [[key, raw[key] as Json]] : [])),
      ...(enabled.includes("subagents") ? { subagents: { model: null, max_parallel: DEFAULT_MAX_PARALLEL, max_turns: 12, timeout_secs: 120 } } : {}),
      ...(enabled.includes("oracle") ? { oracle: { model: null, timeout_secs: 120 } } : {}),
      ...(enabled.includes("expert") ? { expert: raw.expert as Json } : {}),
      ...(enabled.includes("background_shell") ? { background_shell: JobSettingsSchema.parse(raw.background_shell ?? {
        max_jobs: 4, max_output_bytes: 8 * 1024 * 1024, max_runtime_secs: 3600, wait_ms: 10_000,
      }) } : {}),
    },
  };
  const capabilities: HostCapabilities = {
    catalog: 10, enabled, ...(web ? { web } : {}), ...(mcp ? { mcp } : {}), ...(local ? { llama_cpp: local } : {}),
  };
  return { capabilities, guidance, todo: enabled.includes("todo") };
}

export function sessionSocket(dir: string, env: NodeJS.ProcessEnv): string {
  const runtime = env.XDG_RUNTIME_DIR;
  const parent = runtime ? join(runtime, "corral-next") : join("/tmp", `corral-next-${process.getuid?.() ?? -1}`);
  return join(parent, `${createHash("sha256").update(dir).digest("hex").slice(0, 16)}.sock`);
}

export function findWorkspaceRoot(env: NodeJS.ProcessEnv, cwd: string): string | undefined {
  if (env.CORRAL_NEXT_ROOT) return resolve(cwd, env.CORRAL_NEXT_ROOT);
  for (let cur = cwd; ; cur = dirname(cur)) {
    if (existsSync(join(cur, ".corral-next", "corral.toml"))) return cur;
    if (cur === dirname(cur)) return undefined;
  }
}

export function parseRunFlags(command: "run" | "chat", argv: readonly string[], assets: string,
  env: NodeJS.ProcessEnv = process.env, cwd = process.cwd()): HostConfig {
  rejectRemovedModelOptions(argv);
  const { values } = parseArgs({ args: [...argv], options: {
    ...modelFlags, dir: { type: "string" }, socket: { type: "string" }, "prompt-file": { type: "string" },
    corral: { type: "string" }, project: { type: "string" }, headless: { type: "boolean" },
    ...(command === "chat" ? { new: { type: "boolean" } } as const : {}),
  } });
  if (!values.dir && (command === "run" || values.dir !== undefined)) {
    throw new Error(`corral-pi ${command} requires --dir <session-directory>`);
  }
  if (values.dir && values.new) throw new Error("--new is only available for terminal chat without --dir");
  const root = values.dir ? resolve(cwd, env.CORRAL_NEXT_ROOT ?? ".") : findWorkspaceRoot(env, cwd);
  if (!root) throw new Error("corral-pi chat is outside a Corral workspace");
  const dir = values.dir ? resolve(cwd, values.dir) : join(root, ".corral-next", "terminal-chat");
  const actor = env.CORRAL_ACTOR;
  if (command === "run" && actor !== undefined && actor !== "agent" && actor !== "reviewer") {
    throw new Error("CORRAL_ACTOR must be agent or reviewer for corral-pi run");
  }
  const role = command === "chat" ? "assistant" : actor === "reviewer" ? "reviewer" : "worker";
  const task = env.CORRAL_TASK || undefined;
  const run = env.CORRAL_RUN || undefined;
  const id = role === "assistant" ? basename(dir) : task ? `${task}/${role === "reviewer" ? "reviewer" : run ?? basename(dir)}` : basename(dir);
  const settings = readPiSettings(root);
  return {
    identity: { store: "local", project: values.project ?? "", role, task, run,
      ...(role === "assistant" ? { assistant: id } : {}) },
    cwd, root, corral: values.corral ? resolve(cwd, values.corral) : values.dir ? "" : "corral-next", assets, pane: null,
    local: { dir, id, socket: values.socket ? resolve(cwd, values.socket) : sessionSocket(dir, env),
      headless: values.headless === true || !process.stdin.isTTY },
    sessionDir: join(dir, "sessions"), runtimeDir: join(dir, "runtime"),
    ...(!values.dir ? { terminalChat: true } as const : {}),
    freshSession: values.new === true || env.CORRAL_FRESH === "1",
    ...(values["prompt-file"] ? { promptFile: resolve(cwd, values["prompt-file"]) } : {}),
    ...workspaceSettings(root, role, run, settings), ...modelSettings(values, !values.dir ? settings : undefined),
  };
}
