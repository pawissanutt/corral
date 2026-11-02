import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { secretLeaks, sha256 } from "./support/workspace.ts";

test("secret scan tolerates a listed file vanishing without hiding secrets or other I/O errors", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-secret-scan-"));
  try {
    const vanished = join(root, "vanished"), surviving = join(root, "surviving");
    const plain = "fixture-private-value", token = "b".repeat(64);
    const secrets = { plain: [plain], digests: [sha256(token)] };
    writeFileSync(vanished, "transient holder");
    writeFileSync(surviving, `${plain} ${token}`);
    let removed = false;
    const leaks = secretLeaks([root], { extra: plain }, secrets, path => {
      if (path === vanished) { unlinkSync(path); removed = true; }
      return readFileSync(path); // Real ENOENT, deterministically after enumeration.
    });
    expect(removed).toBeTrue();
    expect(leaks.sort()).toEqual([`${surviving}: plain secret`, `${surviving}: preimage of a stored digest`, "extra: plain secret"].sort());
    const denied = Object.assign(new Error("fixture denied"), { code: "EACCES" });
    expect(() => secretLeaks([root], {}, secrets, () => { throw denied; })).toThrow("fixture denied");
  } finally { rmSync(root, { recursive: true, force: true }); }
});
