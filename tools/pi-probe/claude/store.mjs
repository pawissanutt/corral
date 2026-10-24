import { chmodSync, fstatSync, lstatSync, mkdirSync, openSync, closeSync, readFileSync, readSync, renameSync, writeFileSync } from "node:fs";
import { constants } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join, resolve, sep } from "node:path";
import { ProbeFailure, record } from "../credentials.mjs";
import { FORBIDDEN_STORE_SEGMENTS } from "./policy.mjs";

/** Probe credentials live apart from the production corral-owned dir (OD8) a later child provisions.
 * @param {NodeJS.ProcessEnv} env */
export function defaultStoreDir(env = process.env) {
  const data = env.XDG_DATA_HOME || join(env.HOME || homedir(), ".local/share");
  return join(data, "corral", "pi-native-probe", "claude");
}

/** Refuse any directory that belongs to another harness's credential store.
 * @param {string} dir */
export function checkStoreDir(dir) {
  if (!isAbsolute(dir)) throw new ProbeFailure("credential_dir_not_absolute");
  const segments = resolve(dir).split(sep);
  if (segments.some((segment) => FORBIDDEN_STORE_SEGMENTS.includes(segment))) {
    throw new ProbeFailure("credential_dir_foreign_store");
  }
  return resolve(dir);
}

/**
 * A 0600 `auth.json` in an owner-only 0700 directory, written atomically and serialized in process.
 * It implements Pi's CredentialStore so Pi itself refreshes and persists the OAuth tokens.
 * @param {string} directory
 * @returns {import("@earendil-works/pi-ai").CredentialStore & {readonly path: string}}
 */
export function openCredentialStore(directory) {
  const dir = checkStoreDir(directory);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const dirStat = lstatSync(dir);
  if (!dirStat.isDirectory() || dirStat.uid !== process.getuid?.()) throw new ProbeFailure("credential_dir_not_owner_directory");
  chmodSync(dir, 0o700);
  const path = join(dir, "auth.json");
  const present = () => {
    try { lstatSync(path); return true; } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ENOENT") return false;
      throw error;
    }
  };
  /** @returns {Record<string, unknown>} */
  const load = () => {
    if (!present()) return {};
    let fd;
    try {
      fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    } catch (error) {
      if (error instanceof Error && "code" in error && error.code === "ELOOP") throw new ProbeFailure("credential_file_not_owner_private_bounded");
      throw error;
    }
    try {
      const stat = fstatSync(fd);
      if (!stat.isFile() || stat.size > 65_536 || stat.uid !== process.getuid?.() || (stat.mode & 0o077) !== 0) {
        throw new ProbeFailure("credential_file_not_owner_private_bounded");
      }
      const buffer = Buffer.alloc(stat.size);
      readSync(fd, buffer, 0, buffer.length, 0);
      const text = buffer.toString("utf8");
      /** @type {unknown} */
      let data;
      try { data = JSON.parse(text); } catch { throw new ProbeFailure("unsupported_credential_schema"); }
      if (!record(data)) throw new ProbeFailure("unsupported_credential_schema");
      return data;
    } finally { closeSync(fd); }
  };
  /** @param {Record<string, unknown>} data */
  const save = (data) => {
    writeFileSync(path + ".tmp", JSON.stringify(data, null, 2) + "\n", { mode: 0o600 });
    renameSync(path + ".tmp", path);
  };
  /** @type {Promise<unknown>} */
  let chain = Promise.resolve();
  /** @template T @param {() => Promise<T>} work @returns {Promise<T>} */
  const serialized = (work) => {
    const next = chain.then(work, work);
    chain = next.catch(() => undefined);
    return next;
  };
  return {
    path,
    async read(providerId) {
      return /** @type {import("@earendil-works/pi-ai").Credential | undefined} */ (load()[providerId]);
    },
    async list() {
      return Object.entries(load()).map(([providerId, value]) => ({ providerId,
        type: /** @type {"oauth" | "api_key"} */ (record(value) && value.type === "api_key" ? "api_key" : "oauth") }));
    },
    modify(providerId, fn) {
      return serialized(async () => {
        const data = load();
        const next = await fn(/** @type {import("@earendil-works/pi-ai").Credential | undefined} */ (data[providerId]));
        if (next === undefined) return /** @type {import("@earendil-works/pi-ai").Credential | undefined} */ (data[providerId]);
        data[providerId] = next;
        save(data);
        return next;
      });
    },
    delete(providerId) {
      return serialized(async () => {
        const data = load();
        delete data[providerId];
        save(data);
      });
    },
  };
}
