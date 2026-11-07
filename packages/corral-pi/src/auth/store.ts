// The corral-owned credential files behind Pi's CredentialStore (OD8): one file per provider,
// <auth dir>/<provider>.json, the directory 0700, each file 0600, every write an atomic rename
// under the directory's one cross-process lock (auth.json.lock), so Pi's own refresh
// (Models.getAuth runs it inside modify) rewrites the credential in place. A Codex credential
// still in the pre-T-479 shared auth.json is read from there and moved into openai-codex.json
// by its first write, so an existing login keeps working. Explicit API connections use separate
// <connection>.json files and accept only literal API keys; key presence never admits spending.
import { closeSync, constants, fstatSync, fsyncSync, lstatSync, mkdirSync, openSync, readSync, renameSync, unlinkSync, writeSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { join } from "node:path";

import type { Credential, CredentialInfo, CredentialStore, OAuthCredential } from "@earendil-works/pi-ai";

import { type LockHooks, withFileLock } from "./lock.ts";
import { AUTH_FILE, CredentialsUnavailable } from "./paths.ts";
import type { ProviderId } from "./providers.ts";
import { type ConnectionId, isApiConnection, resolveConnection, validApiKey } from "./connections.ts";

const FILE_MAX_BYTES = 64 * 1024;
/** Bounded retained-handle read, also used for protected API-key input files. */
export function readPrivateFile(file: string, maximum = FILE_MAX_BYTES): string {
  let fd: number | undefined;
  try {
    fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
    const meta = fstatSync(fd);
    if (!meta.isFile() || meta.uid !== process.getuid?.() || (meta.mode & 0o777) !== 0o600 || meta.size > maximum) {
      throw new Error("unsafe credential input");
    }
    const bytes = Buffer.alloc(maximum + 1);
    let size = 0;
    for (;;) {
      const count = readSync(fd, bytes, size, bytes.length - size, null);
      size += count;
      if (size > maximum) throw new Error("oversized credential input");
      if (count === 0) return bytes.subarray(0, size).toString("utf8");
    }
  } catch {
    throw new CredentialsUnavailable("credential input must be an owner-only regular 0600 file within its size bound");
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

/** The only provider the pre-T-479 shared auth.json ever held. */
const LEGACY_PROVIDER: ProviderId = "openai-codex";

function isOAuth(value: unknown): value is OAuthCredential {
  if (typeof value !== "object" || value === null) return false;
  const entry = value as Record<string, unknown>;
  return entry.type === "oauth" && typeof entry.access === "string" && typeof entry.refresh === "string" &&
    typeof entry.expires === "number" && Number.isFinite(entry.expires);
}

function missing(error: unknown): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === "ENOENT";
}

export function credentialFile(dir: string, provider: ProviderId | ConnectionId): string {
  return join(dir, `${provider}.json`);
}

export class FileCredentialStore implements CredentialStore {
  readonly file: string;
  private readonly legacyFile: string;
  private readonly lockFile: string;
  private chain: Promise<unknown> = Promise.resolve();

  readonly connection: ConnectionId;

  constructor(readonly dir: string, readonly provider: ProviderId = "openai-codex", private readonly hooks: LockHooks = {}, connection?: ConnectionId) {
    this.connection = resolveConnection(provider, connection);
    this.file = credentialFile(dir, isApiConnection(this.connection) ? this.connection : provider);
    this.legacyFile = join(dir, AUTH_FILE);
    this.lockFile = `${this.legacyFile}.lock`;
  }

  present(): boolean {
    try {
      return this.load() !== undefined;
    } catch {
      return false;
    }
  }

  ensureDir(): void {
    mkdirSync(this.dir, { recursive: true, mode: 0o700 });
    this.checkDir();
  }

  private checkDir(): void {
    const meta = lstatSync(this.dir);
    const uid = process.getuid?.();
    if (!meta.isDirectory() || (uid !== undefined && meta.uid !== uid)) {
      throw new CredentialsUnavailable(`${this.dir} must be a directory owned by this user`);
    }
    if ((meta.mode & 0o077) !== 0) {
      throw new CredentialsUnavailable(`${this.dir} must be mode 0700`);
    }
  }

  private readJson(file: string): unknown {
    let meta;
    try {
      meta = lstatSync(file);
    } catch (error) {
      if (missing(error)) return undefined;
      throw error;
    }
    if (!meta.isFile()) throw new CredentialsUnavailable("credential must be a private regular file");
    let parsed: unknown;
    try { parsed = JSON.parse(readPrivateFile(file, FILE_MAX_BYTES)); }
    catch { throw new CredentialsUnavailable("credential file is unsafe or malformed"); }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new CredentialsUnavailable(`${file} is not a JSON object`);
    }
    return parsed;
  }

  private legacy(): Record<string, unknown> | undefined {
    if (this.provider !== LEGACY_PROVIDER || isApiConnection(this.connection)) return undefined;
    return this.readJson(this.legacyFile) as Record<string, unknown> | undefined;
  }

  private valid(value: unknown): value is Credential {
    if (!isApiConnection(this.connection)) return isOAuth(value);
    const key = value as { type?: unknown; key?: unknown } | null;
    return key?.type === "api_key" && typeof key.key === "string" && validApiKey(key.key);
  }

  private load(): Credential | undefined {
    // Validate the directory on reads too; never follow an auth-directory symlink.
    try { this.checkDir(); } catch (error) { if (missing(error)) return undefined; throw error; }
    const own = this.readJson(this.file);
    if (own !== undefined) return this.valid(own) ? own : undefined;
    const entry = this.legacy()?.[this.provider];
    return isOAuth(entry) ? entry : undefined;
  }

  private writeAtomically(file: string, value: unknown): void {
    const staged = `${file}.${process.pid}.${randomBytes(4).toString("hex")}`;
    const fd = openSync(staged, "wx", 0o600);
    try {
      writeSync(fd, `${JSON.stringify(value, null, 2)}\n`);
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    renameSync(staged, file);
  }

  /** Drops this provider from the shared pre-T-479 file once its own file holds the truth. */
  private retireLegacy(): void {
    const legacy = this.legacy();
    if (legacy === undefined || !(this.provider in legacy)) return;
    const { [this.provider]: _moved, ...rest } = legacy;
    if (Object.keys(rest).length === 0) unlinkSync(this.legacyFile);
    else this.writeAtomically(this.legacyFile, rest);
  }

  private locked<T>(work: () => Promise<T>): Promise<T> {
    this.ensureDir();
    return withFileLock(this.lockFile, work, this.hooks);
  }

  private serialized<T>(work: () => Promise<T>): Promise<T> {
    const next = this.chain.catch(() => undefined).then(() => this.locked(work));
    this.chain = next.catch(() => undefined);
    return next;
  }

  async read(provider: string): Promise<Credential | undefined> {
    if (provider !== this.provider) return undefined;
    const entry = this.load();
    return entry ? structuredClone(entry) : undefined;
  }

  async list(): Promise<readonly CredentialInfo[]> {
    return this.load() ? [{ providerId: this.provider, type: isApiConnection(this.connection) ? "api_key" as const : "oauth" as const }] : [];
  }

  modify(
    provider: string, fn: (current: Credential | undefined) => Promise<Credential | undefined>,
  ): Promise<Credential | undefined> {
    if (provider !== this.provider) {
      return Promise.reject(new CredentialsUnavailable(`this store holds only the ${this.provider} credential`));
    }
    return this.serialized(async () => {
      const current = this.load();
      const next = await fn(current ? structuredClone(current) : undefined);
      if (next === undefined) return current;
      if (!this.valid(next)) throw new CredentialsUnavailable(isApiConnection(this.connection)
        ? "API connection stores only literal platform API keys" : "corral-pi stores only OAuth credentials (no API key fallback)");
      this.writeAtomically(this.file, next);
      this.retireLegacy();
      return structuredClone(next);
    });
  }

  delete(provider: string): Promise<void> {
    if (provider !== this.provider) return Promise.resolve();
    return this.serialized(async () => {
      try {
        unlinkSync(this.file);
      } catch (error) {
        if (!missing(error)) throw error;
      }
      this.retireLegacy();
    });
  }
}
