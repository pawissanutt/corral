// The corral-owned credential file behind Pi's CredentialStore (OD8). One JSON object keyed by
// provider id in <auth dir>/auth.json: the directory 0700, the file 0600, every write an atomic
// rename under a cross-process lock, so Pi's own refresh (Models.getAuth runs it inside
// modify) rewrites the credential in place. Only OAuth credentials are kept: an API key would
// be a paid fallback, which pi-native never uses.
import {
  closeSync, fsyncSync, lstatSync, mkdirSync, openSync, readFileSync, renameSync, rmSync, writeSync,
} from "node:fs";
import { randomBytes } from "node:crypto";
import { join } from "node:path";

import type { Credential, CredentialInfo, CredentialStore, OAuthCredential } from "@earendil-works/pi-ai";

import { AUTH_FILE, CredentialsUnavailable } from "./paths.ts";

const FILE_MAX_BYTES = 64 * 1024;
const LOCK_WAIT_MS = 30_000;
const LOCK_BACKOFF_MS = 25;

type Data = Record<string, OAuthCredential>;

function isOAuth(value: unknown): value is OAuthCredential {
  if (typeof value !== "object" || value === null) return false;
  const entry = value as Record<string, unknown>;
  return entry.type === "oauth" && typeof entry.access === "string" && typeof entry.refresh === "string" &&
    typeof entry.expires === "number" && Number.isFinite(entry.expires);
}

function missing(error: unknown): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === "ENOENT";
}

function alive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

export class FileCredentialStore implements CredentialStore {
  readonly file: string;
  private readonly lockFile: string;
  private chain: Promise<unknown> = Promise.resolve();

  constructor(readonly dir: string) {
    this.file = join(dir, AUTH_FILE);
    this.lockFile = `${this.file}.lock`;
  }

  present(provider = "openai-codex"): boolean {
    try {
      return isOAuth(this.load()[provider]);
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

  private load(): Data {
    let meta;
    try {
      meta = lstatSync(this.file);
    } catch (error) {
      if (missing(error)) return {};
      throw error;
    }
    if (!meta.isFile() || (meta.mode & 0o077) !== 0 || meta.size > FILE_MAX_BYTES) {
      throw new CredentialsUnavailable(`${this.file} must be a regular 0600 file of at most ${FILE_MAX_BYTES} bytes`);
    }
    const parsed: unknown = JSON.parse(readFileSync(this.file, "utf8"));
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new CredentialsUnavailable(`${this.file} is not a JSON object`);
    }
    const data: Data = {};
    for (const [provider, entry] of Object.entries(parsed)) if (isOAuth(entry)) data[provider] = entry;
    return data;
  }

  private save(data: Data): void {
    const staged = `${this.file}.${process.pid}.${randomBytes(4).toString("hex")}`;
    const fd = openSync(staged, "wx", 0o600);
    try {
      writeSync(fd, `${JSON.stringify(data, null, 2)}\n`);
      fsyncSync(fd);
    } finally {
      closeSync(fd);
    }
    renameSync(staged, this.file);
  }

  private async locked<T>(work: () => Promise<T>): Promise<T> {
    this.ensureDir();
    const deadline = Date.now() + LOCK_WAIT_MS;
    for (;;) {
      try {
        const fd = openSync(this.lockFile, "wx", 0o600);
        writeSync(fd, String(process.pid));
        closeSync(fd);
        break;
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
        const holder = Number(readFileSync(this.lockFile, "utf8").trim());
        if (!Number.isSafeInteger(holder) || holder <= 0 || !alive(holder)) {
          rmSync(this.lockFile, { force: true });
          continue;
        }
        if (Date.now() > deadline) throw new CredentialsUnavailable(`${this.lockFile} is held by pid ${holder}`);
        await new Promise((resolve) => setTimeout(resolve, LOCK_BACKOFF_MS));
      }
    }
    try {
      return await work();
    } finally {
      rmSync(this.lockFile, { force: true });
    }
  }

  private serialized<T>(work: () => Promise<T>): Promise<T> {
    const next = this.chain.catch(() => undefined).then(() => this.locked(work));
    this.chain = next.catch(() => undefined);
    return next;
  }

  async read(provider: string): Promise<Credential | undefined> {
    const entry = this.load()[provider];
    return entry ? structuredClone(entry) : undefined;
  }

  async list(): Promise<readonly CredentialInfo[]> {
    return Object.keys(this.load()).map((providerId) => ({ providerId, type: "oauth" as const }));
  }

  modify(
    provider: string, fn: (current: Credential | undefined) => Promise<Credential | undefined>,
  ): Promise<Credential | undefined> {
    return this.serialized(async () => {
      const data = this.load();
      const next = await fn(data[provider] ? structuredClone(data[provider]) : undefined);
      if (next === undefined) return data[provider];
      if (!isOAuth(next)) throw new CredentialsUnavailable("pi-native stores only OAuth credentials (no API key fallback)");
      data[provider] = next;
      this.save(data);
      return structuredClone(next);
    });
  }

  delete(provider: string): Promise<void> {
    return this.serialized(async () => {
      const data = this.load();
      delete data[provider];
      this.save(data);
    });
  }
}
