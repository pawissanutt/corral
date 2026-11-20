/** The most a child's brief may hold (`FleetRuntime.start` refuses a longer one). */
export const BRIEF_LIMIT = 32768;

export interface ReviewBriefParts {
  /** What the reviewer always needs, in order before the variable sections. */
  head: string[];
  /** The owner's prompts, verbatim while they fit. */
  prompts: string[];
  /** One entry per slice: its id, title, owned paths and report. */
  slices: string[];
  /** What follows the variable sections: the gate, the focus, how to finish. */
  tail: string[];
  /** Where the full prompts and reports stay, named when any of them was shortened. */
  record: string;
}

const JOIN = "\n\n";

/**
 * The reviewer's brief within `BRIEF_LIMIT`. A long build's slice reports and prompts together
 * outgrew the limit, and the fleet refused the reviewer, so no build of that size could be
 * reviewed. The brief is whole when it fits. Otherwise the head and tail stay whole and the
 * prompts and slices share what is left, each shortened only as far as the others leave no
 * room for it (the shortest stay whole), with a note saying how much went and where the whole
 * text is; when even those notes do not fit, one line stands for them all.
 */
export function reviewBrief(parts: ReviewBriefParts): string {
  const whole = assemble(parts, parts.prompts, parts.slices, false);
  if (whole.length <= BRIEF_LIMIT) return whole;
  const fixed = assemble(parts, [], [], true).length;
  const items = [...parts.prompts, ...parts.slices];
  const fitted = fit(items, BRIEF_LIMIT - fixed);
  if (!fitted) throw new Error(`The review brief needs ${fixed} characters before any prompt or report; shorten the focus.`);
  const prompts = fitted.length === items.length ? fitted.slice(0, parts.prompts.length) : fitted;
  const slices = fitted.length === items.length ? fitted.slice(parts.prompts.length) : [];
  return assemble(parts, prompts, slices, true);
}

function assemble(parts: ReviewBriefParts, prompts: string[], slices: string[], shortened: boolean): string {
  const notice = `Some prompts or reports below are shortened; the whole text is in ${parts.record}.`;
  return [...parts.head, ...(shortened ? [notice] : []), "Owner prompts (verbatim):", ...prompts, "Slices:", ...slices, ...parts.tail]
    .join(JOIN);
}

/**
 * `items` within `room`, counting the separator each one adds: every item cut to one common cap,
 * the largest that fits with every cut's note; else one line for them all; else none.
 */
function fit(items: string[], room: number): string[] | undefined {
  const cut = (item: string, cap: number) => {
    if (item.length <= cap) return item;
    const note = `\n[${item.length - cap} characters omitted]`;
    return item.slice(0, Math.max(0, cap - note.length)) + note;
  };
  const size = (cap: number) => items.reduce((sum, item) => sum + cut(item, cap).length + JOIN.length, 0);
  if (size(0) > room) {
    const all = [`[all ${items.length} prompts and reports omitted]`];
    return all[0]!.length + JOIN.length <= room ? all : undefined;
  }
  let low = 0;
  let high = Math.max(0, ...items.map(item => item.length));
  while (low < high) {
    const mid = Math.ceil((low + high) / 2);
    if (size(mid) <= room) low = mid; else high = mid - 1;
  }
  return items.map(item => cut(item, low));
}
