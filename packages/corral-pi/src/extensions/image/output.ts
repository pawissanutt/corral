import { randomUUID } from "node:crypto";
import { accessSync, closeSync, constants, fstatSync, linkSync, lstatSync, openSync, unlinkSync, writeFileSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";
import { within } from "../../roles/assistant/paths.ts";

const MAX_BYTES = 4 * 1024 * 1024;
const SIGNATURE = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const crcTable = Uint32Array.from({ length: 256 }, (_, i) => {
  for (let bit = 0; bit < 8; bit++) i = (i >>> 1) ^ ((i & 1) ? 0xedb88320 : 0);
  return i;
});
const sampleDepths: Record<number, number[]> = { 0: [1, 2, 4, 8, 16], 2: [8, 16], 3: [1, 2, 4, 8], 4: [8, 16], 6: [8, 16] };

function checkPng(bytes: Buffer): void {
  const invalid = () => new Error("Image provider did not return a complete, supported PNG.");
  if (bytes.length > MAX_BYTES) throw new Error("Image PNG exceeds the 4 MiB limit.");
  if (bytes.length < 45 || !bytes.subarray(0, 8).equals(SIGNATURE)) throw invalid();
  let color = -1;
  let depth = 0;
  let palette = false;
  let idatBytes = 0;
  let idatStarted = false;
  let idatEnded = false;
  for (let pos = 8; pos + 12 <= bytes.length;) {
    const length = bytes.readUInt32BE(pos);
    const data = pos + 8;
    const end = data + length;
    if (end + 4 > bytes.length) throw invalid();
    const type = bytes.toString("latin1", pos + 4, data);
    if (!/^[A-Za-z]{2}[A-Z][A-Za-z]$/.test(type) || (pos === 8 && type !== "IHDR")) throw invalid();
    let crc = 0xffffffff;
    for (let i = pos + 4; i < end; i++) crc = (crc >>> 8) ^ crcTable[(crc ^ bytes[i]!) & 255]!;
    if ((~crc >>> 0) !== bytes.readUInt32BE(end)) throw invalid();
    if (idatStarted && type !== "IDAT") idatEnded = true;
    switch (type) {
      case "IHDR": {
        if (pos !== 8 || length !== 13) throw invalid();
        const width = bytes.readUInt32BE(data), height = bytes.readUInt32BE(data + 4);
        if (!width || !height || width > 4096 || height > 4096) {
          throw new Error("PNG dimensions must be between 1 and 4096 pixels per side.");
        }
        depth = bytes[data + 8]!;
        color = bytes[data + 9]!;
        if (!sampleDepths[color]?.includes(depth) || bytes[data + 10] || bytes[data + 11] || bytes[data + 12]! > 1) throw invalid();
        break;
      }
      case "PLTE":
        if (palette || idatStarted || color === 0 || color === 4 || !length || length % 3 || length > 768
            || (color === 3 && length / 3 > 2 ** depth)) throw invalid();
        palette = true;
        break;
      case "IDAT":
        if (idatEnded || (color === 3 && !palette)) throw invalid();
        idatStarted = true;
        idatBytes += length;
        break;
      case "IEND":
        if (length || !idatBytes || end + 4 !== bytes.length) throw invalid();
        return;
      default:
        // Reject unknown critical chunks and animation: the tool returns a single image.
        if (/^[A-Z]/.test(type) || type === "acTL" || type === "fcTL" || type === "fdAT") throw invalid();
    }
    pos = end + 4;
  }
  throw invalid();
}

export function validatePng(encoded: string): Buffer {
  const invalid = () => new Error("Image provider did not return canonical base64 PNG data.");
  if (encoded.length > 4 * Math.ceil(MAX_BYTES / 3)) throw new Error("Image PNG exceeds the 4 MiB limit.");
  // Round-trip also rejects missing padding, nonzero pad bits, and whitespace accepted by Buffer.
  if (!encoded.length || encoded.length % 4 || /[^A-Za-z0-9+/=]/.test(encoded)) throw invalid();
  const bytes = Buffer.from(encoded, "base64");
  if (bytes.toString("base64") !== encoded) throw invalid();
  checkPng(bytes);
  return bytes;
}

const DIRECTORY = constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_DIRECTORY;
const at = (fd: number, component: string) => `/proc/self/fd/${fd}/${component}`;
const stat = (fd: number) => fstatSync(fd, { bigint: true });
const entry = (path: string) => lstatSync(path, { bigint: true, throwIfNoEntry: false });
const same = (a: ReturnType<typeof stat>, b: ReturnType<typeof entry>) => !!b && a.dev === b.dev && a.ino === b.ino;

function walk(start: number, parts: readonly string[]): number {
  let current = openSync(at(start, "."), DIRECTORY);
  try {
    for (const part of parts) {
      const next = openSync(at(current, part), DIRECTORY);
      closeSync(current);
      current = next;
    }
    return current;
  } catch (error) {
    closeSync(current);
    throw error;
  }
}

export function preparePng(root: string, path: string): { path: string; identity: string; revalidate(): void; publish(bytes: Buffer): void; close(): void } {
  if (/[\\:\x00-\x1f\x7f\u00a0\u2000-\u200a\u202f\u205f\u3000]/.test(path) || !path.endsWith(".png")) {
    throw new Error("Choose a new .png path.");
  }
  if (process.platform !== "linux" || !isAbsolute(root)) {
    throw new Error("Safe image file publication requires an absolute session root and Linux descriptor paths.");
  }
  if (Buffer.from(path).toString("utf8") !== path) {
    throw new Error("Choose a valid UTF-8 .png path.");
  }
  const target = resolve(root, path);
  const checked = within(target, resolve(root)) ? relative(root, target) : target;
  if (checked.split("/").some(part => /^\.corral(?:$|[-.])/i.test(part) || /^\.worktrees$/i.test(part))) {
    throw new Error("Choose a .png path outside Corral state and worktrees.");
  }
  const parts = target.split("/").filter(Boolean);
  const name = parts.pop();
  if (!name || !name.endsWith(".png")) throw new Error("Choose a new .png path.");
  const rootParts = resolve(root).split("/").filter(Boolean);
  const slash = openSync("/", DIRECTORY);
  let rootFd: number | undefined;
  let parentFd: number | undefined;
  let closed = false;
  let used = false;
  const close = () => {
    if (closed) return;
    closed = true;
    try {
      if (parentFd !== undefined) closeSync(parentFd);
    } finally {
      try { if (rootFd !== undefined) closeSync(rootFd); }
      finally { closeSync(slash); }
    }
  };
  try {
    const boundRoot = rootFd = walk(slash, rootParts);
    const boundParent = parentFd = walk(slash, parts);
    const rootIdentity = stat(boundRoot);
    const parentIdentity = stat(boundParent);
    const destination = at(boundParent, name);
    const rejectExisting = () => {
      if (entry(destination)) throw new Error("Image destination already exists; choose a new path.");
    };
    const revalidate = () => {
      try {
        const freshRoot = walk(slash, rootParts);
        try {
          if (!same(rootIdentity, stat(freshRoot))) throw new Error();
          const freshParent = walk(slash, parts);
          try { if (!same(parentIdentity, stat(freshParent))) throw new Error(); }
          finally { closeSync(freshParent); }
        } finally { closeSync(freshRoot); }
      } catch {
        throw new Error("Image destination directory changed; choose a new path.");
      }
    };
    revalidate();
    rejectExisting();
    try { accessSync(at(boundParent, "."), constants.W_OK | constants.X_OK); }
    catch { throw new Error("Image destination directory must be writable."); }
    return {
      path: target,
      // Without birth time, conservatively invalidate on ctime changes to avoid inode reuse.
      identity: [rootIdentity.dev, rootIdentity.ino, rootIdentity.birthtimeNs || rootIdentity.ctimeNs,
        parentIdentity.dev, parentIdentity.ino, parentIdentity.birthtimeNs || parentIdentity.ctimeNs].join(":"),
      close,
      revalidate() { revalidate(); rejectExisting(); },
      publish(bytes) {
        if (closed || used) throw new Error("Image output is no longer open for publication.");
        used = true;
        checkPng(bytes);
        revalidate();
        rejectExisting();
        const temp = at(boundParent, `.corral-image-${randomUUID()}.tmp`);
        const stage = openSync(temp, constants.O_WRONLY | constants.O_EXCL | constants.O_CREAT | constants.O_NOFOLLOW, 0o600);
        try {
          const identity = stat(stage);
          const removeOwn = (file: string) => { if (same(identity, entry(file))) unlinkSync(file); };
          let linked = false;
          try {
            writeFileSync(stage, bytes);
            revalidate();
            if (!same(identity, entry(temp))) throw new Error("Image staging file changed before publication.");
            // Link atomically fails if any entry appeared at the destination.
            try { linkSync(temp, destination); }
            catch (error) {
              if ((error as NodeJS.ErrnoException).code === "EEXIST") rejectExisting();
              throw error;
            }
            linked = true;
            // Node cannot atomically combine the ancestry check and link; detect and
            // roll back an observed move, without resolving the original string path.
            revalidate();
            if (!same(identity, entry(destination))) throw new Error("Image output changed during publication.");
            removeOwn(temp);
          } catch (error) {
            try { if (linked) removeOwn(destination); }
            finally { removeOwn(temp); }
            throw error;
          }
        } finally { closeSync(stage); }
      },
    };
  } catch (error) {
    close();
    throw error;
  }
}
