// The assistant's fixed shell refusals (T-544): no input into a herdr pane and no direct write
// under `.corral/`. They stop an honest model's accidents, not an adversary. Presets, capability
// settings and configured bash rules can neither disable nor override them. The board changes
// only through corral commands, which audit and attribute each change.
//
// A small quote-aware lexer splits the line into the simple commands it executes, including
// `$(...)` and backtick bodies (also inside double quotes), `sh -c` strings and `eval`. Quoted
// text is data: a comment that quotes a herdr command, or a printed example of a redirection, is
// never refused. Only executed commands, unquoted redirections and write destinations count.

interface Simple {
  readonly words: string[];
  /** Targets of output redirections (`>`, `>>`, `>|`, `&>`). */
  readonly writes: string[];
}

const NESTING_MAX = 4;
const WRAPPERS = new Set(["sudo", "env", "command", "nice", "nohup", "time", "exec"]);
const SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh"]);
/** Every operand is written (or removed): the store must not appear among them. */
const ALL_OPERANDS = new Set([
  "rm", "rmdir", "unlink", "shred", "touch", "mkdir", "truncate", "chmod", "chown", "chgrp", "tee", "mv",
]);
/** Only the destination (last operand, or `-t DIR`) is written; sources are read. */
const DESTINATION = new Set(["cp", "install", "rsync", "ln"]);

/** The index just past the `)` closing the `$(` whose body starts at `start`. */
function closing(line: string, start: number): number {
  let depth = 1;
  for (let i = start; i < line.length; i++) {
    if (line[i] === "(") depth += 1;
    if (line[i] === ")" && --depth === 0) return i;
  }
  return line.length;
}

function parse(line: string, depth = 0): Simple[] {
  const out: Simple[] = [];
  let words: string[] = [];
  let writes: string[] = [];
  let word = "";
  let inWord = false;
  let quoted = false;
  let redirect: "write" | "skip" | undefined;
  const nested = (body: string) => {
    if (depth < NESTING_MAX) out.push(...parse(body, depth + 1));
  };
  const endWord = () => {
    if (!inWord) return;
    if (redirect === "write") writes.push(word);
    else if (redirect === undefined) words.push(word);
    redirect = undefined;
    word = "";
    inWord = false;
    quoted = false;
  };
  const endCommand = () => {
    endWord();
    if (words.length > 0 || writes.length > 0) out.push({ words, writes });
    words = [];
    writes = [];
  };
  // Appends a substitution's placeholder and parses its body as commands of their own.
  const substitute = (i: number): number => {
    if (line[i] === "`") {
      const end = line.indexOf("`", i + 1);
      const stop = end < 0 ? line.length : end;
      nested(line.slice(i + 1, stop));
      return stop;
    }
    const end = closing(line, i + 2);
    nested(line.slice(i + 2, end));
    return end;
  };
  for (let i = 0; i < line.length; i++) {
    const c = line[i] ?? "";
    if (c === "\\") {
      word += line[i + 1] ?? "";
      inWord = true;
      quoted = true;
      i += 1;
    } else if (c === "'") {
      const end = line.indexOf("'", i + 1);
      const stop = end < 0 ? line.length : end;
      word += line.slice(i + 1, stop);
      inWord = true;
      quoted = true;
      i = stop;
    } else if (c === "\"") {
      inWord = true;
      quoted = true;
      for (i += 1; i < line.length && line[i] !== "\""; i++) {
        if (line[i] === "\\") word += line[++i] ?? "";
        else if (line[i] === "`" || (line[i] === "$" && line[i + 1] === "(")) i = substitute(i);
        else word += line[i];
      }
    } else if (c === "`" || (c === "$" && line[i + 1] === "(")) {
      i = substitute(i);
      inWord = true;
    } else if (c === ">" || c === "<" || (c === "&" && line[i + 1] === ">")) {
      // A bare descriptor number before the operator (`2>`) is not a word of the command.
      if (inWord && !quoted && /^\d+$/.test(word)) {
        word = "";
        inWord = false;
      }
      endWord();
      if (c === "&") i += 1;
      const output = line[i] === ">";
      while (line[i + 1] === ">" || line[i + 1] === "<" || line[i + 1] === "|") i += 1;
      const duplicate = line[i + 1] === "&";
      if (duplicate) i += 1;
      redirect = output && !duplicate ? "write" : "skip";
    } else if (c === ";" || c === "|" || c === "&" || c === "\n" || c === "(" || c === ")") {
      endCommand();
    } else if (c === " " || c === "\t") {
      endWord();
    } else {
      word += c;
      inWord = true;
    }
  }
  endCommand();
  for (const simple of [...out]) {
    const { name, args } = invocation(simple.words);
    const flag = args.findIndex((arg) => /^-[A-Za-z]*c[A-Za-z]*$/.test(arg));
    if (SHELLS.has(name) && flag >= 0) {
      const body = args.slice(flag + 1).find((arg) => !arg.startsWith("-"));
      if (body !== undefined) nested(body);
    }
    if (name === "eval") nested(args.join(" "));
  }
  return out;
}

/** The executed program (wrappers and assignments skipped) and its arguments. */
function invocation(words: readonly string[]): { name: string; args: string[] } {
  let index = 0;
  while (index < words.length) {
    const word = words[index] ?? "";
    const wrapped = index > 0 && WRAPPERS.has(words[index - 1] ?? "") && word.startsWith("-");
    if (!WRAPPERS.has(word) && !/^[A-Za-z_][A-Za-z0-9_]*=/.test(word) && !wrapped) break;
    index += 1;
  }
  return { name: (words[index] ?? "").split("/").pop() ?? "", args: words.slice(index + 1) };
}

/** A path with a `.corral` component, the workspace store's directory. */
function inStore(path: string): boolean {
  return /(?:^|\/)\.corral(?:\/|$)/.test(path);
}

function paneInput(name: string, args: readonly string[]): boolean {
  if (name !== "herdr") return false;
  return args.some((arg, i) => (arg === "pane" && ["send-text", "send-keys", "run"].includes(args[i + 1] ?? ""))
    || (arg === "agent" && args[i + 1] === "prompt"));
}

function writesStore(simple: Simple, name: string, args: readonly string[]): boolean {
  if (simple.writes.some(inStore)) return true;
  const operands = args.filter((arg) => !arg.startsWith("-"));
  if (ALL_OPERANDS.has(name)) return operands.some(inStore);
  if (DESTINATION.has(name)) {
    const target = args.findIndex((arg) => arg === "-t" || arg === "--target-directory");
    const named = args.find((arg) => arg.startsWith("--target-directory="))?.split("=")[1];
    const destination = named ?? (target >= 0 ? args[target + 1] : operands.at(-1));
    return destination !== undefined && inStore(destination);
  }
  if (name === "dd") return args.some((arg) => arg.startsWith("of=") && inStore(arg.slice(3)));
  if (name === "sed" || name === "perl") {
    return args.some((arg) => /^-i|^--in-place/.test(arg)) && operands.some(inStore);
  }
  return false;
}

/** Why the assistant may not run `command`, or undefined when it may. Nested shells count. */
export function assistantShellRefusal(command: string): string | undefined {
  for (const simple of parse(command)) {
    const { name, args } = invocation(simple.words);
    const shown = JSON.stringify(simple.words.join(" "));
    if (paneInput(name, args)) {
      return `assistant rule: no input into a herdr pane (${shown}); nothing ran. `
        + "An assistant never types into another pane: tell the owner, or hand the request to the PM.";
    }
    if (writesStore(simple, name, args)) {
      return `assistant rule: no direct write under .corral/ (${shown}); nothing ran. `
        + "Change the board through corral commands, which record who changed what.";
    }
  }
  return undefined;
}
