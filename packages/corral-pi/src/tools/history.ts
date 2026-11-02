// Fixed read-only command vocabulary. No paths, shell text, restore or session replacement API.
import { execFile } from "node:child_process";
export type HistoryRequest =
  | { kind: "tasks"; query: string }
  | { kind: "assistants" }
  | { kind: "task"; task: string }
  | { kind: "questions"; task: string }
  | { kind: "assistant"; assistant: string }
  | { kind: "recovery"; task: string; run: string }
  | { kind: "transcript"; task: string; run: string; first: number };
export function historyArgs(request: HistoryRequest): string[] {
  const task = (id: string) => { if (!/^T-\d+$/.test(id)) throw new Error("Invalid history task"); return id; };
  const run = (id: string) => { if (!/^R\d+$/.test(id)) throw new Error("Invalid history run"); return id; };
  switch (request.kind) {
    case "tasks": return ["list", "--all", "--query", request.query.slice(0, 256), "--json"];
    case "assistants": return ["assistant", "list", "--json"];
    case "task": return ["show", task(request.task), "--json", "--recorded"];
    case "questions": return ["question", "list", "--task", task(request.task), "--json"];
    case "assistant":
      if (!/^A-\d+$/.test(request.assistant)) throw new Error("Invalid history assistant");
      return ["assistant", "show", request.assistant, "--json"];
    case "recovery": return ["agent", "rewind", task(request.task), "--run", run(request.run), "--list"];
    case "transcript":
      if (!Number.isSafeInteger(request.first) || request.first < 1 || request.first > 1_000_000) throw new Error("Invalid history page");
      return ["run", "transcript", task(request.task), run(request.run), "--lines", `${request.first}-${request.first + 199}`];
  }
}
export function readHistory(corral: string, root: string, request: HistoryRequest): Promise<unknown> {
  const args = historyArgs(request);
  return new Promise((resolve, reject) => {
    const child = execFile(corral, ["--root", root, ...args], { cwd: root, timeout: 30_000, maxBuffer: 4 * 1024 * 1024 }, (error, stdout) => {
      if (error) { reject(new Error(`History unavailable: ${error.message}`)); return; }
      try { resolve(request.kind === "transcript" ? stdout : JSON.parse(stdout)); }
      catch { reject(new Error("Invalid history response")); }
    });
    child.stdin?.end();
  });
}
