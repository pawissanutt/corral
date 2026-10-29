// Exercise the production compiled startup, then derive auth without network or real logins.
import fixtureAuth from "./auth/index.ts";
import { createModelCatalog } from "../../src/host/catalog.ts";
import { PROVIDER_IDS } from "../../src/auth/providers.ts";

process.argv = [process.execPath, process.execPath, "--version"];
await import("../../src/host/compiled.ts");
globalThis.fetch = async () => { throw new Error("compiled OAuth smoke test must stay offline"); };
const codex = fixtureAuth.credentials({
  provider: "openai-codex", apiEndpoint: "", refreshEndpoint: "",
});
const models = await createModelCatalog({
  async read(provider, options) {
    return provider === "anthropic" ? {
      type: "oauth", access: "sk-ant-oat-synthetic", refresh: "r", expires: 4_000_000_000_000,
    } : codex.read(provider, options);
  },
  async list() { return PROVIDER_IDS.map((providerId) => ({ providerId, type: "oauth" as const })); },
  async modify() { throw new Error("smoke test credentials are read-only"); },
  async delete() { throw new Error("smoke test credentials are read-only"); },
}, { providers: PROVIDER_IDS, modelsPath: null });
for (const provider of PROVIDER_IDS) {
  const auth = await models.getAuth(provider);
  if (!auth?.auth.apiKey) throw new Error(`OAuth did not derive auth for ${provider}`);
  process.stdout.write(`${JSON.stringify({ provider, derived: true })}\n`);
}
