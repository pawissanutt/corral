// A host that starts one job and stalls inside its identity commit, so a test can kill it in
// exactly the window between spawn and the started record. It prints `STARTED <pid>` there.
import { JobRuntime } from "../../../src/extensions/jobs/runtime.ts";

const [runtimeDir, marker] = process.argv.slice(2);
if (!runtimeDir || !marker) throw new Error("usage: crash-host.ts <runtime-dir> <marker>");
const runtime = new JobRuntime({
  cwd: runtimeDir, runtimeDir, generation: 1,
  settings: { max_jobs: 1, max_output_bytes: 1024, max_runtime_secs: 60 },
  call: async (_op, _key, payload) => {
    if (payload.action !== "started") return { state: "committed", result: {} };
    process.stdout.write(`STARTED ${String(payload.pid)}\n`);
    return new Promise(() => undefined);
  },
});
void runtime.start(`echo ran > ${marker}`, "crash");
