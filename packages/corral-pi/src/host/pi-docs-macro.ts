// Evaluated by Bun at build time so installed hosts carry the documentation matching the SDK.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export function piDocs(): Record<string, string> {
  const root = join(dirname(fileURLToPath(import.meta.url)), "../../node_modules/@earendil-works/pi-coding-agent");
  const files: Record<string, string> = {};
  function collect(relative: string): void {
    for (const entry of readdirSync(join(root, relative), { withFileTypes: true })) {
      const path = join(relative, entry.name);
      if (entry.isDirectory()) collect(path);
      else if (entry.isFile()) files[path] = readFileSync(join(root, path)).toString("base64");
    }
  }
  files["README.md"] = readFileSync(join(root, "README.md")).toString("base64");
  collect("docs");
  collect("examples");
  return files;
}
