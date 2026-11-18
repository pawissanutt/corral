// Stop at capability registration: the supervisor exists but has not been sent "run".
// The test kills this stopped host and checks that the supervisor exits without launching work.
import { mkdirSync, writeSync } from "node:fs";
import { JobRuntime } from "../../../src/extensions/jobs/runtime.ts";

const [runtimeDir, marker] = process.argv.slice(2);
if (!runtimeDir || !marker) throw new Error("usage: crash-host.ts <runtime-dir> <marker>");
mkdirSync(runtimeDir, { recursive: true });
const runtime = new JobRuntime({
  cwd: runtimeDir, runtimeDir,
  settings: { max_jobs: 1, max_output_bytes: 1024, max_runtime_secs: 60 },
  registerCapability(pid) {
    writeSync(1, `STARTED ${pid}\n`);
    process.kill(process.pid, "SIGSTOP");
    return () => {};
  },
});
await runtime.start(`echo ran > "${marker}"`);
