// The launch message `corral agent launch` writes to the inherited pipe: the bound identity,
// paths and both generation secrets. It never reaches argv, the environment or a file.
import { closeSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";

import { type ProviderId, isProviderId } from "../auth/providers.ts";
import { ROLES, type HostIdentity, type Role } from "../roles/types.ts";

export const LAUNCH_VERSION = 1;
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
  readonly model: string;
  readonly provider?: ProviderId;
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
    model: field(message, "model", TOKEN, 128),
    ...(provider ? { provider } : {}),
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
