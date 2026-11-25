import { createHash } from "node:crypto";
import { closeSync, constants, fstatSync, lstatSync, openSync, readSync } from "node:fs";
import { join } from "node:path";
import { BridgeRefusal, parseSubmitInput, type SessionInput } from "./protocol.ts";

// Bounds one filesystem read, not the number of attachments. The 60 KiB app
// request cannot expand past this: each upload reference includes a 64-byte
// digest and expands to < 820 bytes; links add only small field overhead.
const MANIFEST_MAX = 1024 * 1024;
const OPEN = constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK;

function bad(): never {
  throw new BridgeRefusal("bad_attachment", "invalid or unavailable session attachment manifest");
}

function readManifest(sessionDir: string, name: string, bytes: number, sha256: string): unknown {
  const directory = openSync(sessionDir, OPEN | constants.O_DIRECTORY);
  try {
    const dir = fstatSync(directory);
    const uid = process.getuid?.();
    if (!dir.isDirectory() || dir.uid !== uid || (dir.mode & 0o777) !== 0o700) bad();
    // Linux can anchor lookup to the opened directory. Elsewhere, use the
    // configured private directory and check its identity around the open.
    const linux = process.platform === "linux";
    const checkDirectory = () => {
      const current = lstatSync(sessionDir);
      if (!current.isDirectory() || current.dev !== dir.dev || current.ino !== dir.ino) bad();
    };
    if (!linux) checkDirectory();
    const fd = openSync(join(linux ? `/proc/self/fd/${directory}` : sessionDir, name), OPEN);
    try {
      if (!linux) checkDirectory();
      const stat = fstatSync(fd);
      if (!stat.isFile() || stat.uid !== uid || (stat.mode & 0o077) !== 0 || stat.nlink !== 1 || stat.size !== bytes) bad();
      // Cap the read even if the file grows, and validate the snapshot before
      // decoding it. The client can unlink the file once we reply.
      const buffer = Buffer.alloc(bytes + 1);
      let length = 0;
      while (length < buffer.length) {
        const count = readSync(fd, buffer, length, buffer.length - length, null);
        if (!count) break;
        length += count;
      }
      const data = buffer.subarray(0, length);
      if (length !== bytes || createHash("sha256").update(data).digest("hex") !== sha256) bad();
      return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(data));
    } finally { closeSync(fd); }
  } finally { closeSync(directory); }
}

/** Hydrate only attachment metadata; origin and input tags stay in the socket frame. */
export function parseManifestSubmitInput(args: Record<string, unknown>, sessionDir?: string): SessionInput {
  const { attachments_manifest: manifest, ...input } = args;
  if (args.origin !== "owner" || !sessionDir || Object.hasOwn(args, "attachments") ||
      typeof manifest !== "object" || manifest === null || Array.isArray(manifest) || Object.keys(manifest).length !== 3) bad();
  const { name, bytes, sha256 } = manifest as Record<string, unknown>;
  if (typeof name !== "string" || !/^attachments-[A-Za-z0-9]{24}\.json$/.test(name) ||
      typeof bytes !== "number" || !Number.isSafeInteger(bytes) || bytes < 1 || bytes > MANIFEST_MAX ||
      typeof sha256 !== "string" || !/^[0-9a-f]{64}$/.test(sha256)) bad();
  let attachments: unknown;
  try { attachments = readManifest(sessionDir, name, bytes, sha256); }
  catch { bad(); }
  if (!Array.isArray(attachments) || !attachments.length) bad();
  return parseSubmitInput({ ...input, attachments });
}
