// The launch message `corral agent launch` writes to the inherited pipe: the bound identity,
// paths and both generation secrets. It never reaches argv, the environment or a file.
import { closeSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";

import type { AwsBinding } from "../auth/aws.ts";
import type { CapabilityGuidance } from "../capabilities/guidance.ts";
import { type ProviderId, isAwsRegion, isProviderId } from "../auth/providers.ts";
import { PLANNING_ROLES, ROLES, type CapabilitySet, type HostIdentity, type Role } from "../roles/types.ts";
import { WebSettingsSchema } from "../extensions/web/settings.ts";
import { McpSettingsSchema } from "./mcp-settings.ts";
import { parseControls } from "./tool-controls.ts";
import { JobSettingsSchema } from "../extensions/jobs/settings.ts";

export const LAUNCH_VERSION = 1;
/** Catalog keys this host implements; compat.json's `capabilities` lists the same keys. */
export const HOST_CAPABILITIES: readonly string[] = [
  "todo", "web.enabled", "context_files", "skills", "prompt_templates", "compaction", "session_export",
  "mcp.enabled", "tool_search", "codemode",
  "hooks", "bash_rules", "background_shell", "role_enabled", "lsp",
];
const LAUNCH_MAX_BYTES = 64 * 1024;

export type UiMode = "rich" | "plain";

export interface HostSecrets {
  readonly hostCap: string;
  readonly ctlCap: string;
  readonly bindNonce: string;
}

export interface HostConfig {
  readonly identity: HostIdentity;
  readonly secrets: HostSecrets;
  readonly pane: string;
  readonly cwd: string;
  readonly root: string;
  readonly corral: string;
  readonly assets: string;
  readonly runtimeDir: string;
  readonly sessionDir: string;
  readonly resumeSession?: string;
  readonly freshSession?: boolean;
  readonly todo?: boolean;
  readonly capabilities?: CapabilitySet;
  /** The snapshot's settings and project-text trust; absent from a corral that predates them. */
  readonly guidance?: CapabilityGuidance;
  readonly model: string;
  readonly provider?: ProviderId;
  /** The non-secret Amazon Bedrock binding corral read at dispatch; absent means none. */
  readonly aws?: AwsBinding;
  readonly effort?: string;
  readonly promptFile?: string;
  readonly ui?: UiMode;
}

export class LaunchMessageInvalid extends Error {
  readonly code = "launch_message_invalid";
  constructor(reason: string) {
    super(`launch_message_invalid: ${reason}`);
  }
}

type Json = Record<string, unknown>;

function field(value: Json, key: string, pattern: RegExp, max = 4096): string {
  const item = value[key];
  if (typeof item !== "string" || item.length === 0 || item.length > max || !pattern.test(item)) {
    throw new LaunchMessageInvalid(`${key} is missing or malformed`);
  }
  return item;
}

function optional(value: Json, key: string, pattern: RegExp): string | undefined {
  return value[key] === undefined || value[key] === null ? undefined : field(value, key, pattern);
}

function path(value: Json, key: string): string {
  const item = field(value, key, /^[^\0]+$/);
  if (!isAbsolute(item)) throw new LaunchMessageInvalid(`${key} must be an absolute path`);
  return item;
}

const SECRET = /^[A-Za-z0-9_-]{32,256}$/;
const TOKEN = /^[A-Za-z0-9._:+-]+$/;
const AWS_PROFILE = /^[A-Za-z0-9_.+=,@-]{1,64}$/;

function awsBinding(value: unknown): AwsBinding | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "object" || Array.isArray(value)) throw new LaunchMessageInvalid("aws must be an object");
  const binding = value as Json;
  const region = field(binding, "region", /^[a-z0-9-]+$/, 32);
  if (!isAwsRegion(region)) throw new LaunchMessageInvalid("region is missing or malformed");
  return { profile: field(binding, "profile", AWS_PROFILE, 64), region };
}

function capabilityGuidance(value: unknown): CapabilityGuidance | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const set = value as Json;
  if (set.settings === undefined) return undefined;
  const settings = set.settings;
  if (typeof settings !== "object" || settings === null || Array.isArray(settings) ||
      Object.values(settings).some((entry) => typeof entry !== "object" || entry === null || Array.isArray(entry))) {
    throw new LaunchMessageInvalid("capabilities.settings must map names to objects");
  }
  if (typeof set.project_text !== "boolean") throw new LaunchMessageInvalid("capabilities.project_text must be a boolean");
  const projectDir = set.project_dir === undefined || set.project_dir === null ? undefined : path(set, "project_dir");
  return { settings: settings as CapabilityGuidance["settings"], projectText: set.project_text, ...(projectDir ? { projectDir } : {}) };
}

function capabilitySet(value: unknown): CapabilitySet | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "object" || Array.isArray(value)) throw new LaunchMessageInvalid("capabilities must be an object");
  const set = value as Json;
  if (typeof set.catalog !== "number" || !Number.isSafeInteger(set.catalog) || set.catalog < 1) {
    throw new LaunchMessageInvalid("capabilities.catalog must be a positive integer");
  }
  const enabled = set.enabled;
  if (!Array.isArray(enabled) || enabled.some((key) => typeof key !== "string")) {
    throw new LaunchMessageInvalid("capabilities.enabled must be a list of names");
  }
  // Corral resolves against compat.json, so an unknown enabled key is a mismatched install.
  const unknown = enabled.find((key) => !HOST_CAPABILITIES.includes(key as string));
  if (unknown !== undefined) throw new LaunchMessageInvalid(`capability ${unknown} is not implemented by this host`);
  const web = set.web === undefined ? undefined : WebSettingsSchema.safeParse(set.web);
  if (web && !web.success) throw new LaunchMessageInvalid("capabilities.web settings are malformed");
  if (enabled.includes("web.enabled") !== (web?.success === true)) {
    throw new LaunchMessageInvalid("web.enabled requires exactly one resolved capabilities.web settings object");
  }
  const mcp = set.mcp === undefined ? undefined : McpSettingsSchema.safeParse(set.mcp);
  if (mcp && !mcp.success) throw new LaunchMessageInvalid("capabilities.mcp settings are malformed");
  if (enabled.includes("mcp.enabled") !== (mcp?.success === true)) {
    throw new LaunchMessageInvalid("mcp.enabled requires exactly one resolved capabilities.mcp settings object");
  }
  return {
    catalog: set.catalog,
    digest: field(set, "digest", /^[0-9a-f]{64}$/, 64),
    preset: field(set, "preset", /^[a-z_]+$/, 32),
    enabled: [...new Set(enabled as string[])],
    ...(web?.success ? { web: web.data } : {}),
    ...(mcp?.success ? { mcp: mcp.data } : {}),
  };
}

export function parseLaunchMessage(text: string): HostConfig {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new LaunchMessageInvalid("not JSON");
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new LaunchMessageInvalid("not an object");
  const message = value as Json;
  if (message.version !== LAUNCH_VERSION) throw new LaunchMessageInvalid(`version must be ${LAUNCH_VERSION}`);
  const role = message.role;
  if (typeof role !== "string" || !ROLES.includes(role as Role)) throw new LaunchMessageInvalid("role is unknown");
  const generation = message.generation;
  if (typeof generation !== "number" || !Number.isSafeInteger(generation) || generation < 1) {
    throw new LaunchMessageInvalid("generation must be a positive integer");
  }
  const secrets = message.secrets;
  if (typeof secrets !== "object" || secrets === null) throw new LaunchMessageInvalid("secrets are missing");
  const secretFields = secrets as Json;
  const parsed: HostSecrets = {
    hostCap: field(secretFields, "host_cap", SECRET),
    ctlCap: field(secretFields, "ctl_cap", SECRET),
    bindNonce: field(secretFields, "bind_nonce", SECRET),
  };
  if (parsed.hostCap === parsed.ctlCap) throw new LaunchMessageInvalid("host_cap and ctl_cap must differ");
  const provider = message.provider === undefined || message.provider === null ? undefined : message.provider;
  if (provider !== undefined && !isProviderId(provider)) throw new LaunchMessageInvalid("provider is unknown");
  if (message.fresh_session !== undefined && typeof message.fresh_session !== "boolean") {
    throw new LaunchMessageInvalid("fresh_session must be a boolean");
  }
  if (message.todo !== undefined && typeof message.todo !== "boolean") {
    throw new LaunchMessageInvalid("todo must be a boolean");
  }
  const aws = awsBinding(message.aws);
  const capabilities = capabilitySet(message.capabilities);
  if (role === "pm" && capabilities?.enabled.includes("web.enabled")) {
    throw new LaunchMessageInvalid("web.enabled is not available to the pm role");
  }
  // Corral refuses to launch a disabled planning role; a host never runs one without its snapshot.
  if (PLANNING_ROLES.includes(role as Role) && capabilities?.enabled.includes("role_enabled") !== true) {
    throw new LaunchMessageInvalid(`the ${role} role needs an enabled role_enabled capability`);
  }
  const guidance = capabilityGuidance(message.capabilities);
  try {
    parseControls(role as Role, capabilities?.enabled ?? [], guidance?.settings);
  } catch (error) {
    throw new LaunchMessageInvalid(error instanceof Error ? error.message : String(error));
  }
  if (capabilities?.enabled.includes("background_shell")) {
    if (role !== "worker") throw new LaunchMessageInvalid(`background_shell is not available to the ${role} role`);
    if (!JobSettingsSchema.safeParse(guidance?.settings.background_shell).success) {
      throw new LaunchMessageInvalid("background_shell requires its resolved settings");
    }
  }
  const ui = message.ui === undefined || message.ui === null ? undefined : message.ui;
  if (ui !== undefined && ui !== "rich" && ui !== "plain") throw new LaunchMessageInvalid("ui must be rich or plain");
  return {
    identity: {
      store: field(message, "store", TOKEN, 64),
      project: field(message, "project", TOKEN, 128),
      role: role as Role,
      task: optional(message, "task", TOKEN),
      run: optional(message, "run", TOKEN),
      review: optional(message, "review", TOKEN),
      callback: optional(message, "callback", TOKEN),
      generation,
    },
    secrets: parsed,
    pane: field(message, "pane", TOKEN, 128),
    cwd: path(message, "cwd"),
    root: path(message, "root"),
    corral: path(message, "corral"),
    assets: path(message, "assets"),
    runtimeDir: path(message, "runtime_dir"),
    sessionDir: path(message, "session_dir"),
    resumeSession: message.resume_session === undefined || message.resume_session === null ? undefined : path(message, "resume_session"),
    freshSession: message.fresh_session === true,
    todo: capabilities ? capabilities.enabled.includes("todo") : message.todo === true,
    ...(capabilities ? { capabilities } : {}),
    ...(guidance ? { guidance } : {}),
    model: field(message, "model", TOKEN, 128),
    ...(provider ? { provider } : {}),
    ...(aws ? { aws } : {}),
    effort: optional(message, "effort", TOKEN),
    promptFile: message.prompt_file === undefined || message.prompt_file === null ? undefined : path(message, "prompt_file"),
    ...(ui ? { ui } : {}),
  };
}

/** Reads the launch message from the inherited descriptor and closes it. */
export function readLaunchMessage(fd: number): HostConfig {
  let bytes: Buffer;
  try {
    bytes = readFileSync(fd);
  } finally {
    closeSync(fd);
  }
  if (bytes.length > LAUNCH_MAX_BYTES) throw new LaunchMessageInvalid("exceeds its bound");
  return parseLaunchMessage(bytes.toString("utf8"));
}
