// Process exits observed through Linux pidfds (T-768): a pidfd becomes readable when its process
// exits, so a test waits on that exact signal instead of polling /proc on a timer.
import { existsSync } from "node:fs";

interface Ffi {
  dlopen(path: string, symbols: Record<string, { args: string[]; returns: string }>): {
    symbols: {
      pidfd_open(pid: number, flags: number): number;
      poll(fds: unknown, count: number, timeout: number): number;
      close(fd: number): number;
    };
  };
  ptr(view: ArrayBufferView): unknown;
}

// Typed locally like src/host/peer.ts: the package's TypeScript setup has no bun:ffi declarations.
const { dlopen, ptr } = (await import("bun:ffi" as string)) as Ffi;
const libc = dlopen("libc.so.6", {
  pidfd_open: { args: ["i32", "u32"], returns: "i32" },
  poll: { args: ["ptr", "u64", "i32"], returns: "i32" },
  close: { args: ["i32"], returns: "i32" },
}).symbols;
const POLLIN = 1;

/**
 * Opens a pidfd per process, pinning its identity. A process already gone needs none; failing to
 * watch one that still exists (seccomp, fd exhaustion) throws instead of leaving it unwatched.
 */
export function watchExits(pids: readonly number[]): number[] {
  const fds: number[] = [];
  for (const pid of pids) {
    const fd = libc.pidfd_open(pid, 0);
    if (fd >= 0) fds.push(fd);
    else if (existsSync(`/proc/${pid}`)) {
      for (const open of fds) libc.close(open);
      throw new Error(`could not open a pidfd for live process ${pid}`);
    }
  }
  return fds;
}

/** Blocks until every watched process has exited, then closes the pidfds; throws after `ms`. */
export function awaitExits(fds: readonly number[], ms: number): void {
  const deadline = Date.now() + ms;
  let pending = [...fds];
  try {
    while (pending.length) {
      const left = deadline - Date.now();
      if (left <= 0) throw new Error(`${pending.length} watched process(es) did not exit within ${ms} ms`);
      const set = new Uint8Array(8 * pending.length);
      const view = new DataView(set.buffer);
      pending.forEach((fd, i) => {
        view.setInt32(8 * i, fd, true);
        view.setInt16(8 * i + 4, POLLIN, true);
      });
      // A negative result is an interrupted poll; the deadline bounds the retry.
      if (libc.poll(ptr(set), pending.length, left) < 0) continue;
      pending = pending.filter((_, i) => view.getInt16(8 * i + 6, true) === 0);
    }
  } finally {
    for (const fd of fds) libc.close(fd);
  }
}
