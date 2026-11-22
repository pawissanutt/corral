import { execFile } from "node:child_process";
import { resolve as absolute } from "node:path";

export interface WorkspaceStartup {
  readonly revision: string;
  readonly model: string | null;
}

function isRevision(value: unknown): value is string {
  return value === "missing" || (typeof value === "string" && value.length === 64 && /^[0-9a-f]{64}$/u.test(value));
}

function isModel(value: unknown): value is string | null {
  return value === null || (typeof value === "string" && value.length > 0 && value.length <= 256 && Buffer.byteLength(value) <= 256);
}

function publicError(action: "read" | "write", stderr: string): string {
  // Never return raw child output (even for a different or older executable).
  switch (stderr.trim()) {
    case "error: pi.toml changed; reload the startup setting before saving":
    case "error: Pi settings are being saved; reload and try again":
      return "Workspace startup settings changed or are being saved. Reload and try again.";
    case "error: pi.toml startup.model is invalid; correct it on the workspace host":
      return "Workspace startup.model is invalid. Correct it in .corral-next/pi.toml.";
    case "error: pi.toml contains invalid TOML":
    case "error: pi.toml startup must be a table":
      return "Workspace Pi settings are invalid. Check .corral-next/pi.toml.";
    default:
      return action === "read"
        ? "Could not read the workspace startup model."
        : "Could not save the workspace startup model. Reload before trying again.";
  }
}

async function run(root: string, corral: string, action: "read" | "write", input?: string): Promise<WorkspaceStartup> {
  if (!root || !corral) throw new Error("No Corral workspace is connected.");
  const cwd = absolute(root);
  const stdout = await new Promise<string>((resolve, reject) => {
    try {
      const child = execFile(corral, ["--root", cwd, "pi-startup", action], {
        cwd, shell: false, encoding: "utf8", timeout: 5_000, killSignal: "SIGKILL", maxBuffer: 4 * 1024,
      }, (error, output, stderr) => {
        if (error) reject(new Error(publicError(action, stderr)));
        else resolve(output);
      });
      // Early CLI failure can close stdin while the request is being written.
      child.stdin?.on("error", () => {});
      child.stdin?.end(input);
    } catch {
      reject(new Error(publicError(action, "")));
    }
  });
  let result: Partial<WorkspaceStartup> | null = null;
  try { result = JSON.parse(stdout); } catch { /* never expose the response text */ }
  if (!isRevision(result?.revision) || !isModel(result?.model)) throw new Error(publicError(action, ""));
  return { revision: result.revision, model: result.model };
}

export function readWorkspaceStartup(root: string, corral: string): Promise<WorkspaceStartup> {
  return run(root, corral, "read");
}

export async function saveWorkspaceStartup(root: string, corral: string, expected: WorkspaceStartup, model: string | null): Promise<WorkspaceStartup> {
  if (!isRevision(expected.revision)) throw new Error("Reload the workspace startup model before saving.");
  if (!isModel(model)) throw new Error("Invalid workspace startup model.");
  return run(root, corral, "write", JSON.stringify({ revision: expected.revision, model }));
}
