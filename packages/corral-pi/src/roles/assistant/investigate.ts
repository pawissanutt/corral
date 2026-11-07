// Closed host-owned Git investigation. No caller argv, cwd, executable or output paths.
// This does not authorize arbitrary shell or claim to contain hostile same-user Git binaries.
import { execFile } from "node:child_process";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const QUERIES: Readonly<Record<string, readonly string[]>> = {
  head: ["rev-parse", "--verify", "HEAD"],
  status: ["status", "--porcelain=v1", "--untracked-files=normal"],
  diff: ["diff", "--no-ext-diff", "--no-textconv", "--"],
  staged_diff: ["diff", "--cached", "--no-ext-diff", "--no-textconv", "--"],
};
export function investigationTool(cwd: string): ToolDefinition {
  return {
    name: "assistant_inspect", label: "Inspect Git facts",
    description: "Read bounded Git HEAD, status or unstaged/staged diff in this assistant's linked directory. "
      + "No shell, caller options, hooks, fsmonitor, external diff or textconv. Truncated output is labelled.",
    parameters: Type.Object({ fact: Type.Union(Object.keys(QUERIES).map(k => Type.Literal(k))) }, { additionalProperties: false }),
    async execute(_id, params: { fact: string }, signal) {
      const args = QUERIES[params.fact];
      if (!args) throw new Error("unknown investigation fact");
      const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")));
      const value = await new Promise<{ output: string; truncated: boolean }>((resolve, reject) => {
        const child = execFile("git", ["--no-pager", "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
          "-c", "diff.external=", ...args], { cwd, signal, timeout: 5000, maxBuffer: 32 * 1024,
          env: { ...env, GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: "/dev/null", GIT_OPTIONAL_LOCKS: "0", LC_ALL: "C" },
          encoding: "utf8" }, (error, stdout, stderr) => {
          if (error && error.code !== "ERR_CHILD_PROCESS_STDIO_MAXBUFFER") reject(new Error(`Git inspection failed: ${stderr || error.message}`));
          else resolve({ output: stdout, truncated: error !== null });
        });
        child.stdin?.end();
      });
      return { content: [{ type: "text", text: `${value.output}${value.truncated ? "\n[Git inspection truncated at 32 KiB]" : ""}` }], details: value };
    },
  };
}
