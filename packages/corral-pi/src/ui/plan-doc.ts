// A plan document as its review reads it: its sections by heading, and what changed between two submissions.
// Pure. A section is named by its headings from the top level down, so the same section is found again in the next
// submission wherever it moved; nothing here judges the plan.

export interface PlanSection {
  /** The heading's words; empty for what stands before the first heading. */
  readonly title: string;
  /** 1 to 3; 0 for what stands before the first heading. Deeper headings stay inside their section. */
  readonly level: number;
  /** The section's source, its heading line included. */
  readonly text: string;
  /** The titles from the top-level heading down: the section's name across submissions. */
  readonly key: string;
}
export type SectionChange = "same" | "changed" | "added";
export interface DiffLine { readonly kind: " " | "+" | "-"; readonly text: string }

const HEADING = /^ {0,3}(#{1,3})[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$/;
const FENCE = /^ {0,3}(`{3,}|~{3,})/, CLOSE = /^ {0,3}(`{3,}|~{3,})[ \t]*$/;
/** Past this many cells a line diff is not computed: the section reads as removed and written anew. */
const DIFF_CELLS = 1_000_000;

/** A document's text for drawing: tabs as four blanks, every other control character but the line break a blank. */
export function planText(text: string): string {
  return text.replace(/\r\n?/g, "\n").replaceAll("\t", "    ").replace(/[\u0000-\u0009\u000b-\u001f\u007f-\u009f]/g, " ");
}

/** The document's sections in order. A `#` inside a fenced code block is code, not a heading. */
export function planSections(markdown: string): PlanSection[] {
  const sections: PlanSection[] = [], path: string[] = [], seen = new Map<string, number>();
  let lines: string[] = [], title = "", level = 0, fence: string | undefined;
  const close = () => {
    const text = lines.join("\n").replace(/\s+$/, "");
    if (level === 0 && !text.trim()) return;
    // The heading path and which section of that path it is, as one value no heading's own text can imitate.
    const name = JSON.stringify(level === 0 ? [] : Array.from(path.slice(0, level), part => part ?? ""));
    const count = (seen.get(name) ?? 0) + 1;
    seen.set(name, count);
    sections.push({ title, level, text, key: JSON.stringify([name, count]) });
  };
  for (const line of markdown.split("\n")) {
    // A fence closes on a line of its own mark alone, at least as long as the one that opened it.
    const mark = (fence === undefined ? FENCE : CLOSE).exec(line)?.[1];
    if (mark) {
      if (fence === undefined) fence = mark;
      else if (mark[0] === fence[0] && mark.length >= fence.length) fence = undefined;
    }
    const heading = fence === undefined && !mark ? HEADING.exec(line) : null;
    if (!heading) { lines.push(line); continue; }
    close();
    lines = [line]; level = heading[1]!.length; title = heading[2]!.trim();
    path.length = level; path[level - 1] = title;
  }
  close();
  return sections;
}

/**
 * What each section of `current` is against `previous`, the section it was there, and the sections of `previous`
 * that `current` no longer has.
 */
export function sectionChanges(previous: readonly PlanSection[], current: readonly PlanSection[]):
  { readonly changes: SectionChange[]; readonly before: (PlanSection | undefined)[]; readonly removed: PlanSection[] } {
  const was = new Map(previous.map(section => [section.key, section]));
  const before = current.map(section => was.get(section.key));
  const changes = current.map((section, index): SectionChange => {
    const old = before[index];
    return !old ? "added" : old.text.trim() === section.text.trim() ? "same" : "changed";
  });
  const kept = new Set(current.map(section => section.key));
  return { changes, before, removed: previous.filter(section => !kept.has(section.key)) };
}

/** The lines of `after` against `before`: kept, added and removed, in reading order. */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before.split("\n"), b = after.split("\n");
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const x = a.slice(head, a.length - tail), y = b.slice(head, b.length - tail);
  const lines: DiffLine[] = a.slice(0, head).map(text => ({ kind: " ", text }));
  if (x.length * y.length > DIFF_CELLS) {
    for (const text of x) lines.push({ kind: "-", text });
    for (const text of y) lines.push({ kind: "+", text });
  } else {
    // The longest common run of lines, by its length from each pair of positions to the end.
    const width = y.length + 1, table = new Uint32Array((x.length + 1) * width);
    for (let i = x.length - 1; i >= 0; i--) for (let j = y.length - 1; j >= 0; j--) {
      table[i * width + j] = x[i] === y[j] ? table[(i + 1) * width + j + 1]! + 1 : Math.max(table[(i + 1) * width + j]!, table[i * width + j + 1]!);
    }
    let i = 0, j = 0;
    while (i < x.length && j < y.length) {
      if (x[i] === y[j]) { lines.push({ kind: " ", text: x[i]! }); i++; j++; }
      else if (table[(i + 1) * width + j]! >= table[i * width + j + 1]!) lines.push({ kind: "-", text: x[i++]! });
      else lines.push({ kind: "+", text: y[j++]! });
    }
    while (i < x.length) lines.push({ kind: "-", text: x[i++]! });
    while (j < y.length) lines.push({ kind: "+", text: y[j++]! });
  }
  for (let at = a.length - tail; at < a.length; at++) lines.push({ kind: " ", text: a[at]! });
  return lines;
}
