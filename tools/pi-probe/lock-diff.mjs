// Lists every difference between the committed bun.lock and T-136's retained
// npm lock (its raw SHA-256 is pinned in pins.json) for review: resolutions,
// integrities, root declarations, install placement, and what each dependent
// actually resolves for each declared dependency under Node's lookup walk.
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readBunLock, readPins } from "./runtime.mjs";
import { segments } from "./lock-check.mjs";

/** @param {string} t136LockPath */
export function lockDiff(t136LockPath) {
  const probeRoot = dirname(fileURLToPath(import.meta.url));
  const pins = readPins(probeRoot);
  const npmBytes = readFileSync(t136LockPath);
  const npmSha256 = createHash("sha256").update(npmBytes).digest("hex");
  if (npmSha256 !== pins.t136LockSha256) throw new Error("not T-136's retained lock: " + npmSha256);
  const npm = JSON.parse(npmBytes.toString("utf8"));
  const bun = readBunLock(join(probeRoot, "bun.lock"));
  /** @typedef {{name: string, version: string, integrity: string, deps: string[]}} Placed */
  /** @param {Record<string, any>} meta */
  const declared = (meta) => Object.keys({ ...meta.dependencies, ...meta.optionalDependencies, ...meta.peerDependencies });
  /** @type {Map<string, Placed>} */
  const npmPlaced = new Map();
  for (const [path, value] of Object.entries(npm.packages)) {
    if (!path) continue;
    const entry = /** @type {Record<string, any>} */ (value);
    const name = path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length);
    npmPlaced.set(path.replace(/^node_modules\//, "").split("/node_modules/").join("/"),
      { name, version: entry.version, integrity: entry.integrity, deps: declared(entry) });
  }
  /** @type {Map<string, Placed>} */
  const bunPlaced = new Map();
  for (const [key, value] of Object.entries(bun.packages)) {
    const entry = /** @type {[string, string, Record<string, any>, string]} */ (value);
    const at = entry[0].lastIndexOf("@");
    bunPlaced.set(key, { name: entry[0].slice(0, at), version: entry[0].slice(at + 1), integrity: entry[3],
      deps: declared(entry[2] ?? {}) });
  }
  /** Node's lookup: the dependent's own node_modules, then each ancestor's, then the root.
   * @param {Map<string, Placed>} placed */
  const resolved = (placed) => {
    /** @type {Map<string, Set<string>>} */
    const out = new Map();
    for (const [key, item] of placed) {
      const scope = segments(key);
      for (const dep of item.deps) {
        let target = "absent";
        for (let depth = scope.length; depth >= 0; depth--) {
          const hit = placed.get([...scope.slice(0, depth), dep].join("/"));
          if (hit) { target = hit.name + "@" + hit.version; break; }
        }
        const id = item.name + "@" + item.version + " -> " + dep;
        (out.get(id) ?? out.set(id, new Set()).get(id))?.add(target);
      }
    }
    return out;
  };
  /** @param {Map<string, {name: string, version: string, integrity: string}>} placed */
  const resolutions = (placed) => new Map([...placed.values()].map((item) => [item.name + "@" + item.version, item.integrity]));
  const npmResolved = resolutions(npmPlaced);
  const bunResolved = resolutions(bunPlaced);
  const onlyBun = [...bunResolved.keys()].filter((key) => !npmResolved.has(key)).sort();
  const onlyT136 = [...npmResolved.keys()].filter((key) => !bunResolved.has(key)).sort();
  const integrityMismatches = [...bunResolved].filter(([key, integrity]) => npmResolved.has(key) &&
    npmResolved.get(key) !== integrity).map(([key]) => key);
  /** @param {string} key @param {Map<string, {name: string, version: string}>} placed */
  const at = (key, placed) => { const item = placed.get(key); return item ? item.name + "@" + item.version : null; };
  const placementDifferences = [...new Set([...npmPlaced.keys(), ...bunPlaced.keys()])].sort()
    .filter((key) => at(key, npmPlaced) !== at(key, bunPlaced))
    .map((key) => ({ placement: key, t136: at(key, npmPlaced), bun: at(key, bunPlaced) }));
  const npmEdges = resolved(npmPlaced);
  const bunEdges = resolved(bunPlaced);
  const dependencyResolutionDifferences = [...new Set([...npmEdges.keys(), ...bunEdges.keys()])].sort()
    .map((edge) => ({ edge, t136: [...(npmEdges.get(edge) ?? [])].sort(), bun: [...(bunEdges.get(edge) ?? [])].sort() }))
    .filter((item) => JSON.stringify(item.t136) !== JSON.stringify(item.bun));
  const root = /** @type {Record<string, Record<string, string>>} */ (npm.packages[""]);
  const workspace = bun.workspaces[""];
  return {
    t136: { lock: "T-136:R1:d96d0a73-package-lock.json", sha256: npmSha256, lockfileVersion: npm.lockfileVersion,
      placements: npmPlaced.size, resolutions: npmResolved.size, root: { dependencies: root.dependencies,
        devDependencies: root.devDependencies } },
    bun: { lock: "tools/pi-probe/bun.lock", sha256: bun.sha256, placements: bunPlaced.size, resolutions: bunResolved.size,
      root: { dependencies: workspace.dependencies, devDependencies: workspace.devDependencies } },
    migratedFrom: "bun install migrated the T-136 package-lock.json, keeping its resolutions",
    onlyBun, onlyT136, integrityMismatches, placementDifferences,
    dependencyEdges: { t136: npmEdges.size, bun: bunEdges.size }, dependencyResolutionDifferences,
  };
}

if (import.meta.main) {
  const path = process.argv[2];
  if (!path) throw new Error("usage: bun lock-diff.mjs <T-136 package-lock.json>");
  console.log(JSON.stringify(lockDiff(path), null, 2));
}
