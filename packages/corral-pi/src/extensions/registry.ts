// Per-generation runtime registry shared with the watchdog. The session path is bound by hello;
// the host PID separates generations that reuse a session. Entries disappear when children exit.
import { readFileSync, renameSync, writeFileSync } from "node:fs";

export type RegisterCapability = (pid: number) => () => void;

export function capabilityRegistry(sessionFile: () => string | undefined): RegisterCapability {
  const entries = new Map<number, string | null>();
  const path = () => {
    const session = sessionFile();
    return session ? `${session}.capabilities-${process.pid}.json` : undefined;
  };
  const save = () => {
    const file = path();
    if (!file) throw new Error("capability registry requires a bound session file");
    const tmp = `${file}.tmp`;
    writeFileSync(tmp, JSON.stringify([...entries].map(([pid, started]) => ({ pid, started }))), { mode: 0o600 });
    renameSync(tmp, file);
  };
  return (pid) => {
    let started: string | null = null;
    if (process.platform === "linux") {
      const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
      started = stat.slice(stat.lastIndexOf(")") + 2).split(/\s+/)[19] ?? null;
      if (!started) throw new Error("capability process has no start identity");
    }
    // Other platforms use PID alone; Linux additionally guards against PID reuse.
    entries.set(pid, started);
    save();
    return () => {
      entries.delete(pid);
      try { save(); } catch { /* shutdown must not throw from an exit handler */ }
    };
  };
}
