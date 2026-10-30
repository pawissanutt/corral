import { compareFile } from "./compare.ts";
import { markdown } from "./report.ts";

const args = process.argv.slice(2);
if (args.length < 1 || args.length > 2 || (args[1] !== undefined && args[1] !== "--markdown")) {
  console.error("Usage: bun tools/pi-compare/src/main.ts <manifest.json> [--markdown]");
  process.exitCode = 1;
} else {
  const path = args[0];
  if (path === undefined) throw new TypeError("missing manifest");
  try {
    const report = await compareFile(path);
    console.log(args[1] === "--markdown" ? markdown(report) : JSON.stringify(report, null, 2));
    process.exitCode = report.outcome === "pass" ? 0 : 2;
  } catch (error) {
    if (!(error instanceof Error)) throw error;
    console.error(error.message);
    process.exitCode = 1;
  }
}
