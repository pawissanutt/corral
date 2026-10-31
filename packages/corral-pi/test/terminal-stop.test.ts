import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

for (const mode of ["throw", "emit"]) test(`terminal shutdown tolerates EIO (${mode}) and stops once`, () => {
  const module = join(import.meta.dirname, "../src/host/terminal-stop.ts");
  const script = `
    import { EventEmitter } from 'node:events';
    import { guardTerminalStop } from ${JSON.stringify(module)};
    const stdin = new EventEmitter();
    let calls = 0;
    const terminal = { stop() {
      calls++;
      const error = Object.assign(new Error('setRawMode failed'), { code: 'EIO' });
      if (${JSON.stringify(mode)} === 'throw') throw error;
      queueMicrotask(() => stdin.emit('error', error));
    }};
    guardTerminalStop(terminal, stdin);
    terminal.stop(); terminal.stop();
    queueMicrotask(() => { if (calls !== 1) process.exit(3); });
  `;
  const result = spawnSync(process.execPath, ["-e", script], { encoding: "utf8" });
  expect(result.status).toBe(0);
  expect(result.stderr).toBe("");
});
