// Builds a bounded temp asset directory from the package: src/, package.json, bun.lock and
// compat.json, the package's installed node_modules linked in, and (with overlays) fixture.json
// plus the fixture role, auth and delivery modules copied to their fixed module paths. Fixture
// modules exist only inside that temp directory, never in the package's own src/.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { roleDir } from "../../src/host/modules.ts";
import { ROLES } from "../../src/roles/types.ts";

export const PACKAGE_ROOT = fileURLToPath(new URL("../..", import.meta.url)).replace(/\/$/, "");
const FILES_MAX = 1024;
const BYTES_MAX = 16 * 1024 * 1024;

export interface FixtureOptions {
  /** Copy the fixture modules and fixture.json (default true). */
  readonly overlays?: boolean;
  /** Fixture modules to leave out, for single-module refusal tests. */
  readonly omit?: readonly ("role" | "delivery" | "auth")[];
  readonly parent?: string;
  /** Give the fixture host a private upstream Pi state directory, including crashes.json. */
  readonly isolatePiState?: boolean;
  /** Keep a fixed state directory only for tests that explicitly inspect a crashed host. */
  readonly retainPiState?: boolean;
}

export interface FixtureInstall {
  readonly dir: string;
  cleanup(): void;
}

/** Remove state left by killed hosts at an independent test boundary. */
export function clearFixturePiStates(install: FixtureInstall): void {
  for (const name of readdirSync(install.dir)) {
    if (name.startsWith("pi-state-")) rmSync(join(install.dir, name), { recursive: true, force: true });
  }
}

function bounded(dir: string): void {
  let files = 0;
  let bytes = 0;
  const walk = (path: string) => {
    for (const entry of readdirSync(path, { withFileTypes: true })) {
      if (entry.name === "node_modules") continue;
      const child = join(path, entry.name);
      if (entry.isDirectory()) walk(child);
      else {
        files++;
        bytes += statSync(child).size;
      }
      if (files > FILES_MAX || bytes > BYTES_MAX) {
        throw new Error(`fixture install exceeds ${FILES_MAX} files or ${BYTES_MAX} bytes`);
      }
    }
  };
  walk(dir);
}

function copy(from: string, to: string): void {
  mkdirSync(dirname(to), { recursive: true });
  cpSync(from, to, { recursive: true });
}

export function buildFixtureInstall(options: FixtureOptions = {}): FixtureInstall {
  const dir = mkdtempSync(join(options.parent ?? tmpdir(), "corral-pi-assets-"));
  for (const file of ["package.json", "bun.lock", "compat.json"]) copy(join(PACKAGE_ROOT, file), join(dir, file));
  copy(join(PACKAGE_ROOT, "src"), join(dir, "src"));
  if (options.isolatePiState) {
    const main = join(dir, "src/host/main.ts");
    const source = readFileSync(main, "utf8");
    const restrict = "  restrictEnvironment(process.env, provider);";
    if (source.split(restrict).length !== 2) throw new Error("fixture host environment hook not found");
    let setup: string;
    if (options.retainPiState) {
      const state = join(dir, "pi-state");
      mkdirSync(state);
      setup = `process.env.PI_CODING_AGENT_DIR = ${JSON.stringify(state)};`;
    } else {
      // Each launched host (including a resumed generation) gets its own state. The containing
      // fixture removes any residue after SIGKILL, which cannot run the exit handler.
      setup = `const fs = await import("node:fs");\n  const state = fs.mkdtempSync(${JSON.stringify(join(dir, "pi-state-"))});\n  process.env.PI_CODING_AGENT_DIR = state;\n  process.once("exit", () => fs.rmSync(state, { recursive: true, force: true }));`;
    }
    writeFileSync(main, source.replace(restrict, `${restrict}\n  ${setup}`));
  }
  symlinkSync(join(PACKAGE_ROOT, "node_modules"), join(dir, "node_modules"), "dir");
  if (options.overlays ?? true) {
    const omit = new Set(options.omit ?? []);
    const fixtures = join(PACKAGE_ROOT, "test/fixtures");
    const modules: string[] = [];
    if (!omit.has("role")) {
      copy(join(fixtures, "roles/fixture-role.ts"), join(dir, "src/roles/fixture-role.ts"));
      for (const role of ROLES.map(roleDir)) {
        copy(join(fixtures, `roles/${role}/index.ts`), join(dir, `src/roles/${role}/index.ts`));
        modules.push(`src/roles/${role}/index.ts`);
      }
    }
    if (!omit.has("auth")) {
      copy(join(fixtures, "auth/index.ts"), join(dir, "src/auth/index.ts"));
      copy(join(fixtures, "auth/fake-transport.ts"), join(dir, "src/auth/fake-transport.ts"));
      modules.push("src/auth/index.ts");
    } else {
      rmSync(join(dir, "src/auth/index.ts"), { force: true });
    }
    if (!omit.has("delivery")) {
      copy(join(fixtures, "delivery/index.ts"), join(dir, "src/delivery/index.ts"));
      modules.push("src/delivery/index.ts");
    }
    writeFileSync(join(dir, "fixture.json"), `${JSON.stringify({
      version: 1, fixture: true, built_by: "packages/corral-pi/test/support/fixture-install.ts", modules,
    }, null, 2)}\n`);
  }
  bounded(dir);
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [parent, flag] = process.argv.slice(2);
  const install = buildFixtureInstall({ ...(parent ? { parent } : {}), overlays: flag !== "--bare" });
  process.stdout.write(`${install.dir}\n`);
}
