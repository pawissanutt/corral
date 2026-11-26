// Stop at capability registration or after the TERM-resistant command is active, so the
// parent fixture can kill this host with the lifeline in a known state.
import { mkdirSync, writeSync } from "node:fs";
import { JobRuntime } from "../../../src/extensions/jobs/runtime.ts";

const [runtimeDir, marker, active] = process.argv.slice(2);
if (!runtimeDir || !marker) throw new Error("usage: crash-host.ts <runtime-dir> <marker> [active]");
mkdirSync(runtimeDir, { recursive: true });
let launcher: number | undefined;
function freeze(identity: string): void {
  writeSync(1, `STARTED ${identity}\n`);
  process.kill(process.pid, "SIGSTOP");
}
const runtime = new JobRuntime({
  cwd: runtimeDir, runtimeDir,
  settings: { max_jobs: 1, max_output_bytes: 1024, max_runtime_secs: 60 },
  registerCapability(pid) {
    launcher = pid;
    if (!active) freeze(String(pid));
    return () => {};
  },
});
runtime.onLine((_job, line) => {
  const child = /^ACTIVE (\d+)$/.exec(line)?.[1];
  if (active && child && launcher !== undefined) freeze(`${launcher} ${child}`);
});
await runtime.start(active ? `trap '' TERM; echo "ACTIVE $$"; exec sleep 30` : `echo ran > "${marker}"`);
