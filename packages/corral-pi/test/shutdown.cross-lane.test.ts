// Real interactive host in the private herdr server. The fixture injects failures only
// at stdin's raw-mode restoration; bridge, InteractiveMode and shutdown remain real.
import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { connect } from "./support/bridge-client.ts";
import { writeFakeCorral } from "./support/fake-corral.ts";
import { buildFixtureInstall } from "./support/fixture-install.ts";
import { within } from "./support/wait.ts";
import { fileAppears, ok, privateHerdr, privateRoot, run } from "./support/workspace.ts";

const isolated = privateHerdr();
const cap = "c".repeat(48);
const frame = (command: string) => ({ version: 1, id: command, ctl_cap: cap, generation: 1, command, args: {} });

for (const mode of ["normal", "throw", "emit"]) test(`interactive worker shutdown: ${mode}`, async () => {
  if (!isolated) { console.log("skipped: requires private herdr"); return; }
  const dir = mkdtempSync(join(privateRoot() ?? "", "pi-shutdown-"));
  const assets = buildFixtureInstall({ parent: dir });
  let workspace = "";
  try {
    const main = join(assets.dir, "src/host/main.ts");
    let source = readFileSync(main, "utf8");
    // Count calls to the actual InteractiveMode.stop, then invoke the guarded stop twice.
    source = source.replace("  guardTerminalStop(terminal, process.stdin);", `
      let stopCalls = 0, restores = 0, emitted = 0;
      const actualStop = terminal.stop.bind(terminal);
      terminal.stop = () => { stopCalls++; actualStop(); };
      process.on('exit', () => require('node:fs').writeFileSync(${JSON.stringify(join(dir, "counts.json"))}, JSON.stringify({stopCalls, restores, emitted})));
      guardTerminalStop(terminal, process.stdin);`);
    source = source.replace("        terminal?.stop();", "        terminal?.stop(); terminal?.stop();");
    source = source.replace("  session.markAttached(\"interactive\");", `
      const raw = process.stdin.setRawMode.bind(process.stdin);
      process.stdin.setRawMode = (value) => {
        if (!value) {
          restores++;
          const error = Object.assign(new Error('injected raw mode EIO'), { code: 'EIO' });
          if (${JSON.stringify(mode)} === 'throw') throw error;
          if (${JSON.stringify(mode)} === 'emit') queueMicrotask(() => { emitted++; process.stdin.emit('error', error); });
        }
        return raw(value);
      };
      require('node:fs').writeFileSync(${JSON.stringify(join(dir, "attached"))}, 'ready');
      session.markAttached("interactive");`);
    writeFileSync(main, source);
    const created = JSON.parse(ok(["herdr", "--session", "corral-test", "workspace", "create", "--cwd", dir, "--label", `shutdown-${mode}`, "--no-focus"])).result;
    workspace = created.workspace.workspace_id;
    const pane: string = created.root_pane.pane_id;
    const runtime = join(dir, "runtime"); mkdirSync(runtime, { mode: 0o700 });
    const corral = writeFakeCorral(dir);
    const message = { version: 1, store: "st-0123456789abcdef", project: "app", role: "worker", task: "T-001", run: "R1", generation: 1,
      secrets: { host_cap: "h".repeat(48), ctl_cap: cap, bind_nonce: "n".repeat(48) }, pane, cwd: dir, root: dir,
      corral: corral.path, assets: assets.dir, runtime_dir: runtime, session_dir: join(dir, "sessions"), model: "gpt-6.1-sol", ui: "plain" };
    const launch = join(dir, "launch.json"); writeFileSync(launch, JSON.stringify(message), { mode: 0o600 });
    const stderr = join(dir, "stderr");
    ok(["herdr", "--session", "corral-test", "pane", "run", pane,
      `${process.execPath} --no-env-file ${main} --launch-fd 3 3<${launch} 2>${stderr}; printf 'HOST_EXIT_%s\\n' "$?"`]);
    await fileAppears(runtime, "host.sock");
    await fileAppears(dir, "attached");
    const client = await connect(join(runtime, "host.sock"));
    const closed = client.closed();
    await client.request(frame("shutdown"));
    await within(closed, 30_000, "bridge shutdown");
    ok(["herdr", "--session", "corral-test", "pane", "wait-output", pane, "--match", "HOST_EXIT_0", "--source", "recent", "--timeout", "30000"]);
    const screen = ok(["herdr", "--session", "corral-test", "pane", "read", pane, "--source", "recent", "--lines", "400"]);
    const errors = readFileSync(stderr, "utf8");
    const counts = JSON.parse(readFileSync(join(dir, "counts.json"), "utf8"));
    console.log(`worker pane ${mode}: ${screen}\nstderr: ${JSON.stringify(errors)} counts: ${JSON.stringify(counts)}`);
    expect(screen).toContain("HOST_EXIT_0");
    expect(screen).not.toMatch(/setRawMode failed|injected raw mode EIO|\n\s+at /);
    expect(errors).toBe("");
    expect(counts.stopCalls).toBe(1);
    expect(counts.restores).toBeGreaterThan(0);
    if (mode === "emit") expect(counts.emitted).toBeGreaterThan(0);
  } finally {
    if (workspace) run(["herdr", "--session", "corral-test", "workspace", "close", workspace]);
    assets.cleanup(); rmSync(dir, { recursive: true, force: true });
  }
}, 120_000);
