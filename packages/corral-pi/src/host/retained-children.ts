import { randomUUID } from "node:crypto";
import { closeSync, constants, existsSync, fstatSync, lstatSync, mkdirSync, openSync, readFileSync, readSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import type { ChildSnapshot } from "../build/types.ts";
import type { HelperInspection } from "../extensions/subagent/inspection.ts";
import { childDetail, helperDetail } from "./remote/children.ts";
import { boundFleet, childRow, confinedRegularFile, helperRow, LIVE_FLEET_BYTES, type SessionFleet } from "./remote/fleet.ts";
import { parseChildId } from "./remote/input.ts";
import { readTranscriptHeader } from "./transcript-preview.ts";
import { resolveChildRecording, retainedReference, type TranscriptRecording } from "./transcript-resolution.ts";

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

  recording(id: string): TranscriptRecording | undefined {
    try {
      const candidate = join(this.dir, `${parseChildId(id)}.json`);
      const path = confinedRegularFile(this.root, candidate);
      if (!path) return undefined;
      const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
      try {
        const stat = fstatSync(fd);
        if (!stat.isFile() || stat.size > 256 * 1024) return undefined;
        const buffer = Buffer.alloc(stat.size);
        if (readSync(fd, buffer, 0, buffer.length, 0) !== buffer.length) return undefined;
        if (confinedRegularFile(this.root, candidate) !== path) return undefined;
        const pathStat = lstatSync(path);
        if (pathStat.dev !== stat.dev || pathStat.ino !== stat.ino || pathStat.size !== stat.size || pathStat.mtimeMs !== stat.mtimeMs) return undefined;
        const record = JSON.parse(buffer.toString("utf8"));
        if (record.detail?.id !== id || typeof record.detail.kind !== "string" || record.detail.kind === "helper:web_search") return undefined;
        const reference = retainedReference(this.root, record.session_file, record.session_id);
        return reference && resolveChildRecording(this.root, id, reference);
      } finally { closeSync(fd); }
    } catch { return undefined; }
  }

  child(child: ChildSnapshot): boolean {
    return !!child.sessionFile && this.save(child.sessionFile, childRow(child), childDetail(child, false));
  }

  helper(helper: HelperInspection, file: string, result?: string): boolean {
    if (helper.kind === "web_search") return false; // Not a native, owner-readable transcript.
    return this.save(file, helperRow(helper), helperDetail(helper, false, result));
  }

  /** True only when exact-ID readback has both the index record and a native file. */
  private save(file: string, row: SessionFleet["children"][number], detail: ReturnType<typeof childDetail> | ReturnType<typeof helperDetail>): boolean {
    const path = relative(this.root, file);
    if (retainedReference(this.root, path, undefined)?.file !== file) return false;
    const confined = confinedRegularFile(this.root, file);
    const session = confined && readTranscriptHeader(confined);
    try {
      const id = parseChildId(row.id);
      if (path.startsWith("children/") && path !== `children/${id}.jsonl`) return false;
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
      atomic(`${id}.json`, { session_file: path, session_id: session, detail });
      if (existing < 0) this.fleet.children.push(row); else this.fleet.children[existing] = row;
      this.fleet.children = boundFleet(this.fleet).children;
      atomic("fleet", this.fleet);
      return !!confined;
    } catch { return false; /* Readback never changes execution, the transcript or accounting. */ }
  }
}
