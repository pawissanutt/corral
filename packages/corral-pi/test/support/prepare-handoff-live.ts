// Private assets for the real-service /handoff PTY scenario: the production assistant role and
// host, with only the offline fixture auth/delivery modules and an isolated Pi state directory.
import { appendFileSync, cpSync } from "node:fs";
import { join } from "node:path";
import { buildFixtureInstall, PACKAGE_ROOT } from "./fixture-install.ts";

const work = process.argv[2];
if (!work) throw new Error("private fixture directory required");
const assets = buildFixtureInstall({ parent: work, omit: ["role"], isolatePiState: true });
cpSync(join(PACKAGE_ROOT, "assets"), join(assets.dir, "assets"), { recursive: true });
// A prompt naming STREAM_HOLD_<n> streams its first part, then waits until the driver creates
// release-stream-<n>: the driver observes overlays and drafts while a turn genuinely streams.
const release = JSON.stringify(join(work, "release-stream-"));
appendFileSync(join(assets.dir, "src/auth/index.ts"), `
import { existsSync as releaseExists } from "node:fs";
import { requestBody as streamBody } from "./fake-transport.ts";
// The host adds context after the owner's text and resends history, so arm on the highest new n.
const streamBase = fixtureTransport();
let streamArmed = 0;
(globalThis as unknown as Record<symbol, unknown>)[Symbol.for("corral-pi.fixture.transport")] =
  Object.assign(Object.create(streamBase), { send: ((input, init) => {
    const n = Math.max(0, ...[...JSON.stringify(streamBody(init)).matchAll(/STREAM_HOLD_(\\d+)/g)].map(m => Number(m[1])));
    if (n > streamArmed) {
      streamArmed = n;
      streamBase.script([{ chunks: ["Streaming first part. ", "Streaming done."],
        afterFirstChunk: async () => { while (!releaseExists(${release} + n)) await new Promise(r => setTimeout(r, 100)); } }]);
    }
    return streamBase.send(input, init);
  }) as typeof streamBase.send });
`);
process.stdout.write(JSON.stringify({ assets: assets.dir }));
