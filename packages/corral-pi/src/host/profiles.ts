// Profile selection is an owner request. Rust revalidates the named binding and spending
// policy under the session claim; a catalog or saved credential grants no paid authority.
import { randomBytes } from "node:crypto";
import { closeSync, constants, fstatSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { isConnectionId } from "../auth/connections.ts";
import type { HostIdentity } from "../roles/types.ts";
import type { AuthModule } from "../auth/types.ts";
import { ProfileReadiness } from "./profile-readiness.ts";
import type { ProfileChoice, ProfileController } from "./profile-controls.ts";

export const PROFILE_EXIT = 79;
export const PROFILE_CATALOG_FILE = "profiles.json";
export const PROFILE_REQUEST_FILE = "profile-request.json";
const MAX_CATALOG_BYTES = 256 * 1024;

interface CatalogProfile extends ProfileChoice {
  readonly effort?: string;
  readonly bindingDigest: string;
  readonly policyDigest: string;
}
interface Catalog { readonly version: 1; readonly current: string; readonly profiles: readonly CatalogProfile[] }
const object = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string" && value.length > 0
  && value.length <= 1024 && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
const digest = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);

/** No special files, symlinks, partial JSON or invented billing identities enter a picker. */
export function readProfileCatalog(runtimeDir: string): Catalog {
  const fd = openSync(join(runtimeDir, PROFILE_CATALOG_FILE), constants.O_RDONLY | constants.O_NOFOLLOW);
  let value: unknown;
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > MAX_CATALOG_BYTES) throw new Error("profile_catalog_invalid: expected a bounded regular file");
    value = JSON.parse(readFileSync(fd, "utf8"));
  } finally { closeSync(fd); }
  if (!object(value) || value.version !== 1 || !text(value.current) || !Array.isArray(value.profiles)
      || value.profiles.length > 1024) throw new Error("profile_catalog_invalid: malformed profile catalog");
  const names = new Set<string>();
  const profiles: CatalogProfile[] = value.profiles.map(row => {
    if (!object(row) || !text(row.name) || !text(row.label) || !text(row.model)
        || !isConnectionId(row.connection) || typeof row.available !== "boolean"
        || !digest(row.binding_digest) || !digest(row.policy_digest)
        || row.reason !== undefined && !text(row.reason) || names.has(row.name)
        || row.effort !== undefined && !text(row.effort)
        || row.new_session !== undefined && typeof row.new_session !== "boolean"
        || row.allowance_micros !== undefined && (!Number.isSafeInteger(row.allowance_micros)
          || Number(row.allowance_micros) <= 0)) {
      throw new Error("profile_catalog_invalid: malformed or duplicate profile");
    }
    names.add(row.name);
    return { name: row.name, label: row.label, model: row.model, connection: row.connection,
      bindingDigest: row.binding_digest, policyDigest: row.policy_digest,
      available: row.available, ...(row.reason === undefined ? {} : { reason: row.reason }),
      ...(row.effort === undefined ? {} : { effort: row.effort as string }),
      ...(row.new_session === undefined ? {} : { newSession: row.new_session as boolean }),
      ...(row.allowance_micros === undefined ? {} : { allowanceMicros: row.allowance_micros as number }) };
  });
  return { version: 1, current: value.current, profiles };
}

export interface BoundProfileOptions {
  readonly identity: HostIdentity;
  readonly runtimeDir: string;
  readonly auth: AuthModule;
  /** Covers model work, shell execution, pending queues and a previous handoff. */
  readonly blocked: () => string | undefined;
  readonly restart: () => void;
}

export class BoundProfileController implements ProfileController {
  private pending = false;
  private selected: string | undefined;
  private readonly readiness: ProfileReadiness;
  constructor(private readonly options: BoundProfileOptions) { this.readiness = new ProfileReadiness(options.auth); }
  current(): string | undefined { return this.selected; }
  blocked(): string | undefined {
    return this.pending ? "A profile switch is already in progress" : this.options.blocked();
  }
  async list(): Promise<readonly ProfileChoice[]> {
    const catalog = readProfileCatalog(this.options.runtimeDir);
    this.selected = catalog.current;
    return Promise.all(catalog.profiles.map(async row => {
      if (!row.available) return row;
      try { await this.readiness.check(row); return row; }
      catch (error) { return { ...row, available: false, reason: error instanceof Error ? error.message : String(error) }; }
    }));
  }
  async select(name: string): Promise<void> {
    const blocked = this.blocked();
    if (blocked) throw new Error(`profile_switch_refused: ${blocked}; current profile unchanged`);
    const catalog = readProfileCatalog(this.options.runtimeDir);
    this.selected = catalog.current;
    const chosen = catalog.profiles.find(row => row.name === name);
    if (!chosen || !chosen.available) {
      throw new Error(`profile_switch_refused: ${chosen?.reason ?? "profile is not available"}; current profile unchanged`);
    }
    if (catalog.current === name) return;
    await this.readiness.check(chosen);
    const again = this.blocked();
    if (again) throw new Error(`profile_switch_refused: ${again}; current profile unchanged`);
    const path = join(this.options.runtimeDir, PROFILE_REQUEST_FILE);
    const temporary = `${path}.${randomBytes(12).toString("hex")}.tmp`;
    const { identity } = this.options;
    this.pending = true;
    try {
      writeFileSync(temporary, JSON.stringify({ version: 1, profile: name, generation: identity.generation,
        binding_digest: chosen.bindingDigest, policy_digest: chosen.policyDigest,
        ...(chosen.newSession ? { new_session: true } : {}),
        ...(identity.assistant ? { session: identity.assistant } : {}) }), { mode: 0o600, flag: "wx" });
      renameSync(temporary, path);
      this.options.restart();
    } catch (error) {
      this.pending = false;
      rmSync(path, { force: true });
      throw error;
    } finally { rmSync(temporary, { force: true }); }
  }
}
