import { expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { authDir, corralPiDirectory } from "./paths.ts";

test("legacy installed credentials and skills are read in place and never moved", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-migration-"));
  try {
    const data = join(root, "data");
    const oldData = join(data, "corral/pi-native");
    mkdirSync(join(oldData, "auth"), { recursive: true });
    writeFileSync(join(oldData, "auth/auth.json"), "credential");
    const auth = authDir({ XDG_DATA_HOME: data });
    expect(auth).toBe(join(oldData, "auth"));
    expect(readFileSync(join(auth, "auth.json"), "utf8")).toBe("credential");
    expect(existsSync(oldData)).toBe(true);
    expect(existsSync(join(data, "corral/corral-pi"))).toBe(false);

    const parent = join(root, ".corral/skills");
    const oldSkills = join(parent, "pi-native");
    const skills = join(parent, "corral-pi");
    mkdirSync(oldSkills, { recursive: true });
    writeFileSync(join(oldSkills, "SKILL.md"), "guidance");
    expect(corralPiDirectory(skills)).toBe(oldSkills);
    expect(readFileSync(join(oldSkills, "SKILL.md"), "utf8")).toBe("guidance");
    expect(existsSync(skills)).toBe(false);
    mkdirSync(skills);
    writeFileSync(join(skills, "other"), "new install");
    expect(corralPiDirectory(skills)).toBe(skills);
    expect(existsSync(join(oldSkills, "SKILL.md"))).toBe(true);
    expect(existsSync(join(skills, "SKILL.md"))).toBe(false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
