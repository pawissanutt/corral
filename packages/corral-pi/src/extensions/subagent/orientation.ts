// A small, bounded map of the worktree for read-only research children, which do not load
// project guide files. Reading the layout is cheaper than spending the first turns on ls.
import { readdirSync, type Dirent } from "node:fs";
import { join } from "node:path";

const SKIP = new Set(["node_modules", "target", "dist", "build", "out", "coverage", "vendor", "__pycache__"]);
const GUIDES = ["AGENTS.md", "CLAUDE.md", "README.md"];
const MAX_TOP = 40;
const MAX_NESTED = 12;
const MAX_CHARS = 2400;

function entries(dir: string): Dirent[] {
  try {
    return readdirSync(dir, { withFileTypes: true })
      .filter(entry => !entry.name.startsWith(".") && !SKIP.has(entry.name))
      .sort((a, b) => a.name.localeCompare(b.name));
  } catch { return []; }
}

/** Two levels of directories plus top-level files; undefined when nothing is readable. */
export function projectOrientation(root: string): string | undefined {
  const top = entries(root);
  if (!top.length) return undefined;
  const lines: string[] = [];
  for (const entry of top.slice(0, MAX_TOP)) {
    if (!entry.isDirectory()) { lines.push(entry.name); continue; }
    const nested = entries(join(root, entry.name)).filter(child => child.isDirectory());
    const shown = nested.slice(0, MAX_NESTED).map(child => `${child.name}/`).join(", ");
    lines.push(`${entry.name}/${shown ? `: ${shown}${nested.length > MAX_NESTED ? `, +${nested.length - MAX_NESTED} more` : ""}` : ""}`);
  }
  if (top.length > MAX_TOP) lines.push(`+${top.length - MAX_TOP} more entries`);
  let layout = lines.join("\n");
  if (layout.length > MAX_CHARS) layout = `${layout.slice(0, MAX_CHARS)}\n[layout truncated]`;
  const guides = GUIDES.filter(name => top.some(entry => entry.isFile() && entry.name === name));
  return `Project layout (directories two levels deep; generated; hidden and build-output directories omitted):\n${layout}`
    + (guides.length ? `\n\nProject guides at the root: ${guides.join(", ")}. Read the relevant section when layout, conventions or ownership matter.` : "");
}
