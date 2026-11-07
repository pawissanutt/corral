// F1: exercise both production foreground dispatches with background_shell disabled.
// FIFO handshakes hold a real descendant after its parent shell exits; no timing sleeps.
import { expect, test } from "bun:test";
import { execFileSync } from "node:child_process";
import { closeSync, constants, existsSync, mkdtempSync, openSync, readFileSync, rmSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { EventLog } from "../src/bridge/events.ts";
import { loadModules } from "../src/host/modules.ts";
import { HostSession, authContext } from "../src/host/session.ts";
import { guardedFetch, restrictEnvironment } from "../src/host/restrict.ts";
import { startTimeOf } from "../src/extensions/jobs/runtime.ts";
import { assistantShellBackend, requireVerifiedPlanningDrain } from "../src/roles/assistant/shell.ts";
import type { PlanningState } from "../src/host/planning-policy.ts";
import { CorralClient, sha256Hex } from "../src/tools/corral.ts";
import { fixtureTransport } from "./fixtures/auth/index.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";

// The managed runtime verifies ownership with Linux /proc; other platforms refuse this backend.
test.skipIf(process.platform !== "linux")("foreground user shell and fallback bash cannot leave a descendant past acknowledged planning drain", async () => {
  const observations: { path: string; phase: string; released: boolean; alive: boolean; activities: number; wrote: boolean }[] = [];
  const sdk = await import("@earendil-works/pi-coding-agent");
  for (const variant of ["user_bash", "bash"] as const) {
    const work = mkdtempSync(join(tmpdir(), "planning-shell-"));
    const assets = buildFixtureInstall(), raw = globalThis.fetch;
    const ready = join(work, "ready"), release = join(work, "release"), pidFile = join(work, "pid"), output = join(work, "late");
    execFileSync("mkfifo", [ready, release], { stdio: "ignore" });
    let host: HostSession | undefined, pid: number | undefined, start: string | undefined;
    try {
      restrictEnvironment(process.env);
      const loaded = await loadModules(assets.dir, "assistant");
      const modules = { ...loaded, role: { ...loaded.role, upstreamTools: ["read", "bash"] as const } };
      const transport = modules.auth.transport?.(authContext());
      if (!transport) throw new Error("missing fixture transport");
      const network = guardedFetch(transport); globalThis.fetch = network.fetch;
      const identity = { store: "st-demo", project: "", role: "assistant" as const, assistant: "A-1", generation: 1 };
      const fake = writeFakeCorral(work);
      const client = new CorralClient({ corral: fake.path, root: work, identity, hostCap: "h".repeat(64), sessionId: () => host?.sessionId() ?? "" });
      host = await HostSession.create({ sdk, identity, cwd: work, agentDir: join(work, "agent"), runtimeDir: join(work, "runtime"),
        model: "gpt-6.1-sol", modules, client, events: new EventLog(1), network, ui: "plain",
        capabilities: { catalog: 9, digest: "b".repeat(64), preset: "strict", enabled: [] },
        sessionManager: sdk.SessionManager.create(work, join(work, "sessions")) });
      await host.session.bindExtensions({}); await host.hello("n".repeat(64), "pane-1", sha256Hex("c".repeat(64)));
      expect(host.loadout).not.toContain("job_start"); // enforcement must not enable the optional capability
      const command = `(printf '%s' "$BASHPID" > '${pidFile}'; printf ready > '${ready}'; read -r line < '${release}'; printf late > '${output}') >/dev/null 2>&1 & read -r line < '${ready}'; printf parent-returned`;
      if (variant === "user_bash") {
        const action = await host.session.extensionRunner.emitUserBash({ type: "user_bash", command, cwd: work, excludeFromContext: true });
        if (!action?.operations) throw new Error("missing user-shell operations");
        await action.operations.exec(command, work, { onData() {} });
      } else {
        fixtureTransport().script([{ toolCalls: [{ callId: "foreground", name: "bash", arguments: { command } }] }, { text: "done" }]);
        await host.session.prompt("Run the fixture command", { source: "interactive" }); await host.drained();
        const result = host.session.messages.find(m => m.role === "toolResult" && m.toolName === "bash");
        console.log("FALLBACK_RESULT", JSON.stringify(result));
        expect(result?.role === "toolResult" && !result.isError).toBeTrue();
      }
      pid = Number(readFileSync(pidFile, "utf8")); start = startTimeOf(pid);
      const entered = await host.enterPlanning("A-1/g1/i1");
      let released = false;
      try {
        const fd = openSync(release, constants.O_WRONLY | constants.O_NONBLOCK);
        try { writeSync(fd, "go\n"); released = true; } finally { closeSync(fd); }
      } catch (error) { if (!(error instanceof Error) || !("code" in error) || error.code !== "ENXIO") throw error; }
      observations.push({ path: variant, phase: entered.phase, released, alive: start !== undefined, activities: entered.boundary?.activities.length ?? 0, wrote: existsSync(output) });
      console.log("SHELL_DRAIN", JSON.stringify(observations.at(-1)));
    } finally {
      // Baseline's leaking child is ours: only signal the recorded, still-matching PID.
      if (pid && start && startTimeOf(pid) === start) { try { process.kill(pid, "SIGKILL"); } catch {} }
      await host?.close(); globalThis.fetch = raw; assets.cleanup(); rmSync(work, { recursive: true, force: true });
    }
  }
  expect(observations).toEqual(["user_bash", "bash"].map(path => ({ path, phase: "planning", released: false, alive: false, activities: 0, wrote: false })));
}, 30_000);

test("unsupported drain preserves only ordinary non-workflow SDK shell and refuses Planning before transition", () => {
  expect(assistantShellBackend(false, { phase: "ordinary" })).toBe("sdk");
  expect(() => requireVerifiedPlanningDrain(false)).toThrow();
  expect(() => requireVerifiedPlanningDrain(true)).not.toThrow();
  expect(assistantShellBackend(true, { phase: "ordinary" })).toBe("managed");
  const protectedStates: PlanningState[] = [
    { phase: "planning" }, { phase: "recovering" },
    { phase: "closed", expected: { workflow: "W1", state_revision: 2, authority_epoch: 2, plan_revision: null } },
    { phase: "ordinary", boundary: { activities: [{ id: "old", tool: "user_bash", uncertain: true }] } },
  ];
  for (const state of protectedStates) expect(() => assistantShellBackend(false, state)).toThrow();
});
