// Offline diagnostic for the embedded QuickJS worker, independent of model credentials.
import { readdirSync } from "node:fs";
import { CodemodeSandbox, loadQuickJSWasm } from "@earendil-works/pi-codemode";
import { getCodemodeWorkerSpecifier, getQuickJSWasmPath } from "../../node_modules/@earendil-works/pi-coding-agent/dist/config.js";

export async function checkCodemode(): Promise<void> {
  const sandbox = new CodemodeSandbox({
    wasm: loadQuickJSWasm(getQuickJSWasmPath()),
    workerUrl: getCodemodeWorkerSpecifier(),
    tools: [{ name: "ls", description: "List the current directory", execute: async () => readdirSync(process.cwd()) }],
  });
  try {
    const result = await sandbox.execute('return await tools.ls({});');
    if (!result.ok || !Array.isArray(result.value)) throw new Error(`codemode check failed: ${JSON.stringify(result)}`);
    process.stdout.write(`codemode nested ls: ${JSON.stringify(result.value)}\n`);
  } finally {
    await sandbox.close();
  }
}
