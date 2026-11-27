import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { explicitSkillPaths, readSkillPolicy } from "./skill-policy.ts";

test("skill policy rejects malformed values and broad home roots without quoting the settings", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-skill-policy-"));
  const home = join(root, "home");
  const file = join(root, ".corral-next/pi.toml");
  try {
    expect(readSkillPolicy(root, home)).toEqual({ disabled: new Set(), userDirs: [] });
    mkdirSync(join(root, ".corral-next"));
    for (const [source, error] of [
      ['skills = "secret"', "skills must be a table"],
      ['[skills]\ndisabled = [false]', "skills.disabled must be an array"],
      ['[skills]\nuser_dirs = [true]', "skills.user_dirs must be an array"],
      ['[skills]\nuser_dirs = ["relative"]', "must be absolute or start with ~/"],
      ['[skills]\nuser_dirs = ["~/"]', "cannot include home"],
      ['[skills]\nuser_dirs = ["/"]', "cannot include home"],
    ] as const) {
      writeFileSync(file, source);
      expect(() => readSkillPolicy(root, home)).toThrow(error);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("explicit user discovery skips symlinks and stops descending at a skill root", () => {
  const root = mkdtempSync(join(tmpdir(), "corral-pi-skill-paths-"));
  const inside = join(root, "inside");
  const external = join(root, "external");
  try {
    mkdirSync(join(inside, "group", "nested"), { recursive: true });
    mkdirSync(external);
    for (const path of [join(inside, "direct.md"), join(inside, "group/SKILL.md"),
      join(inside, "group/nested/SKILL.md"), join(external, "SKILL.md")]) writeFileSync(path, "");
    symlinkSync(external, join(inside, "external-link"));
    symlinkSync(join(external, "SKILL.md"), join(inside, "external.md"));
    expect(explicitSkillPaths([inside])).toEqual([join(inside, "direct.md"), join(inside, "group/SKILL.md")]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
