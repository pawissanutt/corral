// Bash allow/ask/deny rules (T-572). Rust resolves the rules once per generation; the host
// decides every bash call, top-level or nested, before it runs. Deny wins over ask and ask
// over allow. An ask runs only after the owner answers a corral_ask permission question bound
// to that exact command, and the rerun is decided again from the start.
import { createHash } from "node:crypto";

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

export const PERMISSION_REF = "bash-permission:";
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
export function pieces(command: string, depth = 0): string[] {
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

export function commandDigest(command: string): string {
  return createHash("sha256").update(command).digest("hex");
}

const ANSWER = /^(?:Corrected answer|Answer) to (\S+) \(revision \d+\) by (\w+):/;
const OPTION = /\nChosen option: ([^\n]*)$/;

/**
 * One-shot owner grants for `ask` commands. A grant exists only when this host refused the
 * exact command, a committed corral_ask of kind permission quoted it and named its digest, and
 * the inbox delivered the owner's answer to that question choosing `allow`. The PM cannot
 * answer permission questions; a correction to any other option revokes an unused grant.
 */
export class BashPermissions {
  private readonly requested = new Map<string, string>();
  private readonly questions = new Map<string, string>();
  private readonly granted = new Set<string>();

  request(command: string, decision: BashDecision): string {
    const digest = commandDigest(command);
    this.requested.set(digest, command);
    return `bash rule ${JSON.stringify(decision.rule ?? `default ${decision.action}`)} requires owner permission for `
      + `${JSON.stringify(decision.segment ?? command)}. Nothing ran. To ask, call corral_ask with kind "permission", `
      + `options ["allow", "deny"], context_refs ["${PERMISSION_REF}${digest}"] and a text that quotes the exact `
      + "command. When the owner's answer arrives choosing allow, run the identical command once more: each answer "
      + "permits one run.";
  }

  observeAsk(input: Record<string, unknown>, receipt: unknown): boolean {
    const result = receipt as { state?: unknown; result?: { key?: unknown } } | undefined;
    const key = result?.state === "committed" ? result.result?.key : undefined;
    if (input.kind !== "permission" || typeof key !== "string" || typeof input.text !== "string") return false;
    const refs = Array.isArray(input.context_refs) ? input.context_refs : [];
    for (const ref of refs) {
      if (typeof ref !== "string" || !ref.startsWith(PERMISSION_REF)) continue;
      const digest = ref.slice(PERMISSION_REF.length);
      const command = this.requested.get(digest);
      if (command !== undefined && input.text.includes(command)) {
        this.questions.set(key, digest);
        return true;
      }
    }
    return false;
  }

  observeAnswer(text: string): "granted" | "revoked" | undefined {
    const header = ANSWER.exec(text);
    const digest = header ? this.questions.get(header[1] ?? "") : undefined;
    if (!header || digest === undefined) return undefined;
    const option = OPTION.exec(text)?.[1]?.trim().toLowerCase();
    if (header[2] !== "Pm" && option === "allow") {
      this.granted.add(digest);
      return "granted";
    }
    this.granted.delete(digest);
    return "revoked";
  }

  consume(command: string): boolean {
    return this.granted.delete(commandDigest(command));
  }
}
