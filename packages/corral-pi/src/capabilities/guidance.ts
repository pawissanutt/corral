// Text guidance one generation loads (T-564, plan T-460@P3 menus 1-3). Corral selects every
// path from the recorded snapshot: home Agent Skills and prompt templates from `user_dirs`, and
// the project's AGENTS.md/CLAUDE.md, `.agents/skills` and `.agents/prompts` only when corral
// recorded project-text trust. Upstream's own discovery stays off (restrict.ts), so `~/.pi`,
// project `.pi` settings, extensions, MCP and themes are never loaded as a side effect, and the
// role's packaged skill still arrives through its own `resources_discover` handler.
import { existsSync, statSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";

import { loadProjectContextFiles } from "@earendil-works/pi-coding-agent";

import type { CapabilitySet } from "../roles/types.ts";

/** The generation's settings and project-text trust, as `corral agent launch` sent them. */
export interface CapabilityGuidance {
  readonly settings: Readonly<Record<string, Readonly<Record<string, unknown>>>>;
  readonly projectText: boolean;
  /** The PM's selected project checkout; other roles read project text from their cwd. */
  readonly projectDir?: string;
}

export interface GuidanceOptions {
  readonly cwd: string;
  /** The corral workspace root: the outermost directory whose context files may load. */
  readonly root: string;
  readonly home: string;
  /** Upstream's agent directory, whose own context file is never project text. */
  readonly agentDir: string;
  readonly capabilities?: CapabilitySet;
  /** Absent when an older corral launched this host: no optional guidance loads. */
  readonly guidance?: CapabilityGuidance;
}

export interface ContextFile {
  readonly path: string;
  readonly content: string;
}

export interface Guidance {
  readonly skillPaths: readonly string[];
  readonly promptPaths: readonly string[];
  readonly contextFiles: readonly ContextFile[];
}

const NONE: Guidance = { skillPaths: [], promptPaths: [], contextFiles: [] };

function isDir(path: string): boolean {
  try {
    return statSync(path).isDirectory();
  } catch {
    return false;
  }
}

function within(path: string, root: string): boolean {
  const rel = relative(root, path);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !rel.startsWith(sep));
}

/** Configured `user_dirs` that exist, with `~/` expanded against `home`. */
function userDirs(settings: Readonly<Record<string, unknown>> | undefined, home: string): string[] {
  const dirs = Array.isArray(settings?.user_dirs) ? settings.user_dirs : [];
  return dirs
    .filter((dir): dir is string => typeof dir === "string")
    .map((dir) => (dir.startsWith("~/") ? join(home, dir.slice(2)) : dir))
    .filter(isDir);
}

/** `<dir>/<child>` for cwd and each ancestor through the repository root (the first `.git`). */
function repoDirs(cwd: string, child: string): string[] {
  const found: string[] = [];
  let dir = resolve(cwd);
  for (;;) {
    const candidate = join(dir, child);
    if (isDir(candidate)) found.push(candidate);
    const parent = dirname(dir);
    if (existsSync(join(dir, ".git")) || parent === dir) return found;
    dir = parent;
  }
}

/**
 * The guidance a role loads under its snapshot. Context files reuse upstream's selection
 * (ancestor order and a linked worktree shadowing its main checkout's copy), limited to the
 * corral workspace root; nothing is read from a project corral has not recorded as trusted.
 */
export function selectGuidance(options: GuidanceOptions): Guidance {
  const { capabilities, guidance } = options;
  if (!capabilities || !guidance) return NONE;
  const on = (key: string) => capabilities.enabled.includes(key);
  const trusted = guidance.projectText;
  const project = guidance.projectDir ?? options.cwd;
  const dedupe = (paths: string[]) => [...new Set(paths)];
  const skillPaths = on("skills")
    ? dedupe([...userDirs(guidance.settings.skills, options.home), ...(trusted ? repoDirs(project, ".agents/skills") : [])])
    : [];
  const promptPaths = on("prompt_templates")
    ? dedupe([
      ...userDirs(guidance.settings.prompt_templates, options.home),
      ...(trusted ? repoDirs(project, ".agents/prompts") : []),
    ])
    : [];
  const root = resolve(options.root);
  const agentDir = resolve(options.agentDir);
  const contextFiles = on("context_files") && trusted
    ? loadProjectContextFiles({ cwd: project, agentDir })
      .filter((file) => within(resolve(file.path), root) && !within(resolve(file.path), agentDir))
    : [];
  return { skillPaths, promptPaths, contextFiles };
}
