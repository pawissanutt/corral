import { closeSync, constants as f, fstatSync, openSync, readSync } from "node:fs";
import { join } from "node:path";
import { TOML } from "bun";
import { dlopen, FFIType, ptr, read } from "bun:ffi";

function loadFileOps() {
  const apple = process.platform === "darwin";
  const errno = apple ? "__error" : "__errno_location";
  const { symbols } = dlopen(apple ? "libSystem.B.dylib" : "libc.so.6", {
    openat: { args: [FFIType.i32, FFIType.ptr, FFIType.i32], returns: FFIType.i32 },
    [errno]: { args: [], returns: FFIType.ptr },
  });
  return { openat: symbols.openat, errno: symbols[errno]!, cloexec: apple ? 0x01000000 : 0o2000000 };
}
let native: ReturnType<typeof loadFileOps> | null | undefined;

function openChild(dir: number, name: ".corral-next" | "pi.toml", flags: number): number {
  // Darwin's /dev/fd is not a traversable alias to the directory. Use openat(2)
  // relative to the retained descriptor, never a second lookup from the workspace path.
  if (native === undefined) {
    try { native = loadFileOps(); }
    catch (error) {
      if (process.platform !== "linux") throw error;
      native = null; // Linux without glibc still has descriptor-relative procfs paths.
    }
  }
  if (!native) return openSync(join("/proc/self/fd", String(dir), name), flags);
  const child = Buffer.from(`${name}\0`);
  const fd = native.openat(dir, ptr(child), flags | native.cloexec) as number;
  if (fd >= 0) return fd;
  const missing = read.i32(native.errno() as number) === 2;
  throw Object.assign(new Error("could not open workspace setting"), { code: missing ? "ENOENT" : "EIO" });
}

/** Match the daemon's regular-file, no-follow and byte bounds. Parser errors can quote secrets. */
export function readPiSettings(root: string): Record<string, unknown> {
  const directory = f.O_RDONLY | f.O_DIRECTORY | f.O_NOFOLLOW;
  const handles: number[] = [];
  try {
    handles.push(openSync(root, directory));
    handles.push(openChild(handles[0]!, ".corral-next", directory));
    const fd = openChild(handles[1]!, "pi.toml", f.O_RDONLY | f.O_NOFOLLOW | f.O_NONBLOCK);
    handles.push(fd);
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.size > 256 * 1024) throw new Error("invalid settings file");
    const bytes = Buffer.alloc(stat.size + 1);
    let length = 0;
    while (length < bytes.length) {
      const n = readSync(fd, bytes, length, bytes.length - length, null);
      if (!n) break;
      length += n;
    }
    if (length !== stat.size) throw new Error("settings changed during read");
    return TOML.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, length))) as Record<string, unknown>;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw new Error(".corral-next/pi.toml: invalid or unreadable Pi configuration");
  } finally { for (const fd of handles.reverse()) closeSync(fd); }
}
