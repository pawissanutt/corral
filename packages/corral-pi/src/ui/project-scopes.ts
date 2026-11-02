import { isAbsolute, resolve } from "node:path";
import { z } from "zod";
import { plainLabel } from "./panels-view.ts";

export interface ProjectScope { readonly label: string; readonly path: string }
const Snapshot = z.object({
  workspace: z.string().refine(isAbsolute),
  projects: z.record(z.string(), z.object({ path: z.string() })),
  session: z.object({
    scratch: z.array(z.object({ project: z.string(), slug: z.string(), path: z.string().refine(isAbsolute) })).default([]),
  }).nullish(),
});

/** Display scopes only: choosing one never changes the launcher identity, tools or plan. */
export function projectScopes(value: unknown): readonly ProjectScope[] {
  const data = Snapshot.parse(value);
  const projects = Object.entries(data.projects).slice(0, 200).map(([name, project]) => ({
    label: name, path: resolve(data.workspace, project.path),
  }));
  return [...projects, ...(data.session?.scratch ?? []).slice(0, 200).map(scratch => ({
    label: `${scratch.project} / ${scratch.slug} (scratch)`, path: scratch.path,
  }))];
}

export async function chooseProject(scopes: readonly ProjectScope[],
  select: (title: string, choices: string[]) => Promise<string | undefined>): Promise<ProjectScope | undefined> {
  // Indexed labels remain unambiguous even after sanitizing terminal controls in project names.
  const labels = scopes.map((scope, index) => `${index + 1}. ${plainLabel(scope.label)}`);
  const selected = await select("Files scope · does not rebind the session", labels);
  return scopes[labels.indexOf(selected ?? "")];
}
