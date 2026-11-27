// Standalone fixture-fault entrypoint. Normal builds compile main.ts directly.
import { dlopen, FFIType, read } from "bun:ffi";
import { main } from "./main.ts";

function tcpSocketDenied(): boolean {
  // Bun 1.4.2 node:net reports ECONNREFUSED even when socket(2) returns
  // EPERM. Observe the exact errno via the already staged glibc instead;
  // the probe only creates a socket, never connects or sends any traffic.
  const libc = dlopen("libc.so.6", {
    socket: { args: [FFIType.i32, FFIType.i32, FFIType.i32], returns: FFIType.i32 },
    close: { args: [FFIType.i32], returns: FFIType.i32 },
    __errno_location: { args: [], returns: FFIType.ptr },
  });
  try {
    const errnoAddress = libc.symbols.__errno_location();
    if (typeof errnoAddress !== "number" || errnoAddress === 0) return false;
    // Linux x86-64 ABI: AF_INET, SOCK_STREAM | SOCK_CLOEXEC, IPPROTO_TCP.
    const fd = libc.symbols.socket(2, 1 | 0x80000, 6);
    if (typeof fd === "number" && fd >= 0) libc.symbols.close(fd);
    return fd === -1 && read.i32(errnoAddress) === 1; // EPERM
  } finally {
    libc.close();
  }
}

function probeSandbox(): void {
  try {
    if (!tcpSocketDenied()) process.exit(1);
    // The only allowed output is the fixed private stdio receipt. Bound a
    // stalled peer and exit after the write rather than running the worker.
    setTimeout(() => process.exit(1), 1000);
    process.stdout.once("error", () => process.exit(1));
    process.stdout.write('{"type":"sandbox_denied"}\n', (error) => process.exit(error ? 1 : 0));
  } catch {
    process.exit(1);
  }
}

if (import.meta.main) {
  if (process.argv.length === 3 && process.argv[2] === "--probe-sandbox") probeSandbox();
  else await main();
}
