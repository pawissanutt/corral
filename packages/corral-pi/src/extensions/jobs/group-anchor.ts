// A retained Linux PID file descriptor can signal the launcher's group even when
// every member is suspended. It never resolves a numeric PID again after acquisition.
import { closeSync } from "node:fs";
import { FFIType, dlopen } from "bun:ffi";
import { constants } from "node:os";

const PROCESS_GROUP = 1 << 2; // PIDFD_SIGNAL_PROCESS_GROUP (Linux 6.9)
const i32 = FFIType.i32, ptr = FFIType.ptr;
const signature = {
  pidfd_open: { args: [i32, i32], returns: i32 },
  pidfd_send_signal: { args: [i32, i32, ptr, i32], returns: i32 },
};
let libc: ReturnType<typeof dlopen<typeof signature>>["symbols"] | null | undefined;

export interface GroupAnchor {
  force(): boolean;
  close(): void;
}

/** Called while the launcher is gated, with an identity check spanning acquisition. */
export function openGroupAnchor(pid: number, stillOurs: () => boolean): GroupAnchor | undefined {
  if (process.platform !== "linux") return undefined;
  if (libc === undefined) {
    try { libc = dlopen("libc.so.6", signature).symbols; }
    catch { libc = null; }
  }
  const api = libc;
  if (!api) return undefined;
  const acquired = api.pidfd_open(pid, 0) as number;
  if (acquired < 0) return undefined;
  // Signal 0 is only a query: detect old kernels and revalidate the gated launcher.
  if (!stillOurs() || api.pidfd_send_signal(acquired, 0, 0, PROCESS_GROUP) !== 0) {
    closeSync(acquired);
    return undefined;
  }
  let fd: number | undefined = acquired;
  return {
    force: () => fd !== undefined && api.pidfd_send_signal(fd, constants.signals.SIGKILL, 0, PROCESS_GROUP) === 0,
    close() { if (fd !== undefined) { const current = fd; fd = undefined; closeSync(current); } },
  };
}
