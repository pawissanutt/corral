// Read once when the host starts. Reload/compaction reuse this generation's immutable
// block through the shared system prompt (including Claude); no session journal append.
import { closeSync, constants, fstatSync, openSync, readSync } from "node:fs";
import { resolve } from "node:path";
import type { Role } from "../roles/types.ts";

export function memoryContext(role: Role, file?: string, runtimeDir?: string): string {
  if (!file) return ""; // Legacy/test bootstraps; new Rust launchers require capable assets.
  if (!["pm", "assistant", "worker"].includes(role)) throw new Error("memory unavailable for role");
  if (!runtimeDir || resolve(file) !== resolve(runtimeDir, "memory-context.md")) {
    throw new Error("memory file is outside its generation");
  }
  const fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
  try {
    if (!fstatSync(fd).isFile()) throw new Error("memory context is not a regular file");
    const bytes = Buffer.alloc(12289);
    const n = readSync(fd, bytes);
    if (n > 12288) throw new Error("memory context exceeds 12288 UTF-8 bytes");
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, n));
  } finally { closeSync(fd); }
}
