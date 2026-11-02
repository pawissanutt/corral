// Runs under plain `bun`, like the compiled host: `bun test` makes any unhandled rejection in a
// worker fatal, so the kernel's discard path is only observable in a standalone process.
import { CodeKernel, type KernelEvent } from "../../src/extensions/codemode/kernel.ts";

const events: KernelEvent[] = [];
const kernel = new CodeKernel({
  session: "rejections", tools: async () => { throw new Error("tool failed"); }, onEvent: (event) => events.push(event),
});
try {
  const old = await kernel.submit("let rejectOld; new Promise((_resolve, reject) => { rejectOld = reject; }); return 1;").result;
  const later = await kernel.submit(
    'rejectOld(new Error("OLD CELL ERROR")); Promise.reject(new Error("own")); tools.fail({});' +
    " await new Promise((resolve) => setTimeout(resolve, 0)); return 2;",
  ).result;
  const after = await kernel.submit("return typeof rejectOld;").result;
  process.stdout.write(`${JSON.stringify({ results: [old, later, after], lost: events.filter((event) => event.kind === "state_lost") })}\n`);
} finally {
  await kernel.close();
}
