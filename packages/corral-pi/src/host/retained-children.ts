import { randomUUID } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, join, relative } from "node:path";
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

  child(child: ChildSnapshot): void {
    if (!child.sessionFile) return;
    this.save(child.sessionFile, childRow(child), childDetail(child, false));
  }

  helper(helper: HelperInspection, file: string, result?: string): void {
    if (helper.kind === "web_search") return; // Not a native, owner-readable transcript.
    this.save(file, helperRow(helper), helperDetail(helper, false, result));
  }

  private save(file: string, row: SessionFleet["children"][number], detail: ReturnType<typeof childDetail> | ReturnType<typeof helperDetail>): void {
    try {
      const path = relative(this.root, file);
      if (!isAbsolute(file) || !/^(?:children|(?:runtime\/)?auxiliary)\/[^/]+\.jsonl$/.test(path)) return;
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
      if (!lstatSync(this.dir).isDirectory()) return;
      atomic(`${id}.json`, { session_file: path, detail });
      if (existing < 0) this.fleet.children.push(row); else this.fleet.children[existing] = row;
      this.fleet.children = boundFleet(this.fleet).children;
      atomic("fleet", this.fleet);
    } catch { /* Readback is advisory: it never changes execution, the transcript or accounting. */ }
  }
}
