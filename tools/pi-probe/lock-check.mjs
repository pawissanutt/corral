// Checks a frozen install against the committed bun.lock: the lock bytes are
// unchanged, every locked package is installed at its locked version, each
// Pi runtime package resolves to one copy, and every declared peer resolves
// to a satisfying copy, provided by the root manifest where it declares one.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { bunApi, readBunLock, readPins } from "./runtime.mjs";

/** Splits a nested lock key: "a/@s/b/c" -> ["a", "@s/b", "c"]. @param {string} key */
export function segments(key) {
  const parts = key.split("/");
  /** @type {string[]} */
  const out = [];
  for (let index = 0; index < parts.length; index++) {
    out.push(parts[index].startsWith("@") ? parts[index] + "/" + parts[++index] : parts[index]);
  }
  return out;
}

/** @param {string | string[] | undefined} allowed @param {string} actual */
function platformMatches(allowed, actual) {
  if (allowed === undefined) return true;
  const list = Array.isArray(allowed) ? allowed : [allowed];
  return list.includes(actual) || (list.every((item) => item.startsWith("!")) && !list.includes("!" + actual));
}

/** @param {string} dir */
export function lockCheck(dir) {
  const pins = readPins(dirname(fileURLToPath(import.meta.url)));
  const lock = readBunLock(join(dir, "bun.lock"));
  const workspace = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  const rootDeps = { ...workspace.dependencies, ...workspace.devDependencies };
  /** @type {string[]} */
  const problems = [];
  if (lock.sha256 !== pins.bunLockSha256) problems.push("lock_bytes_changed");
  const entries = Object.entries(lock.packages).map(([key, value]) => {
    const entry = /** @type {[string, string, Record<string, any>, string]} */ (value);
    const at = entry[0].lastIndexOf("@");
    return { key, name: entry[0].slice(0, at), version: entry[0].slice(at + 1), meta: entry[2] ?? {} };
  });
  let installed = 0;
  /** @type {string[]} */
  const platformSkipped = [];
  for (const item of entries) {
    if (!platformMatches(item.meta.os, process.platform) || !platformMatches(item.meta.cpu, process.arch)) {
      platformSkipped.push(item.key);
      continue;
    }
    const path = join(dir, "node_modules", ...segments(item.key).flatMap((part, index) => index ? ["node_modules", part] : [part]));
    const manifest = existsSync(join(path, "package.json")) ? JSON.parse(readFileSync(join(path, "package.json"), "utf8")) : undefined;
    if (manifest?.name !== item.name || manifest?.version !== item.version) problems.push("installed_mismatch:" + item.key);
    else installed++;
  }
  /** @type {Record<string, string[]>} */
  const versions = {};
  for (const item of entries) (versions[item.name] ??= []).push(item.version);
  const runtimeCopies = Object.fromEntries(Object.entries(versions)
    .filter(([name]) => name.startsWith("@earendil-works/") || name === "typebox"));
  for (const [name, list] of Object.entries(runtimeCopies)) if (list.length !== 1) problems.push("runtime_duplicate:" + name);
  const peers = [];
  for (const item of entries) {
    for (const [peer, range] of Object.entries(item.meta.peerDependencies ?? {})) {
      const optional = (item.meta.optionalPeers ?? []).includes(peer);
      const scope = segments(item.key);
      let resolved;
      for (let depth = scope.length; depth >= 0 && !resolved; depth--) {
        const key = [...scope.slice(0, depth), peer].join("/");
        if (lock.packages[key]) resolved = entries.find((entry) => entry.key === key);
      }
      const satisfied = resolved ? bunApi().semver.satisfies(resolved.version, String(range)) : optional;
      if (!satisfied) problems.push("peer_unsatisfied:" + item.key + "->" + peer);
      peers.push({ package: item.key, peer, range, optional, resolved: resolved?.version ?? null,
        providedBy: resolved ? (peer in rootDeps ? "root" : "graph") : "absent", satisfied });
    }
  }
  return { type: "lock_check", ok: problems.length === 0, problems, lockSha256: lock.sha256,
    lockUnchanged: lock.sha256 === pins.bunLockSha256, locked: entries.length, installed, platformSkipped,
    runtimeCopies, peers, duplicates: Object.fromEntries(Object.entries(versions)
      .map(([name, list]) => [name, [...new Set(list)]]).filter(([, list]) => list.length > 1)) };
}

if (import.meta.main) {
  const dir = process.argv[2];
  if (!dir) throw new Error("usage: bun lock-check.mjs <install-root>");
  const report = lockCheck(dir);
  console.log(JSON.stringify(report));
  if (!report.ok) process.exitCode = 1;
}
