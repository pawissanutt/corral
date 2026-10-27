// The corral-owned credential files behind Pi's CredentialStore (OD8): one file per provider,
// <auth dir>/<provider>.json, the directory 0700, each file 0600, every write an atomic rename
// under the directory's one cross-process lock (auth.json.lock), so Pi's own refresh
// (Models.getAuth runs it inside modify) rewrites the credential in place. A Codex credential
// still in the pre-T-479 shared auth.json is read from there and moved into openai-codex.json
// by its first write, so an existing login keeps working. Only OAuth credentials are kept: an
// API key would be a paid fallback, which pi-native never uses.
import { closeSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, unlinkSync, writeSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { join } from "node:path";

import type { Credential, CredentialInfo, CredentialStore, OAuthCredential } from "@earendil-works/pi-ai";

import { type LockHooks, withFileLock } from "./lock.ts";
import { AUTH_FILE, CredentialsUnavailable } from "./paths.ts";
import type { ProviderId } from "./providers.ts";

const FILE_MAX_BYTES = 64 * 1024;
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

export function credentialFile(dir: string, provider: ProviderId): string {
  return join(dir, `${provider}.json`);
}

export class FileCredentialStore implements CredentialStore {
  readonly file: string;
  private readonly legacyFile: string;
  private readonly lockFile: string;
  private chain: Promise<unknown> = Promise.resolve();

  constructor(readonly dir: string, readonly provider: ProviderId = "openai-codex", private readonly hooks: LockHooks = {}) {
    this.file = credentialFile(dir, provider);
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
    if (!meta.isFile() || (meta.mode & 0o077) !== 0 || meta.size > FILE_MAX_BYTES) {
      throw new CredentialsUnavailable(`${file} must be a regular 0600 file of at most ${FILE_MAX_BYTES} bytes`);
    }
    const parsed: unknown = JSON.parse(readFileSync(file, "utf8"));
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new CredentialsUnavailable(`${file} is not a JSON object`);
    }
    return parsed;
  }

  private legacy(): Record<string, unknown> | undefined {
    if (this.provider !== LEGACY_PROVIDER) return undefined;
    return this.readJson(this.legacyFile) as Record<string, unknown> | undefined;
  }

  private load(): OAuthCredential | undefined {
    const own = this.readJson(this.file);
    if (own !== undefined) return isOAuth(own) ? own : undefined;
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
    return this.load() ? [{ providerId: this.provider, type: "oauth" as const }] : [];
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
      if (!isOAuth(next)) throw new CredentialsUnavailable("pi-native stores only OAuth credentials (no API key fallback)");
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
