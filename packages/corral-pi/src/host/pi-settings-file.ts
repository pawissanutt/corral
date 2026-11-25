import { closeSync, constants as f, fstatSync, openSync, readSync } from "node:fs";
import { join } from "node:path";
import { TOML } from "bun";

/** Match the daemon's regular-file, no-follow and byte bounds. Parser errors can quote secrets. */
export function readPiSettings(root: string): Record<string, unknown> {
  const directory = f.O_RDONLY | f.O_DIRECTORY | f.O_NOFOLLOW;
  const handles: number[] = [];
  const child = (name: string) => join(process.platform === "linux" ? "/proc/self/fd" : "/dev/fd", String(handles.at(-1)), name);
  try {
    handles.push(openSync(root, directory));
    handles.push(openSync(child(".corral-next"), directory));
    const fd = openSync(child("pi.toml"), f.O_RDONLY | f.O_NOFOLLOW | f.O_NONBLOCK);
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
