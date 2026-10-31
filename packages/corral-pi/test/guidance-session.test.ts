// guidance-and-session (T-564): every role loads the selected home skills, trust-gated project
// text and templates with directory opt-outs and no ambient `.pi` configuration; compaction
// settings reach upstream and summaries record usage; `/corral-export` attaches one offline HTML
// artifact per session snapshot.
import { afterAll, beforeAll, expect, test } from "bun:test";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { EventLog } from "../src/bridge/events.ts";
import type { CapabilityGuidance } from "../src/capabilities/guidance.ts";
import { selectGuidance } from "../src/capabilities/guidance.ts";
import { type LoadedModules, loadModules } from "../src/host/modules.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import type { CapabilitySet, HostIdentity, Role } from "../src/roles/types.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { type FixtureInstall, buildFixtureInstall, PACKAGE_ROOT } from "./support/fixture-install.ts";
import { type FakeCorral, writeFakeCorral } from "./support/fake-corral.ts";
import { nextEvent } from "./support/wait.ts";

const STANDARD = ["context_files", "skills", "prompt_templates", "compaction", "session_export"];
const HOME_DIRS = { skills: { user_dirs: ["~/.agents/skills"] }, prompt_templates: { user_dirs: ["~/.agents/prompts"] } };

let work: string;
let assets: FixtureInstall;
let home: string;
let root: string;
let cwd: string;
let team: string;
const originalFetch = globalThis.fetch;

function put(path: string, text: string): void {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
}

function skill(dir: string, name: string, body = `${name} instructions`): void {
  put(join(dir, name, "SKILL.md"), `---\nname: ${name}\ndescription: ${name} fixture. Use when testing guidance.\n---\n${body}\n`);
}

beforeAll(() => {
  assets = buildFixtureInstall();
  for (const tree of ["src/roles", "src/extensions", "assets"]) {
    cpSync(join(PACKAGE_ROOT, tree), join(assets.dir, tree), { recursive: true });
  }
  work = mkdtempSync(join(tmpdir(), "corral-pi-guidance-"));
  home = join(work, "home");
  skill(join(home, ".agents/skills"), "home-skill", "HOME-SKILL-TEXT");
  put(join(home, ".agents/prompts/home-review.md"), "---\ndescription: Home review\n---\nReview ${1:-everything} from home.\n");
  skill(join(home, ".pi/agent/skills"), "pi-home");
  put(join(home, ".pi/agent/prompts/pi-home-prompt.md"), "ambient home prompt\n");
  team = join(work, "team-skills");
  skill(team, "team-skill");
  put(join(work, "AGENTS.md"), "OUTSIDE-WORKSPACE\n");
  root = join(work, "ws");
  put(join(root, "AGENTS.md"), "WORKSPACE-GUIDE\n");
  cwd = join(root, "app");
  mkdirSync(join(cwd, ".git"), { recursive: true });
  put(join(cwd, "AGENTS.md"), "PROJECT-GUIDE\n");
  skill(join(cwd, ".agents/skills"), "repo-skill");
  put(join(cwd, ".agents/prompts/repo-prompt.md"), "---\ndescription: Repo prompt\n---\nRepo prompt about $1.\n");
  put(join(cwd, ".pi/settings.json"), JSON.stringify({ extensions: ["./extensions/evil.ts"], skills: ["./skills"] }));
  put(join(cwd, ".pi/extensions/evil.ts"), "throw new Error('ambient extension loaded');\n");
  skill(join(cwd, ".pi/skills"), "pi-project");
  put(join(cwd, ".pi/prompts/pi-project-prompt.md"), "ambient project prompt\n");
  restrictEnvironment(process.env);
});

afterAll(() => {
  globalThis.fetch = originalFetch;
  assets.cleanup();
  rmSync(work, { recursive: true, force: true });
});

function identity(role: Role): HostIdentity {
  const base = { store: "st-demo", project: "app", role, generation: 1 };
  if (role === "pm") return base;
  if (role === "reviewer") return { ...base, task: "T-564", run: "R1", review: "R1-review1", callback: "g1:c1" };
  return { ...base, task: "T-564", run: "R1" };
}

function snapshot(enabled: readonly string[]): CapabilitySet {
  return { catalog: 1, digest: sha256Hex(enabled.join(",")), preset: "standard", enabled: [...enabled] };
}

interface Started {
  readonly host: HostSession;
  readonly corral: FakeCorral;
  readonly events: EventLog;
}

interface Layout {
  readonly catalog?: number;
  readonly cwd?: string;
  readonly home?: string;
}

async function start(role: Role, enabled: readonly string[], guidance?: CapabilityGuidance, layout: Layout = {}): Promise<Started> {
  const dir = mkdtempSync(join(work, `${role}-`));
  const modules: LoadedModules = await loadModules(assets.dir, role);
  const transport = modules.auth.transport?.(authContext());
  if (!transport) throw new Error("fixture transport missing");
  const network = guardedFetch(transport);
  globalThis.fetch = network.fetch;
  const sdk = await import("@earendil-works/pi-coding-agent");
  const events = new EventLog(1);
  const corral = writeFakeCorral(dir);
  let host: HostSession | undefined;
  const client = new CorralClient({
    corral: corral.path, root, identity: identity(role), hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "",
  });
  host = await HostSession.create({
    sdk, identity: identity(role), cwd: layout.cwd ?? cwd, agentDir: join(dir, "agent"), model: "gpt-6.1-sol",
    capabilities: { ...snapshot(enabled), catalog: layout.catalog ?? 1 }, ...(guidance ? { guidance } : {}), root, home: layout.home ?? home,
    exportDir: join(dir, "sessions", "exports"),
    modules, client, events, network, sessionManager: sdk.SessionManager.create(layout.cwd ?? cwd, join(dir, "sessions")),
  });
  return { host, corral, events };
}

function loaded(host: HostSession) {
  const resources = host.runtime.services.resourceLoader;
  return {
    skills: resources.getSkills().skills.map((item) => item.name).sort(),
    prompts: resources.getPrompts().prompts.map((item) => item.name).sort(),
    context: resources.getAgentsFiles().agentsFiles.map((file) => file.path),
    extensions: resources.getExtensions().extensions.map((extension) => extension.path),
  };
}

async function bind(started: Started): Promise<void> {
  await started.host.session.bindExtensions({});
  await started.host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
}

async function turn(started: Started, text: string, key: string): Promise<void> {
  fixtureTransport().script([{ text: "Done" }]);
  const ended = nextEvent(started.events, (event) => event.kind === "turn_ended");
  const result = await started.host.submit({ text, busyPolicy: "prompt", idempotencyKey: key, origin: "test" }, `rq-${key}`);
  expect(result.disposition).toBe("started");
  await ended;
  await started.host.drained();
}

function userTexts(host: HostSession): string[] {
  return host.session.messages.flatMap((message) => {
    if (message.role !== "user") return [];
    if (typeof message.content === "string") return [message.content];
    return message.content.flatMap((part) => (part.type === "text" ? [part.text] : []));
  });
}

test("every role loads home skills, trusted project text and templates, never ambient .pi resources", async () => {
  // Given standard guidance with project-text trust for the worker and reviewer in the project
  // worktree, and the PM at the workspace root with its selected project's checkout:
  const trusted: CapabilityGuidance = { settings: HOME_DIRS, projectText: true };
  for (const role of ["worker", "reviewer", "pm"] as const) {
    const pm = role === "pm";
    const started = await start(role, role === "reviewer" ? STANDARD : [...STANDARD, "todo"],
      pm ? { ...trusted, projectDir: cwd } : trusted, pm ? { cwd: root } : {});
    try {
      // When the host binds its extensions and builds its resources:
      await started.host.session.bindExtensions({});
      const found = loaded(started.host);
      // Then home and repository Agent Skills, both template sets and the workspace and project
      // context files load in ancestor order; the worker keeps its packaged role skill; and no
      // ~/.pi, project .pi or out-of-workspace resource is loaded.
      const roleSkills = role === "worker" ? ["corral-worker"] : [];
      expect(found.skills).toEqual([...roleSkills, "home-skill", "repo-skill"].sort());
      expect(found.prompts).toEqual(["home-review", "repo-prompt"]);
      expect(found.context).toEqual([join(root, "AGENTS.md"), join(cwd, "AGENTS.md")]);
      expect(found.extensions.some((path) => path.includes(".pi"))).toBeFalse();
      const state = started.host.state().guidance as { project_text: boolean; export: boolean };
      expect([state.project_text, state.export]).toEqual([true, role !== "pm"]);
    } finally {
      started.host.session.dispose();
    }
  }

  // And without trust only home text loads; strict and an older corral load only the role skill.
  const untrusted = await start("worker", STANDARD, { settings: HOME_DIRS, projectText: false });
  const strict = await start("worker", ["todo", "compaction"], { settings: {}, projectText: true });
  const older = await start("worker", ["todo"]);
  try {
    for (const { host } of [untrusted, strict, older]) await host.session.bindExtensions({});
    const text = loaded(untrusted.host);
    expect([text.skills, text.prompts, text.context]).toEqual([["corral-worker", "home-skill"], ["home-review"], []]);
    for (const host of [strict.host, older.host]) {
      const found = loaded(host);
      expect([found.skills, found.prompts, found.context]).toEqual([["corral-worker"], [], []]);
      expect((host.state().guidance as { export: boolean }).export).toBeFalse();
    }
  } finally {
    for (const { host } of [untrusted, strict, older]) host.session.dispose();
  }
}, 120_000);

test("Pi pack lists and loads all five skills in a standard worker without OmO roots", async () => {
  const packHome = join(work, "pack-home");
  const pack = join(packHome, ".corral/skills/pi-native");
  const names = ["frontend", "visual-qa", "programming", "debugging", "git-master"];
  cpSync(join(PACKAGE_ROOT, "../../crates/corral-ops/assets/skills/pi-native"), pack, { recursive: true });
  const started = await start("worker", STANDARD, {
    settings: { skills: { user_dirs: ["~/.agents/skills", "~/.corral/skills/pi-native"] } },
    projectText: false,
  }, { home: packHome });
  try {
    await bind(started);
    const resources = started.host.runtime.services.resourceLoader.getSkills();
    expect(resources.diagnostics).toEqual([]);
    expect(loaded(started.host).skills).toEqual(["corral-worker", ...names].sort());
    const tools = new Set(started.host.session.getActiveToolNames());
    for (const name of names) {
      const found = resources.skills.find((item) => item.name === name);
      expect(found).toBeDefined();
      if (!found) throw new Error(`missing skill ${name}`);
      expect(found.filePath).toBe(join(pack, name, "SKILL.md"));
      // Load through the same explicit skill command a new session offers the owner.
      await turn(started, `/skill:${name}`, `pack-${name}`);
      expect(userTexts(started.host).some((text) => text.includes(`<skill name="${name}"`))).toBeTrue();
      const text = readFileSync(found.filePath, "utf8");
      // Pack convention: allowed-tools declares every tool, and bare inline identifiers or
      // call-shaped expressions denote tools. Shell commands have arguments inside backticks.
      // Validate against the actual standard worker registry, not a hand-maintained allowlist.
      const declared = text.match(/^allowed-tools: (.+)$/m)?.[1]?.split(/\s+/) ?? [];
      expect(declared.length).toBeGreaterThan(0);
      const mentioned = [...text.matchAll(/`([a-z_][\w.-]*)`|\b([a-z_][\w.]*)\(/g)]
        .map((match) => match[1] ?? match[2]).filter((item) => item !== undefined);
      for (const tool of [...declared, ...mentioned]) expect(tools.has(tool)).toBeTrue();
    }
  } finally {
    started.host.session.dispose();
  }
}, 120_000);

test("home and repository skills never replace the role's packaged skill, before or after reload", async () => {
  // Given a home and a trusted repository that each ship an impostor corral-worker skill:
  const impostorHome = join(work, "impostor-home");
  skill(join(impostorHome, ".agents/skills"), "corral-worker", "HOME-IMPOSTOR");
  const repo = join(work, "impostor-repo");
  mkdirSync(join(repo, ".git"), { recursive: true });
  skill(join(repo, ".agents/skills"), "corral-worker", "REPO-IMPOSTOR");
  const packaged = readFileSync(join(PACKAGE_ROOT, "src/roles/worker/skills/corral-worker/SKILL.md"), "utf8");
  const started = await start("worker", STANDARD, { settings: HOME_DIRS, projectText: true },
    { cwd: repo, home: impostorHome });
  try {
    for (const stage of ["startup", "reload"]) {
      // When the worker binds its extensions, and again after a reload:
      // An error listener is a binding, as the pane's UI is, so reload rediscovers resources.
      if (stage === "startup") await started.host.session.bindExtensions({ onError: () => undefined });
      else await started.host.session.reload();
      // Then the one corral-worker skill is the installation's file with its packaged content.
      const workers = started.host.runtime.services.resourceLoader.getSkills().skills
        .filter((item) => item.name === "corral-worker");
      expect(workers.map((item) => item.filePath)).toEqual([join(assets.dir, "src/roles/worker/skills/corral-worker/SKILL.md")]);
      expect(readFileSync(workers[0]?.filePath ?? "", "utf8")).toBe(packaged);
      const listed = (started.host.state().guidance as { skills: { name: string; path: string }[] }).skills
        .filter((item) => item.name === "corral-worker");
      expect(listed.map((item) => item.path)).toEqual(workers.map((item) => item.filePath));
    }
  } finally {
    started.host.session.dispose();
  }
}, 120_000);

test("the PM's selected project text follows its trust decision and directory settings", () => {
  // Given the PM at the workspace root with its project checkout, trusted or not:
  const select = (settings: CapabilityGuidance["settings"], projectText: boolean) =>
    selectGuidance({ cwd: root, root, home, agentDir: join(work, "agent"), capabilities: snapshot(STANDARD),
      guidance: { settings, projectText, projectDir: cwd } });
  const trusted = select(HOME_DIRS, true);
  const untrusted = select(HOME_DIRS, false);
  const optedOut = select({ skills: { user_dirs: [] }, prompt_templates: { user_dirs: [] } }, true);
  // Then the project's context, skills and templates load only under trust, and opt-outs drop
  // only the home directories.
  expect(trusted.contextFiles.map((file) => file.path)).toEqual([join(root, "AGENTS.md"), join(cwd, "AGENTS.md")]);
  expect(trusted.skillPaths).toEqual([join(home, ".agents/skills"), join(cwd, ".agents/skills")]);
  expect(trusted.promptPaths).toEqual([join(home, ".agents/prompts"), join(cwd, ".agents/prompts")]);
  expect([untrusted.contextFiles, untrusted.skillPaths, untrusted.promptPaths])
    .toEqual([[], [join(home, ".agents/skills")], [join(home, ".agents/prompts")]]);
  expect([optedOut.skillPaths, optedOut.promptPaths]).toEqual([[join(cwd, ".agents/skills")], [join(cwd, ".agents/prompts")]]);
});

test("a role's directory settings add or disable home directories and the skills switch disables both", () => {
  // Given one role that replaces the home skills directory, one with no home directories and
  // one whose skills are off:
  const select = (enabled: readonly string[], settings: CapabilityGuidance["settings"]) =>
    selectGuidance({ cwd, root, home, agentDir: join(work, "agent"), capabilities: snapshot(enabled), guidance: { settings, projectText: true } });
  const added = select(STANDARD, { ...HOME_DIRS, skills: { user_dirs: [team, "~/missing-dir"] } });
  const none = select(STANDARD, { skills: { user_dirs: [] }, prompt_templates: { user_dirs: [] } });
  const off = select(STANDARD.filter((key) => key !== "skills"), HOME_DIRS);
  // Then only the configured existing directories and the trusted repository's own load.
  expect(added.skillPaths).toEqual([team, join(cwd, ".agents/skills")]);
  expect(none.skillPaths).toEqual([join(cwd, ".agents/skills")]);
  expect(none.promptPaths).toEqual([join(cwd, ".agents/prompts")]);
  expect(off.skillPaths).toEqual([]);
  expect(off.promptPaths).toEqual([join(home, ".agents/prompts"), join(cwd, ".agents/prompts")]);
});

test("template and skill slash commands expand into the delivered prompt", async () => {
  // Given a bound worker with standard trusted guidance:
  const started = await start("worker", [...STANDARD, "todo"], { settings: HOME_DIRS, projectText: true });
  try {
    await bind(started);
    // When the turn input names a project template and a home skill:
    await turn(started, "/repo-prompt caching", "template");
    await turn(started, "/skill:home-skill now", "skill");
    // Then the agent received the expanded template text and the skill's instructions.
    const texts = userTexts(started.host);
    expect(texts.some((text) => text.includes("Repo prompt about caching."))).toBeTrue();
    expect(texts.some((text) => text.includes("HOME-SKILL-TEXT"))).toBeTrue();
  } finally {
    started.host.session.dispose();
  }
}, 120_000);

test("compaction and branch-summary settings reach upstream and a summary records its usage", async () => {
  // Given configured settings, compaction turned off, and an older corral without settings:
  const configured = await start("worker", STANDARD, {
    settings: { compaction: { reserve_tokens: 8192, keep_recent_tokens: 1, branch_summary: { reserve_tokens: 2048, skip_prompt: true } } },
    projectText: false,
  }, { catalog: 7 });
  const disabled = await start("worker", ["skills"], { settings: {}, projectText: false });
  const older = await start("worker", ["todo"]);
  try {
    // Then upstream resolves exactly those values, and omitted ones keep its defaults.
    const settings = configured.host.session.settingsManager;
    expect(settings.getCompactionSettings()).toEqual({ enabled: true, reserveTokens: 8192, keepRecentTokens: 1 });
    expect(settings.getBranchSummarySettings()).toEqual({ reserveTokens: 2048, skipPrompt: true });
    expect(disabled.host.session.settingsManager.getCompactionSettings().enabled).toBeFalse();
    expect(older.host.session.settingsManager.getCompactionSettings()).toEqual({
      enabled: true, reserveTokens: 16384, keepRecentTokens: 20000,
    });
    // When the configured worker compacts after a turn:
    await bind(configured);
    await turn(configured, "Remember the deployment checklist.", "before-compaction");
    await turn(configured, "And the rollback plan.", "second");
    fixtureTransport().script([{ text: "SUMMARY of the checklist" }]);
    await configured.host.session.compact();
    // Then the session file records the compaction with its own request usage, which corral's
    // collector adds to the run (crates/corral-ops/src/agent/usage_tests.rs).
    const file = configured.host.session.sessionFile;
    if (!file) throw new Error("no session file");
    const entries = readFileSync(file, "utf8").trim().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    const compaction = entries.find((entry) => entry.type === "compaction") as { summary?: string; usage?: { input: number; output: number } };
    expect(compaction?.summary).toContain("SUMMARY");
    expect(compaction?.usage?.input).toBeGreaterThan(0);
    expect(compaction?.usage?.output).toBeGreaterThan(0);
    const budget = configured.corral.calls().filter((c) => c.envelope.payload.kind === "budget");
    // Compaction may make separate summary/preparation requests: every actual request reserves.
    const reserved = budget.filter((c) => c.envelope.payload.action === "reserve");
    const settled = budget.filter((c) => c.envelope.payload.action === "settle");
    expect(reserved.length).toBeGreaterThanOrEqual(3);
    expect(settled.length).toBe(reserved.length);
  } finally {
    for (const { host } of [configured, disabled, older]) host.session.dispose();
  }
}, 120_000);

test("exporting the same snapshot twice attaches one private offline HTML artifact", async () => {
  // Given a bound worker with session export and one finished turn:
  const started = await start("worker", STANDARD, { settings: {}, projectText: false });
  try {
    await bind(started);
    await turn(started, "Export me please.", "export-1");
    // When the owner exports the same snapshot twice, then again after another turn:
    const first = await started.host.exportSession();
    const second = await started.host.exportSession();
    await turn(started, "One more turn.", "export-2");
    const third = await started.host.exportSession();
    // Then the snapshot has one file in the private export directory, both attach requests share
    // one request ID and payload (corral replays the first receipt), and a new snapshot is new.
    expect(second.path).toBe(first.path);
    expect(third.path).not.toBe(first.path);
    expect(statSync(dirname(first.path)).mode & 0o077).toBe(0);
    const html = readFileSync(first.path, "utf8");
    const embedded = (html.match(/[A-Za-z0-9+/]{200,}={0,2}/g) ?? []).map((run) => Buffer.from(run, "base64").toString("utf8"));
    expect(embedded.some((data) => data.includes("Export me please."))).toBeTrue();
    expect(html).not.toMatch(/<(script|link|img)[^>]+(src|href)=["']https?:/i);
    const artifacts = started.corral.calls().map(({ envelope }) => envelope).filter(({ op }) => op === "artifact");
    expect(artifacts).toHaveLength(3);
    expect(artifacts[1]?.request_id).toBe(artifacts[0]?.request_id ?? "");
    expect(artifacts[1]?.payload_sha256).toBe(artifacts[0]?.payload_sha256 ?? "");
    expect(artifacts[2]?.request_id).not.toBe(artifacts[0]?.request_id);
    expect([artifacts[0]?.payload.path, artifacts[0]?.payload.kind]).toEqual([first.path, "log"]);
    expect(existsSync(`${first.path}.${process.pid}.partial`)).toBeFalse();
  } finally {
    started.host.session.dispose();
  }
}, 120_000);
