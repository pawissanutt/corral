import { spawn, type ChildProcess } from "node:child_process";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { within } from "./wait.ts";

export interface Launch {
  child: ChildProcess;
  exit: Promise<number | null>;
  stderr(): string;
  output(match: RegExp): Promise<string>;
  stop(): Promise<void>;
}

export function launch(assets: string, args: string[], cwd: string, env: NodeJS.ProcessEnv): Launch {
  const child = spawn(process.execPath, [join(assets, "src/cli.ts"), ...args], { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
  const lines = createInterface({ input: child.stdout });
  const seen: string[] = [];
  lines.on("line", line => seen.push(line));
  let stderr = "";
  child.stderr.on("data", (chunk: Buffer) => { stderr += chunk.toString(); });
  const ended = Promise.withResolvers<number | null>();
  child.once("error", ended.reject);
  child.once("close", ended.resolve);
  return {
    child, exit: ended.promise, stderr: () => stderr,
    async output(match: RegExp) {
      const found = seen.find(line => match.test(line));
      if (found) return found;
      const ready = Promise.withResolvers<string>();
      const observe = (line: string) => { if (match.test(line)) ready.resolve(line); };
      lines.on("line", observe);
      try {
        return await within(Promise.race([ready.promise, ended.promise.then(code => {
          throw new Error(`host exited ${code}\n${seen.join("\n")}\n${stderr}`);
        })]), 10_000, `host output ${match}`);
      } finally { lines.off("line", observe); }
    },
    async stop() {
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
      await ended.promise.catch(() => undefined);
    },
  };
}
