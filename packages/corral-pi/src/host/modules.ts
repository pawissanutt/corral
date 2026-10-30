// Fixed-path module loader (F6). Later T-252 children add these files; they never edit the
// host. A missing module is ModuleUnavailable naming the child that supplies it.
import { existsSync, lstatSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import type { ProviderId } from "../auth/providers.ts";
import type { AuthModule } from "../auth/types.ts";
import type { DeliveryModule } from "../delivery/types.ts";
import { CORRAL_OPS, ROLES, type Role, type RoleModule, UPSTREAM_TOOLS } from "../roles/types.ts";
import { compiledBundle } from "./bundle.ts";

export type ModuleKind = "role" | "delivery" | "auth";

const ROLE_OWNERS: Record<Role, string> = {
  worker: "worker-role",
  reviewer: "reviewer-role",
  pm: "pm-role",
  planner: "plan-roles",
  plan_reviewer: "plan-roles",
  assistant: "assistant-role",
};

/** The role module's directory under src/roles; Rust's `AgentRole::module_dir` matches it. */
export function roleDir(role: Role): string {
  return role === "plan_reviewer" ? "plan-reviewer" : role;
}

export class ModuleUnavailable extends Error {
  readonly code: string;
  constructor(readonly kind: ModuleKind, readonly module: string, readonly owner_child: string, role?: Role) {
    const code = kind === "role" ? "role_unavailable" : kind === "delivery" ? "delivery_unavailable"
      : "credentials_unavailable";
    super(`${code}${role ? ` ${role}` : ""}: no ${module} (owner ${owner_child})`);
    this.code = code;
  }
}

export class ModuleInvalid extends Error {
  readonly code = "module_invalid";
  constructor(readonly module: string, reason: string) {
    super(`module_invalid: ${module}: ${reason}`);
  }
}

export interface ModulePaths {
  readonly role: string;
  readonly delivery: string;
  readonly auth: string;
}

export function modulePaths(role: Role): ModulePaths {
  if (!ROLES.includes(role)) throw new ModuleInvalid("src/roles", `unknown role ${String(role)}`);
  return {
    role: `src/roles/${roleDir(role)}/index.ts`,
    delivery: "src/delivery/index.ts",
    auth: "src/auth/index.ts",
  };
}

function present(assetRoot: string, module: string): boolean {
  const path = join(assetRoot, module);
  return existsSync(path) && lstatSync(path).isFile();
}

/** Every missing module of a role, in the order the launcher reports them. */
export function missingModules(assetRoot: string, role: Role): ModuleUnavailable[] {
  const paths = modulePaths(role);
  if (compiledBundle()?.roles) return [];
  const missing: ModuleUnavailable[] = [];
  if (!present(assetRoot, paths.role)) missing.push(new ModuleUnavailable("role", paths.role, ROLE_OWNERS[role], role));
  if (!present(assetRoot, paths.delivery)) missing.push(new ModuleUnavailable("delivery", paths.delivery, "inbox-adapter"));
  if (!present(assetRoot, paths.auth)) missing.push(new ModuleUnavailable("auth", paths.auth, "host-install"));
  return missing;
}

async function importDefault(assetRoot: string, module: string): Promise<unknown> {
  const loaded: { default?: unknown } = await import(pathToFileURL(join(assetRoot, module)).href);
  if (loaded.default === undefined) throw new ModuleInvalid(module, "has no default export");
  return loaded.default;
}

function object(value: unknown, module: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null) throw new ModuleInvalid(module, "default export is not an object");
  return value as Record<string, unknown>;
}

function checkRole(value: unknown, module: string, role: Role): RoleModule {
  const candidate = object(value, module);
  if (candidate.role !== role) throw new ModuleInvalid(module, `declares role ${String(candidate.role)}`);
  const upstream = candidate.upstreamTools;
  const ops = candidate.corralOps;
  if (!Array.isArray(upstream) || !upstream.every((t) => UPSTREAM_TOOLS.includes(t))) {
    throw new ModuleInvalid(module, "upstreamTools must list only read, edit, write, bash, grep, find or ls");
  }
  if (!Array.isArray(ops) || !ops.every((op) => CORRAL_OPS.includes(op))) {
    throw new ModuleInvalid(module, "corralOps must list only v1 operations");
  }
  if (typeof candidate.tools !== "function" || typeof candidate.contract !== "function") {
    throw new ModuleInvalid(module, "needs tools() and contract()");
  }
  checkLocalTools(candidate, module);
  return candidate as unknown as RoleModule;
}

const LOCAL_TOOL_NAME = /^[a-z][a-z0-9_]{0,63}$/;

function checkLocalTools(candidate: Record<string, unknown>, module: string): void {
  const names = candidate.localToolNames;
  if (names === undefined && candidate.localTools === undefined) return;
  if (!Array.isArray(names) || typeof candidate.localTools !== "function") {
    throw new ModuleInvalid(module, "localTools() needs localToolNames, and localToolNames needs localTools()");
  }
  const seen = new Set<string>();
  for (const name of names) {
    if (typeof name !== "string" || !LOCAL_TOOL_NAME.test(name)) {
      throw new ModuleInvalid(module, `local tool name ${String(name)} must match ${LOCAL_TOOL_NAME.source}`);
    }
    if ((UPSTREAM_TOOLS as readonly string[]).includes(name) || name.startsWith("corral_") || seen.has(name)) {
      throw new ModuleInvalid(module, `local tool ${name} collides with an upstream, corral or another local tool`);
    }
    seen.add(name);
  }
}

function checkDelivery(value: unknown, module: string): DeliveryModule {
  const candidate = object(value, module);
  if (typeof candidate.accept !== "function") throw new ModuleInvalid(module, "needs accept()");
  return candidate as unknown as DeliveryModule;
}

function checkAuth(value: unknown, module: string, provider: ProviderId): AuthModule {
  const candidate = object(value, module);
  if (candidate.provider !== "openai-codex" || typeof candidate.credentials !== "function") {
    throw new ModuleInvalid(module, "needs provider openai-codex and credentials()");
  }
  const serves = candidate.providers === undefined ? [candidate.provider] : candidate.providers;
  if (!Array.isArray(serves) || !serves.includes(provider)) {
    throw new ModuleUnavailable("auth", `${module} for provider ${provider}`, "host-install");
  }
  return candidate as unknown as AuthModule;
}

export interface LoadedModules {
  readonly role: RoleModule;
  readonly delivery: DeliveryModule;
  readonly auth: AuthModule;
}

/** Only the auth module, for `corral pi --bare`, which loads no role or delivery module. */
export async function loadAuth(assetRoot: string, provider: ProviderId = "openai-codex"): Promise<AuthModule> {
  const path = "src/auth/index.ts";
  const bundle = compiledBundle();
  if (bundle) return checkAuth(bundle.auth, path, provider);
  if (!present(assetRoot, path)) throw new ModuleUnavailable("auth", path, "host-install");
  return checkAuth(await importDefault(assetRoot, path), path, provider);
}

/** Loads the role, delivery and auth modules from their fixed paths under the asset root. */
export async function loadModules(
  assetRoot: string, role: Role, provider: ProviderId = "openai-codex",
): Promise<LoadedModules> {
  const [first] = missingModules(assetRoot, role);
  if (first) throw first;
  const paths = modulePaths(role);
  const bundle = compiledBundle();
  if (bundle?.roles && bundle.delivery !== undefined) {
    return {
      role: checkRole(bundle.roles[role](assetRoot), paths.role, role),
      delivery: checkDelivery(bundle.delivery, paths.delivery),
      auth: checkAuth(bundle.auth, paths.auth, provider),
    };
  }
  return {
    role: checkRole(await importDefault(assetRoot, paths.role), paths.role, role),
    delivery: checkDelivery(await importDefault(assetRoot, paths.delivery), paths.delivery),
    auth: checkAuth(await importDefault(assetRoot, paths.auth), paths.auth, provider),
  };
}
