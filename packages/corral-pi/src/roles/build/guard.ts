import { relative, sep } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

import { assistantShellRefusal } from "../assistant/guard.ts";
import { real, resolveToolPath, within } from "../assistant/paths.ts";
import type { RoleContext } from "../types.ts";

// Cover ordinary Git invocations, including -C/-c and explicit repository paths. These
// fixed accident guards are deliberately conservative, not a shell security boundary.
const GIT = /(?:^|[\s/;&|()`'"])git\s+(?:(?:-C|-c|--git-dir|--work-tree|--namespace)\s+(?:"[^"]*"|'[^']*'|[^\s;&|]+)\s+|--(?:git-dir|work-tree|namespace)=[^\s;&|]+\s+|--(?:no-pager|paginate|bare)\s+)*/g;

// Only top-level commands without mutating modes belong here. In particular, search
// has --reindex, route has reviewer setters, and attention has park/attend subcommands.
const READ_ONLY_CORRAL: readonly string[] = [
  "show", "list", "log", "stats", "review", "pins", "build-identity",
  "help", "--help", "-h", "--version", "-V",
];
const CORRAL_REFUSAL = "build rule: model shell calls may only read corral state; nothing ran. "
  + "Use /board for the owner's confirmed board plan or /finish for the owner's exit choice.";
const SHELLS: readonly string[] = ["sh", "bash", "zsh", "dash", "ksh"];
const WRAPPERS: readonly string[] = ["env", "command", "exec", "sudo", "nice", "nohup", "time"];

/** Split executable commands, not quoted examples, and inspect substitutions recursively. */
function shellCommands(source: string): string[][] | undefined {
  const commands: string[][] = [];
  let index = 0;
  let complete = true;
  const scan = (stop?: ")" | "`", depth = 0): void => {
    if (depth >= 32) {
      complete = false;
      index = source.length;
      return;
    }
    let words: string[] = [];
    let word = "";
    let active = false;
    let quoted = false;
    let quote: "'" | '"' | undefined;
    let redirect = false;
    const endWord = () => {
      if (!active) return;
      if (!redirect) words.push(word);
      word = "";
      active = false;
      quoted = false;
      redirect = false;
    };
    const endCommand = () => {
      endWord();
      if (words.length) commands.push(words);
      words = [];
    };
    while (index < source.length) {
      const c = source[index]!;
      if (!quote && c === stop) {
        index += 1;
        endCommand();
        return;
      }
      if (quote === "'") {
        if (c === "'") quote = undefined;
        else word += c;
        index += 1;
        continue;
      }
      if (c === "\\" && (!quote || /[$`"\\\n]/.test(source[index + 1] ?? ""))) {
        const escaped = source[index + 1];
        if (escaped === undefined) complete = false;
        else if (escaped !== "\n") {
          word += escaped;
          active = true;
          quoted = true;
        }
        index += 2;
        continue;
      }
      if ((c === "$" && source[index + 1] === "(") || c === "`") {
        index += c === "$" ? 2 : 1;
        scan(c === "$" ? ")" : "`", depth + 1);
        word += "$(unknown)";
        active = true;
        continue;
      }
      if (quote === '"') {
        if (c === '"') quote = undefined;
        else word += c;
        index += 1;
        continue;
      }
      if (c === "'" || c === '"') {
        quote = c;
        active = true;
        quoted = true;
      } else if (c === "#" && !active) {
        while (index < source.length && source[index] !== "\n") index += 1;
        continue;
      } else if (c === "(") {
        endCommand();
        index += 1;
        scan(")", depth + 1);
        continue;
      } else if (c === ")") {
        complete = false;
        endCommand();
      } else if (c === "<" || c === ">") {
        if (active && !quoted && /^\d+$/.test(word)) {
          word = "";
          active = false;
        }
        endWord();
        while (source[index + 1] === c || source[index + 1] === "|" || source[index + 1] === "&") index += 1;
        redirect = true;
      } else if (c === ";" || c === "&" || c === "|" || c === "\n") {
        endCommand();
      } else if (/\s/.test(c)) {
        endWord();
      } else {
        word += c;
        active = true;
      }
      index += 1;
    }
    endCommand();
    if (quote || stop) complete = false;
  };
  scan();
  return complete ? commands : undefined;
}

/** Resolve executable names through shell assignments and ordinary execution prefixes. */
function shellInvocation(words: readonly string[]): { name: string; args: readonly string[] } {
  let index = 0;
  while (index < words.length) {
    const word = words[index]!;
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(word)) {
      index += 1;
      continue;
    }
    const name = word.split("/").pop() ?? "";
    if (["if", "then", "elif", "else", "while", "until", "do", "!", "{"].includes(word)) {
      index += 1;
      continue;
    }
    if (!WRAPPERS.includes(name)) return { name, args: words.slice(index + 1) };
    index += 1;
    while (words[index]?.startsWith("-")) {
      const option = words[index++]!;
      if (option === "--") break;
      if (name === "env" && (option === "-S" || option === "--split-string" || option.startsWith("--split-string="))) {
        const body = option.startsWith("--split-string=") ? option.slice("--split-string=".length) : words[index++] ?? "";
        return { name: "eval", args: [body, ...words.slice(index)] };
      }
      if ((name === "env" && ["-u", "--unset", "-C", "--chdir"].includes(option))
        || (name === "exec" && option === "-a")
        || (name === "sudo" && ["-u", "-g", "-h", "-p", "-C", "-D", "-R", "-T"].includes(option))
        || (name === "nice" && option === "-n")) index += 1;
    }
  }
  return { name: "", args: [] };
}

function corralShellRefusal(command: string): string | undefined {
  const pending = [{ source: command, depth: 0 }];
  while (pending.length) {
    const { source, depth } = pending.pop()!;
    if (depth >= 32) return CORRAL_REFUSAL;
    const commands = shellCommands(source);
    if (!commands) return CORRAL_REFUSAL;
    for (const words of commands) {
      const { name, args } = shellInvocation(words);
      if (name === "corral") {
        let index = 0;
        while (index < args.length) {
          if (args[index] === "--root") index += 2;
          else if (args[index]?.startsWith("--root=")) index += 1;
          else break;
        }
        if (!READ_ONLY_CORRAL.includes(args[index] ?? "")) return CORRAL_REFUSAL;
      } else if (SHELLS.includes(name)) {
        const flag = args.findIndex((arg) => /^-[A-Za-z]*c[A-Za-z]*$/.test(arg) || arg === "--command");
        if (flag >= 0 && args[flag + 1] !== undefined) pending.push({ source: args[flag + 1]!, depth: depth + 1 });
      } else if (name === "eval") {
        pending.push({ source: args.join(" "), depth: depth + 1 });
      }
    }
  }
  return undefined;
}

export function buildShellRefusal(command: string): string | undefined {
  const boundary = corralShellRefusal(command);
  if (boundary) return boundary;
  const fixed = assistantShellRefusal(command);
  if (fixed) return fixed;
  for (const match of command.matchAll(GIT)) {
    const args = command.slice(match.index! + match[0].length);
    if (/^worktree\s+remove\b/.test(args)) {
      return "build rule: do not remove a worktree from under running children; nothing ran. The owner controls session cleanup.";
    }
    // The command-only role hook has no session identity: refuse all branch deletion,
    // rather than risk deleting the session branch through another checkout.
    if (/^branch\b[^\n;&|]*\s(?:-[a-zA-Z]*[dD][a-zA-Z]*|--delete)(?=\s|$|['"])/.test(args)) {
      return "build rule: do not delete branches from under running children; nothing ran. The owner controls session cleanup.";
    }
  }
  return undefined;
}

export type BuildWriteClassification = { allowed: true } | { allowed: false; reason: string };
const PROTECTED: readonly string[] = [".git", ".corral", ".worktrees"];

/** Check both the tool's lexical destination and where its existing symlinks lead. */
export function classifyBuildWrite(path: string, worktree: string): BuildWriteClassification {
  const root = resolveToolPath(worktree, ".");
  const target = resolveToolPath(path, root);
  if (!within(target, root)) return { allowed: false, reason: "build rule: writes must stay inside the session worktree; nothing was written." };
  if (relative(root, target).split(sep).some((part) => PROTECTED.includes(part))) {
    return { allowed: false, reason: "build rule: writes into .git, .corral or .worktrees are refused; nothing was written." };
  }
  const canonicalRoot = real(root);
  const canonicalTarget = real(target);
  if (!canonicalRoot || !canonicalTarget) {
    return { allowed: false, reason: "build rule: the write destination could not be resolved safely; nothing was written." };
  }
  if (!within(canonicalTarget, canonicalRoot)) {
    return { allowed: false, reason: "build rule: the write destination resolves outside the session worktree; nothing was written." };
  }
  if (relative(canonicalRoot, canonicalTarget).split(sep).some((part) => PROTECTED.includes(part))) {
    return { allowed: false, reason: "build rule: the write destination resolves into .git, .corral or .worktrees; nothing was written." };
  }
  return { allowed: true };
}

export function installBuildGuard(pi: ExtensionAPI, context: RoleContext): void {
  pi.on("tool_call", async (event) => {
    const input = event.input as Record<string, unknown>;
    // Host controls also inspect bash/job_start. Apply the boundary here as well so
    // child sessions and other command-bearing tools cannot miss that host hook.
    if (typeof input.command === "string") {
      const refusal = buildShellRefusal(input.command);
      if (refusal) return { block: true, reason: refusal };
    }
    if (event.toolName !== "edit" && event.toolName !== "write") return undefined;
    if (!context.build) return { block: true, reason: "build rule: no build session is bound, so every edit and write is refused; nothing was written." };
    const path = input.path;
    if (typeof path !== "string") return { block: true, reason: "build rule: edit and write require a target path; nothing was written." };
    try {
      const result = classifyBuildWrite(resolveToolPath(path, context.cwd), context.build.session.worktree);
      return result.allowed ? undefined : { block: true, reason: result.reason };
    } catch {
      return { block: true, reason: "build rule: the write destination could not be resolved safely; nothing was written." };
    }
  });
}
