// Rows for Pi's file tools: `read`, `edit`, `write`, `grep`, `find` and `ls`. A finished call is one row with what it
// touched and a count (`27 lines`, `+2 −2 in 2 hunks`, `35+ matches · 7 files`); a failure adds its reason. While the
// arguments stream, `write` shows its newest lines and `edit` its growing replacement. Expanded, a row shows the SDK's
// own full views: the highlighted file text and the rendered diff. Presentation only: results are read, never changed.
import { getLanguageFromPath, highlightCode, renderDiff } from "@earendil-works/pi-coding-agent";
import { Text, type Component } from "@earendil-works/pi-tui";

import { baseState, numberArg, recordOf, resultText, shortPath, stringArg, textLines } from "./tool-row.ts";
import type { BodyLine, CallInput, Presenter, RowModel } from "./tool-row.ts";
import type { PresenterModule } from "./tool-rendering.ts";

/** Rows a streaming `write` keeps once two lines exist, so the row grows once. */
const WRITE_TAIL_ROWS = 8;
/** At most this many rows show the replacement an `edit` is writing. */
const EDIT_STREAM_ROWS = 8;
/** A failed call shows at most this many reason rows. */
const REASON_ROWS = 3;

type Args = Readonly<Record<string, unknown>>;

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count.toLocaleString("en-US")} ${count === 1 ? one : many}`;
}
function size(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
/** Lines of `value` without the trailing empty ones. */
function lines(value: string): string[] {
  const split = value.replace(/\r\n?/g, "\n").split("\n");
  while (split.length > 0 && split.at(-1) === "") split.pop();
  return split;
}
function pathOf(args: Args): string {
  return stringArg(args, "path") ?? stringArg(args, "file_path") ?? "";
}
/** The call's file or directory as the row's target: relative to the session cwd, clipped in the middle. */
function target(path: string, cwd: string, suffix = ""): Pick<RowModel, "target" | "targetClip"> {
  return path ? { target: shortPath(path, cwd) + suffix, targetClip: "middle" } : {};
}

/** A result's text without the tool's trailing `[...]` notice block, and that notice. */
function withoutNotice(value: string): { readonly body: string; readonly notice?: string } {
  const end = value.trimEnd(), at = end.lastIndexOf("\n\n[");
  if (at < 0 || !end.endsWith("]")) return { body: value };
  return { body: end.slice(0, at), notice: end.slice(at + 3, -1) };
}
/** A failure's outcome and up to three reason rows: a known file error reads in words, with the whole reason below. */
function failure(call: CallInput): Pick<RowModel, "outcome" | "outcomeTone" | "body"> {
  const reason = lines(resultText(call.result)).filter(line => line.trim() !== "");
  const first = reason[0] ?? "failed";
  const code = /\b(ENOENT|ENOTDIR|EISDIR|EACCES|EPERM|EEXIST)\b/.exec(first)?.[1];
  const short = code === "ENOENT" ? "not found" : code === "ENOTDIR" ? "not a directory" : code === "EISDIR" ? "is a directory"
    : code === "EACCES" || code === "EPERM" ? "permission denied" : code === "EEXIST" ? "already exists" : undefined;
  const rows = (short ? reason : reason.slice(1)).slice(0, REASON_ROWS).map((line): BodyLine => ({ text: line, tone: "muted" }));
  return { outcome: short ?? first, outcomeTone: "error", ...(rows.length > 0 ? { body: rows } : {}) };
}
/** File text through the SDK's highlighter, as the SDK's own expanded views draw it. */
function highlighted(content: string, path: string): Component {
  const lang = path ? getLanguageFromPath(path) : undefined, shown = content.replace(/\t/g, "   ");
  return new Text(lang ? highlightCode(shown, lang).join("\n") : shown, 0, 0);
}
/** The SDK's rendered diff. Selecting copies the code: each line's first row leaves out its sign and line number. */
export function diffView(diff: string, path: string): Component {
  const text: Component = new Text(renderDiff(diff, { filePath: path }), 0, 0);
  return {
    render: width => text.render(width),
    invalidate: () => text.invalidate(),
    selectionRows: (width, y) => {
      let line: number | undefined;
      return text.selectionRows?.(width, y)?.map(row => {
        if (row.logicalLine === line) return row;
        line = row.logicalLine;
        const gutter = /^[-+ ] *\d+ /.exec(row.text)?.[0] ?? "";
        return { ...row, left: row.left + gutter.length, text: row.text.slice(gutter.length) };
      });
    },
  };
}

function readRange(args: Args): string {
  const offset = numberArg(args, "offset"), limit = numberArg(args, "limit");
  if (offset === undefined && limit === undefined) return "";
  const start = offset ?? 1;
  return limit === undefined ? `:${start}` : `:${start}-${start + Math.max(1, limit) - 1}`;
}
const presentRead: Presenter = call => {
  const path = pathOf(call.args), state = baseState(call);
  const base: RowModel = { state, verb: "read", ...target(path, call.cwd, readRange(call.args)) };
  if (state === "failed") return { ...base, ...failure(call) };
  if (state !== "ok") return base;
  if (call.result?.content.some(part => part.type === "image")) return { ...base, outcome: "image" };
  if (recordOf(recordOf(call.result?.details)?.truncation)?.firstLineExceedsLimit === true) {
    return { ...base, outcome: "first line too long", outcomeTone: "warning" };
  }
  const { body, notice } = withoutNotice(resultText(call.result));
  const expanded = body.trim() ? { expanded: () => highlighted(body, path) } : {};
  // A clipped preview (a Code Mode child's result) counts nothing: only the recorded totals do.
  if (call.totals) return { ...base, ...(call.totals.lines !== undefined ? { outcome: plural(call.totals.lines, "line") } : {}), ...expanded };
  const count = lines(body).length;
  const showing = notice ? /^Showing lines (\d+)-(\d+) of (\d+)/.exec(notice) : null;
  const more = notice ? /^(\d+) more lines in file/.exec(notice) : null;
  const outcome = showing ? `${(Number(showing[2]) - Number(showing[1]) + 1).toLocaleString("en-US")} of ${plural(Number(showing[3]), "line")} · more`
    : more ? `${plural(count, "line")} · ${Number(more[1]).toLocaleString("en-US")} more` : plural(count, "line");
  return { ...base, outcome, ...expanded };
};

interface Edit { readonly oldText: string; readonly newText: string }
/** The edits as far as they streamed: Pi's `edits` list, or the single old/new pair of other spellings. */
function editsOf(args: Args): Edit[] {
  const list: readonly unknown[] = Array.isArray(args.edits) ? args.edits : [args];
  return list.flatMap(item => {
    const edit = recordOf(item);
    if (!edit) return [];
    const oldText = stringArg(edit, "oldText") ?? stringArg(edit, "old_text") ?? stringArg(edit, "old_string");
    const newText = stringArg(edit, "newText") ?? stringArg(edit, "new_text") ?? stringArg(edit, "new_string");
    return oldText === undefined && newText === undefined ? [] : [{ oldText: oldText ?? "", newText: newText ?? "" }];
  });
}
/** `+a −d in n hunks` from the unified patch, or from the display diff when the patch is absent. */
export function diffStat(patch: string | undefined, diff: string | undefined): string | undefined {
  let added = 0, removed = 0, hunks = 0;
  if (patch) {
    for (const line of patch.split("\n")) {
      if (line.startsWith("@@")) hunks++;
      else if (line.startsWith("+") && !line.startsWith("+++")) added++;
      else if (line.startsWith("-") && !line.startsWith("---")) removed++;
    }
  } else if (diff) {
    let changing = false;
    for (const line of diff.split("\n")) {
      const change = line.startsWith("+") || line.startsWith("-");
      if (change && !changing) hunks++;
      changing = change;
      if (line.startsWith("+")) added++;
      else if (line.startsWith("-")) removed++;
    }
  } else return undefined;
  return `+${added} \u2212${removed} in ${plural(hunks, "hunk")}`;
}
const presentEdit: Presenter = call => {
  const path = pathOf(call.args), edits = editsOf(call.args), state = baseState(call);
  const base: RowModel = { state, verb: "edit", ...target(path, call.cwd) };
  if (state === "failed") return { ...base, ...failure(call) };
  if (state === "writing") {
    // The edit being written: the end of the text it replaces, then its replacement as it grows. How many edits
    // follow is unknown until the arguments are complete, so the label names this edit, not a total.
    const current = edits.at(-1);
    if (!current) return base;
    const removed = lines(current.oldText).slice(-2).map((line): BodyLine => ({ text: `- ${line}`, tone: "removed" }));
    const added = lines(current.newText).map((line): BodyLine => ({ text: `+ ${line}`, tone: "added" }));
    return { ...base, outcome: `edit ${edits.length}`, body: [...removed, ...added].slice(-EDIT_STREAM_ROWS) };
  }
  if (state !== "ok") return { ...base, ...(edits.length > 1 ? { outcome: plural(edits.length, "edit") } : {}) };
  const details = recordOf(call.result?.details), diff = stringArg(details ?? {}, "diff");
  const stat = diffStat(stringArg(details ?? {}, "patch"), diff);
  return { ...base, ...(stat ? { outcome: stat } : {}),
    ...(diff ? { expanded: () => diffView(diff, path) } : {}) };
};

const presentWrite: Presenter = call => {
  const path = pathOf(call.args), content = stringArg(call.args, "content") ?? "", state = baseState(call);
  const base: RowModel = { state, verb: "write", ...target(path, call.cwd) };
  if (state === "failed") return { ...base, ...failure(call) };
  const all = lines(content), stat = `${plural(all.length, "line")} · ${size(Buffer.byteLength(content))}`;
  if (state === "writing") {
    if (all.length < 2) return { ...base, ...(content ? { outcome: stat } : {}) };
    // The newest lines, padded to a fixed height so the row grows once while the file streams.
    const tail = all.slice(-WRITE_TAIL_ROWS).map((line): BodyLine => ({ text: line, tone: "text" }));
    while (tail.length < WRITE_TAIL_ROWS) tail.push({ text: "" });
    return { ...base, outcome: stat, body: tail };
  }
  return { ...base, outcome: stat, ...(content ? { expanded: () => highlighted(content, path) } : {}) };
};

/** A search result's rows without the notice block. */
function resultRows(call: CallInput): string[] {
  return lines(withoutNotice(resultText(call.result)).body);
}
function scope(args: Args, cwd: string): string {
  const path = stringArg(args, "path");
  return path ? shortPath(path, cwd) : ".";
}
/** `n+ things` when the tool stopped at its limit, else the exact count. */
function counted(count: number, limit: number | undefined, one: string, many?: string): string {
  return limit !== undefined ? `${limit.toLocaleString("en-US")}+ ${many ?? `${one}s`}` : plural(count, one, many);
}
const presentGrep: Presenter = call => {
  const pattern = stringArg(call.args, "pattern") ?? "", glob = stringArg(call.args, "glob"), state = baseState(call);
  const base: RowModel = { state, verb: "grep", target: `${pattern ? `"${pattern}" ` : ""}in ${scope(call.args, call.cwd)}${glob ? ` (${glob})` : ""}` };
  if (state === "failed") return { ...base, ...failure(call) };
  if (state !== "ok") return base;
  const rows = resultRows(call);
  if (rows.length === 0 || (rows.length === 1 && rows[0] === "No matches found")) return { ...base, outcome: "no matches", outcomeTone: "muted" };
  const limit = numberArg(recordOf(call.result?.details) ?? {}, "matchLimitReached");
  const expanded = () => textLines(rows.join("\n"), "text");
  if (call.totals) {
    // A clipped preview cannot be counted: say only what the totals and the limit know.
    const hint = call.totals.lines !== undefined ? { hiddenHint: plural(call.totals.lines, "line") } : {};
    return { ...base, ...(limit !== undefined ? { outcome: counted(0, limit, "match", "matches") } : {}), ...hint, expanded };
  }
  const files = new Set<string>();
  let matches = 0;
  for (const row of rows) {
    const match = /^(.+?):(\d+): /.exec(row);
    if (match) { matches++; files.add(match[1]!); }
  }
  return { ...base, outcome: `${counted(matches, limit, "match", "matches")} · ${plural(files.size, "file")}`,
    hiddenHint: plural(rows.length, "line"), expanded };
};
/** The count of a listing: the recorded total for a clipped preview (none when unrecorded), else the rows shown. */
function listCount(call: CallInput, rows: readonly string[]): number | undefined {
  return call.totals ? call.totals.lines : rows.length;
}
const presentFind: Presenter = call => {
  const pattern = stringArg(call.args, "pattern") ?? "", state = baseState(call);
  const base: RowModel = { state, verb: "find", target: `${pattern ? `${pattern} ` : ""}in ${scope(call.args, call.cwd)}` };
  if (state === "failed") return { ...base, ...failure(call) };
  if (state !== "ok") return base;
  const rows = resultRows(call);
  if (rows.length === 0 || (rows.length === 1 && rows[0] === "No files found matching pattern")) return { ...base, outcome: "no files", outcomeTone: "muted" };
  const limit = numberArg(recordOf(call.result?.details) ?? {}, "resultLimitReached"), count = listCount(call, rows);
  return { ...base, ...(count !== undefined || limit !== undefined ? { outcome: counted(count ?? 0, limit, "file") } : {}),
    expanded: () => textLines(rows.join("\n"), "text") };
};
const presentLs: Presenter = call => {
  const state = baseState(call);
  const base: RowModel = { state, verb: "ls", target: scope(call.args, call.cwd), targetClip: "middle" };
  if (state === "failed") return { ...base, ...failure(call) };
  if (state !== "ok") return base;
  const rows = resultRows(call);
  if (rows.length === 0 || (rows.length === 1 && rows[0] === "(empty directory)")) return { ...base, outcome: "empty", outcomeTone: "muted" };
  const limit = numberArg(recordOf(call.result?.details) ?? {}, "entryLimitReached"), count = listCount(call, rows);
  return { ...base, ...(count !== undefined || limit !== undefined ? { outcome: counted(count ?? 0, limit, "entry", "entries") } : {}),
    expanded: () => textLines(rows.join("\n"), "text") };
};

const PRESENTERS: Readonly<Record<string, Presenter>> = {
  read: presentRead, edit: presentEdit, write: presentWrite, grep: presentGrep, find: presentFind, ls: presentLs,
};

/** `read`, `edit`, `write`, `grep`, `find` and `ls`; undefined for any other tool. */
export const filePresenters: PresenterModule = {
  presenter: name => PRESENTERS[name],
};
