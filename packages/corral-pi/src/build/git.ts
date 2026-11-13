import { checked, type Run } from "./corral-cli.ts";
import type { BuildSessionRecord, BuildSummary, FleetSnapshot } from "./types.ts";

export async function summarize(run: Run, session: BuildSessionRecord, fleet: FleetSnapshot): Promise<BuildSummary> {
  const cwd = session.worktree;
  const head = (await checked(run, ["rev-parse", "HEAD"], cwd)).trim();
  const ref = (await checked(run, ["rev-parse", "--abbrev-ref", "HEAD"], cwd)).trim();
  const base = session.base;
  const commits = base ? Number((await checked(run, ["rev-list", "--count", `${base}..HEAD`], cwd)).trim()) : 0;
  const diff = base ? await checked(run, ["diff", "--numstat", base, "--"], cwd) : "";
  let files = 0, added = 0, removed = 0;
  for (const line of diff.split("\n")) {
    if (!line) continue;
    const [plus, minus] = line.split("\t");
    files++;
    if (plus !== "-") added += Number(plus) || 0;
    if (minus !== "-") removed += Number(minus) || 0;
  }
  const dirty = (await checked(run, ["status", "--porcelain"], cwd)).length > 0;
  const outside = new Map<string, Set<string>>();
  for (const child of fleet.children) for (const edit of child.edits) {
    if (!edit.outside) continue;
    const by = outside.get(edit.path) ?? new Set<string>();
    by.add(child.id);
    outside.set(edit.path, by);
  }
  return {
    branch: ref === "HEAD" ? null : ref, base, head, commits, files, added, removed, dirty,
    slices: fleet.children.map(child => ({ id: child.id, state: child.state,
      seconds: child.startedAt === undefined ? 0 : Math.max(0, ((child.endedAt ?? Date.now()) - child.startedAt) / 1000),
      ...(child.usage.cost === undefined ? {} : { cost: child.usage.cost }),
    })),
    outside: [...outside].map(([path, by]) => ({ path, by: [...by] })),
  };
}
