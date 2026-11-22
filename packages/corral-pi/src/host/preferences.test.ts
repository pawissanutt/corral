import { expect, test } from "bun:test";
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DEFAULT_PERSONAL_PREFERENCES, readPersonalPreferences, savePersonalPreference } from "./preferences.ts";

test("personal defaults survive reopening without dropping unrelated fields", () => {
  const home = mkdtempSync(join(tmpdir(), "pi-preferences-"));
  const env = { HOME: home };
  try {
    expect(readPersonalPreferences(env)).toEqual(DEFAULT_PERSONAL_PREFERENCES);
    savePersonalPreference("appearance", "light", env);
    savePersonalPreference("density", "compact", env);
    savePersonalPreference("startupModel", "openai-codex/example", env);
    savePersonalPreference("startupModel", null, env);
    const external = join(home, "external.json");
    writeFileSync(external, '"on"');
    symlinkSync(external, join(home, ".local/share/corral/corral-pi/preferences/bell.json"));
    expect(readPersonalPreferences(env)).toEqual({ ...DEFAULT_PERSONAL_PREFERENCES, appearance: "light", density: "compact" });
  } finally { rmSync(home, { recursive: true, force: true }); }
});
