import { randomUUID } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative } from "node:path";
import type { ChildSnapshot } from "../build/types.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { childDetail, helperDetail } from "./remote/children.ts";
import { boundFleet, childRow, helperRow, LIVE_FLEET_BYTES, type SessionFleet } from "./remote/fleet.ts";
import { parseChildId } from "./remote/input.ts";

/** A parent-local readback index, separate from SDK-managed transcripts and unbounded live events. */
export class RetainedChildren {
  private readonly dir: string;
  private readonly fleet: SessionFleet = { children: [], messages: [], messages_total: 0, max_parallel: 0 };
  constructor(private readonly root: string) {
    this.dir = join(root, "child-records");
    const path = join(this.dir, "fleet");
    try {
      const stat = lstatSync(path);
      if (!lstatSync(this.dir).isDirectory() || !stat.isFile() || stat.size > LIVE_FLEET_BYTES) return;
      const previous = JSON.parse(readFileSync(path, "utf8")) as Partial<SessionFleet>;
      if (Array.isArray(previous.children)) this.fleet.children = previous.children;
    } catch { /* No usable previous roster; per-child records are independent. */ }
  }

  has(id: string): boolean { return existsSync(join(this.dir, `${parseChildId(id)}.json`)); }

  child(child: ChildSnapshot): boolean {
    return !!child.sessionFile && this.save(child.sessionFile, childRow(child), childDetail(child, false));
  }

  helper(helper: HelperInspection, file: string, result?: string): boolean {
    if (helper.kind === "web_search") return false; // Not a native, owner-readable transcript.
    return this.save(file, helperRow(helper), helperDetail(helper, false, result));
  }

  /** True only when exact-ID readback has both the index record and a native file. */
  private save(file: string, row: SessionFleet["children"][number], detail: ReturnType<typeof childDetail> | ReturnType<typeof helperDetail>): boolean {
    let indexed = false;
    const path = relative(this.root, file);
    if (!isAbsolute(file) || !/^(?:children|(?:runtime\/)?auxiliary)\/[^/]+\.jsonl$/.test(path)) return false;
    try {
      const id = parseChildId(row.id);
      const existing = this.fleet.children.findIndex(prior => prior.id === id);
      const atomic = (name: string, value: unknown) => {
        const target = join(this.dir, name);
        const temporary = join(this.dir, `${randomUUID()}.tmp`);
        try {
          writeFileSync(temporary, JSON.stringify(value), { flag: "wx", mode: 0o600 });
          renameSync(temporary, target);
        } finally { rmSync(temporary, { force: true }); }
      };
      mkdirSync(this.dir, { recursive: true, mode: 0o700 });
      if (!lstatSync(this.dir).isDirectory()) return false;
      atomic(`${id}.json`, { session_file: path, detail });
      indexed = true;
      if (existing < 0) this.fleet.children.push(row); else this.fleet.children[existing] = row;
      this.fleet.children = boundFleet(this.fleet).children;
      atomic("fleet", this.fleet);
    } catch { /* Readback is advisory: it never changes execution, the transcript or accounting. */ }
    if (!indexed) return false;
    try {
      return (!path.startsWith("runtime/") || lstatSync(join(this.root, "runtime")).isDirectory())
        && lstatSync(dirname(file)).isDirectory() && lstatSync(file).isFile();
    } catch { return false; }
  }
}
