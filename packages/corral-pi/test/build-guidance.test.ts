import { expect, test } from "bun:test";
import { DEFAULT_MAX_PARALLEL, type BuildSettings } from "../src/build/types.ts";
import type { FleetRuntime } from "../src/extensions/fleet/runtime.ts";
import { fleetTools } from "../src/extensions/fleet/tools.ts";
import role from "../src/roles/build/index.ts";

const defaults: BuildSettings = { exitPolicy: "review", maxParallel: DEFAULT_MAX_PARALLEL, profiles: [] };
function contract(settings: BuildSettings, bare = false): string {
  const dir = "/build";
  return role.contract({
    identity: { store: "build", project: "sample", role: "build" }, cwd: dir, model: "parent-model",
    build: { dir, corral: "", settings, session: { version: 1, id: "B-test", name: "test", created: "", updated: "",
      repoRoot: dir, worktree: dir, branch: "main", base: "abc", here: true, bare } },
  });
}

test("a Build without delegation settings renders standard mode and the inherited default", () => {
  const prompt = contract(defaults);
  expect(prompt).toMatch(/^Mode: standard$/m);
  expect(prompt).toMatch(/^Ordered routes[^\n]*:\n- None$/m);
  expect(prompt).toMatch(/^- default: inherits /m);
  expect(prompt).toContain(`Maximum parallel children: ${DEFAULT_MAX_PARALLEL}`);
  expect(contract({ ...defaults, delegation: { mode: "standard", routes: [] } })).toBe(prompt);
});

test("proactive Build guidance renders even with no configured profiles or routes", () => {
  const prompt = contract({ ...defaults, delegation: { mode: "proactive", routes: [] } });
  expect(prompt).toMatch(/^Mode: proactive\n[^\n]*early[^\n]*read-only exploration/m);
  expect(prompt).toMatch(/^- default: inherits /m);
  expect(prompt).toMatch(/^Ordered routes[^\n]*:\n- None$/m);
});

test("Build renders routes in policy order with task kinds and profiles, including a configured default", () => {
  const prompt = contract({ ...defaults, profiles: [
    { name: "trail-scout", model: "openai-codex/synthetic-scout" },
    { name: "default", model: "openai-codex/synthetic-default", effort: "medium" },
  ], delegation: { mode: "standard", routes: [
    { match: "Trace what happens at the storage boundary", kind: "explore", profile: "trail-scout", strategy: "prefer_child" },
    { match: "Implement one storage adapter", kind: "slice", profile: "default", strategy: "when_delegating" },
  ] } }, true);
  expect(prompt.match(/^\d+\. .* → task kind=.*$/gm)).toEqual([
    '1. "Trace what happens at the storage boundary" → task kind=explore, profile="trail-scout", strategy=prefer_child',
    '2. "Implement one storage adapter" → task kind=slice, profile="default", strategy=when_delegating',
  ]);
  expect(prompt).toMatch(/^- default: openai-codex\/synthetic-default; effort=medium$/m);
  expect(prompt).not.toMatch(/^- default: inherits /m);
});

test("only the Build task tool points the model to Build delegation policy", () => {
  const runtime = {} as FleetRuntime; // Rendering the definitions does not invoke the runtime.
  const generic = fleetTools(runtime).find(tool => tool.name === "task")!.promptGuidelines!.join("\n");
  const build = fleetTools(runtime, undefined, undefined, "build").find(tool => tool.name === "task")!.promptGuidelines!.join("\n");
  expect(build).toContain("effective Build delegation policy");
  expect(build).not.toContain("approved provider/model");
  expect(generic).not.toContain("Build delegation policy");
});
