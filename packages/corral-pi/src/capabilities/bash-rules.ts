// Session-configured allow/ask/deny rules; deny wins over ask and ask wins over allow.

import { z } from "zod";

const Pattern = z.string().min(1).max(1024).refine((value) => value.trim().length > 0 && !value.includes("\0"));
const Patterns = z.array(Pattern).max(128).readonly();

export const BashRuleSettingsSchema = z.object({
  allow: Patterns,
  ask: Patterns,
  deny: Patterns,
  default: z.enum(["allow", "ask", "deny"]),
}).strict().readonly();

export type BashRuleSettings = z.infer<typeof BashRuleSettingsSchema>;
export type BashAction = "allow" | "ask" | "deny";

export interface BashDecision {
  readonly action: BashAction;
  /** The matching pattern; absent when the default decided. */
  readonly rule?: string;
  /** The part of the command line that decided. */
  readonly segment?: string;
}

const NESTING_MAX = 4;
/**
 * A shell invocation: its options (`-o name`, `--long`, short clusters such as `-lc`), then the
 * first operand. The operand is a command string whenever any short cluster contains `c`.
 */
const SHELL = /(?:^|[\s;&|(`])(?:\S*\/)?(?:ba|z|da|k)?sh\s+((?:[+-]o\s+\S+\s+|--[a-z-]+\s+|[+-][A-Za-z]+\s+)*)(?:'([^']*)'|"([^"]*)"|(\S+))/g;
const COMMAND_OPTION = /(?:^|\s)-[A-Za-z]*c[A-Za-z]*(?=\s)/;

function normalize(text: string): string {
  return text.replace(/\s+/g, " ").replace(/^[({\s]+|[)}\s]+$/g, "").trim();
}

/** `*` matches any text, `?` one character; the pattern must match the whole segment. */
function glob(pattern: string): RegExp {
  const source = normalize(pattern).split("").map((char) => {
    if (char === "*") return "[\\s\\S]*";
    if (char === "?") return "[\\s\\S]";
    return char.replace(/[.+^${}()|[\]\\]/g, "\\$&");
  }).join("");
  return new RegExp(`^${source}$`);
}

/**
 * The simple commands a line can run: operator-separated pieces plus the bodies of command
 * substitutions and `sh -c`/`bash -c` arguments, recursively. Quotes are not parsed, so a
 * quoted operator splits too: matching errs toward more pieces, never fewer.
 */
function pieces(command: string, depth = 0): string[] {
  const found = new Set<string>();
  for (const part of command.split(/\|\||&&|[;|&\n]/)) {
    const piece = normalize(part);
    if (piece) found.add(piece);
  }
  if (depth < NESTING_MAX) {
    const nested = [
      ...[...command.matchAll(/\$\(([^()]*)\)|`([^`]*)`/g)].map((match) => match[1] ?? match[2] ?? ""),
      ...[...command.matchAll(SHELL)].filter((match) => COMMAND_OPTION.test(` ${match[1] ?? ""}`))
        .map((match) => match[2] ?? match[3] ?? match[4] ?? ""),
    ];
    for (const body of nested) for (const piece of pieces(body, depth + 1)) found.add(piece);
  }
  return [...found];
}

export function decideBash(command: string, rules: BashRuleSettings): BashDecision {
  const parts = pieces(command);
  const segments = [...new Set([normalize(command), ...parts])].filter((segment) => segment.length > 0);
  const first = (patterns: readonly string[]): BashDecision | undefined => {
    for (const pattern of patterns) {
      const matcher = glob(pattern);
      const segment = segments.find((candidate) => matcher.test(candidate));
      if (segment !== undefined) return { action: "allow", rule: pattern, segment };
    }
    return undefined;
  };
  const deny = first(rules.deny);
  if (deny) return { ...deny, action: "deny" };
  const ask = first(rules.ask);
  if (ask) return { ...ask, action: "ask" };
  if (rules.default === "allow") return { action: "allow" };
  const allowed = rules.allow.map(glob);
  const unmatched = parts.find((piece) => !allowed.some((matcher) => matcher.test(piece)));
  if (parts.length > 0 && unmatched === undefined) return { action: "allow", rule: "allow list" };
  return { action: rules.default, ...(unmatched ? { segment: unmatched } : {}) };
}
