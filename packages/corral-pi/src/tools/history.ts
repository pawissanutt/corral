import { createReadStream, readFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";

export type HistoryRequest =
  | { kind: "tasks"; query: string }
  | { kind: "task"; task: string }
  | TranscriptRequest;

interface TranscriptRequest {
  kind: "transcript";
  task: string;
  run: string;
  first: number;
}

/** Read the requested page from the same transcript the socket advertises. */
export async function transcriptPage(root: string, { task, run, first }: TranscriptRequest): Promise<string> {
  if (!/^T-\d+$/.test(task) || !/^R\d+$/.test(run) || !Number.isSafeInteger(first) || first < 1) {
    throw new Error("Invalid transcript page");
  }
  const record = JSON.parse(readFileSync(join(root, ".corral-next", "runs", task, run, "session.json"), "utf8"));
  if (typeof record?.sessionFile !== "string") throw new Error("The run has no Pi transcript");
  const stream = createReadStream(record.sessionFile);
  const lines = createInterface({ input: stream, crlfDelay: Infinity });
  const page: string[] = [];
  let line = 0, bytes = 0;
  try {
    for await (const text of lines) {
      if (++line < first) continue;
      bytes += Buffer.byteLength(text) + 1;
      if (bytes > 256 * 1024) throw new Error("The transcript page exceeds 256 KiB");
      page.push(text);
      if (page.length === 200) break;
    }
  } finally {
    lines.close();
    stream.destroy();
  }
  return page.join("\n");
}
