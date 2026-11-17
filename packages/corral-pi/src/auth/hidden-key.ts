import type { Readable, Writable } from "node:stream";
import { CredentialsUnavailable } from "./paths.ts";

export interface KeyTerminal {
  input: Readable & { isTTY?: boolean; isRaw?: boolean; setRawMode?(raw: boolean): unknown };
  output: Writable & { isTTY?: boolean };
}

/** Read only from an owner terminal, with echo disabled and no key in argv or history. */
export function readHiddenKey(
  { input, output }: KeyTerminal = { input: process.stdin, output: process.stderr },
): Promise<string> {
  if (!input.isTTY || !output.isTTY || !input.setRawMode) {
    return Promise.reject(new CredentialsUnavailable("API key entry requires a terminal; use --key-file for scripts"));
  }
  return new Promise((resolve, reject) => {
    const wasRaw = input.isRaw ?? false;
    let key = "";
    const finish = (error?: Error) => {
      input.off("data", onData);
      input.off("end", onEnd);
      input.off("error", onError);
      input.pause();
      input.setRawMode?.(wasRaw);
      output.write("\n");
      const value = key;
      key = "";
      if (error) reject(error);
      else resolve(value);
    };
    const onEnd = () => finish(new CredentialsUnavailable("API key entry cancelled; nothing stored"));
    const onError = () => finish(new CredentialsUnavailable("API key terminal read failed; nothing stored"));
    const onData = (chunk: Buffer | string) => {
      for (const char of chunk.toString()) {
        if (char === "\x03" || char === "\x04" || char === "\x1b") return onEnd();
        if (char === "\r" || char === "\n") return finish();
        if (char === "\x7f" || char === "\b") key = key.slice(0, -1);
        else if (char === "\x15") key = "";
        else {
          if (char < " " || char > "~" || key.length >= 4096) {
            return finish(new CredentialsUnavailable("invalid API key input; nothing stored"));
          }
          key += char;
        }
      }
    };
    input.setRawMode?.(true);
    input.on("data", onData);
    input.once("end", onEnd);
    input.once("error", onError);
    output.write("API key (hidden; Enter to save, Ctrl-C to cancel): ");
    input.resume();
  });
}
