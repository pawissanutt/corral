// Build keeps its worktree and transcript across profile changes. The Rust launcher owns
// the restart, exact route validation and durable paid allowance; this host only requests it.
import { randomBytes } from "node:crypto";
import { renameSync, rmSync, writeFileSync } from "node:fs";
import { resolveConnection } from "../auth/connections.ts";
import { PROVIDERS, type ProviderId } from "../auth/providers.ts";
import type { AuthModule } from "../auth/types.ts";
import { selectBuildProfile } from "../build/profiles.ts";
import type { BuildContext, BuildProfileRoute } from "../build/types.ts";
import { ProfileReadiness } from "./profile-readiness.ts";
import type { ProfileChoice, ProfileController } from "./profile-controls.ts";

interface Options {
  readonly build: BuildContext;
  readonly auth: AuthModule;
  readonly blocked: () => string | undefined;
  readonly restart: () => void;
}

export class BuildProfileController implements ProfileController {
  private pending = false;
  private readonly readiness: ProfileReadiness;
  constructor(private readonly options: Options) { this.readiness = new ProfileReadiness(options.auth); }
  current(): string | undefined { return this.options.build.session.route?.name; }
  blocked(): string | undefined {
    return this.pending ? "A profile switch is already in progress" : this.options.blocked();
  }
  private async preflight(route: BuildProfileRoute): Promise<void> {
    const provider = route.provider as ProviderId;
    const connection = resolveConnection(provider, route.binding?.connection);
    if (route.effort === undefined) await this.readiness.check({ connection, model: route.model });
    else await this.readiness.check({ connection, model: route.model, effort: route.effort });
    if (PROVIDERS[provider].auth === "local") {
      throw new Error("Local inference is unavailable in Build sessions");
    }
  }
  async list(): Promise<readonly ProfileChoice[]> {
    const { build } = this.options;
    const rows: ProfileChoice[] = [];
    for (const route of build.profileCatalog ?? []) {
      let reason: string | undefined;
      try {
        await selectBuildProfile(build, route.name);
        await this.preflight(route);
      } catch (error) { reason = error instanceof Error ? error.message : String(error); }
      const provider = route.provider as ProviderId;
      const model = route.model.startsWith(`${provider}/`) ? route.model : `${provider}/${route.model}`;
      rows.push({ name: route.name, label: route.label, model,
        connection: resolveConnection(provider, route.binding?.connection), available: reason === undefined,
        ...(reason ? { reason } : {}) });
    }
    return rows;
  }
  async select(name: string): Promise<void> {
    const blocked = this.blocked();
    if (blocked) throw new Error(`profile_switch_refused: ${blocked}`);
    const { build } = this.options;
    const route = await selectBuildProfile(build, name);
    await this.preflight(route);
    if (this.current() === name) return;
    if (!build.profileRequestPath) throw new Error("profile_switch_unavailable: restart this build with updated corral assets");
    const again = this.blocked();
    if (again) throw new Error(`profile_switch_refused: ${again}`);
    const path = build.profileRequestPath;
    const temporary = `${path}.${randomBytes(12).toString("hex")}.tmp`;
    this.pending = true;
    try {
      writeFileSync(temporary, JSON.stringify({ version: 1, profile: name, session: build.session.id, generation: 1 }),
        { mode: 0o600, flag: "wx" });
      renameSync(temporary, path);
      this.options.restart();
    } catch (error) {
      this.pending = false;
      rmSync(path, { force: true });
      throw error;
    } finally { rmSync(temporary, { force: true }); }
  }
}
