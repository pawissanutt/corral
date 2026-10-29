# Pricing

`[pricing."<exact model ID>"]` stores editable USD rates per million tokens.
`input` and `output` are required; `cache_read` and `cache_write` are optional
and independently fall back to `input` when absent. Explicit zero rates remain
zero. `aliases` is an optional array of exact alternate IDs.

```toml
[pricing."claude-sonnet-4-6"]
input = 3.0
output = 15.0
cache_read = 0.3
cache_write = 3.75
aliases = []
```

The exported `corral_core::Price` has public `input: f64`, `output: f64`,
`cache_read: Option<f64>`, `cache_write: Option<f64>`, and
`aliases: Vec<String>` fields. Its `cache_read()` and `cache_write()` methods
return the effective fallback rates. `Config.pricing` is a
`BTreeMap<String, Price>`. `Config::price(&str) -> Option<&Price>` tries the
config exact key first, then config aliases, then the built-in catalog's exact
key, and returns `None` for unknown models. Config always wins, including an
explicit alias or omitted cache rates; catalog fields are not merged into a
configured price.
It does not normalize case, strip providers, match prefixes, or guess model
families. `Config` retains `PartialEq` but cannot implement `Eq` with floats.

`Config::validate` rejects negative or non-finite rates, aliases owned by
multiple entries, and aliases that collide with another exact model key.
Validated config saves use this same validation. Loading older configs without
`pricing` yields an empty editable table. The dated catalog is a lower lookup
layer available immediately on load in every workspace, without modifying files.
Each priced response's `UsageEntry.price_source` is `config` or
`catalog@2026-10-15`; unpriced entries have no source. Task detail cost summaries
show the sources used. Stats refresh reprices stored completed-run evidence with
the effective rates and updates these labels.

## Coverage checks

```
corral pricing check
corral pricing check --json
```

The check refreshes available usage evidence, includes all retained runs and
reviewer runs, and reports the catalog date, models without prices and their
affected tokens, cache tokens priced at the input rate because the corresponding
cache rate is absent, and config overrides whose effective rates differ from the
catalog. Missing model IDs appear as JSON `null`, distinct from a literal model
named `unknown`. Zero-token unpriced entries are listed with zero affected tokens.
Explicit zero cache rates are not fallbacks. Overrides are reported even when
the model has no observed usage, including aliases that shadow catalog IDs.
Matching effective rates are not reported as shadows.

JSON fields are `catalog_date`, `unpriced_tokens`, `unpriced_models`
(`model`, `entries`, `tokens`), `cache_rate_fallbacks` (`model`, `price_source`,
`entries`, `cache_read_tokens`, `cache_write_tokens`), and `shadows`
(`model`, `config_model`, `config_price`, `catalog_price`).
Coverage gaps do not make this diagnostic command exit nonzero.

`corral doctor` adds a soft `pricing_coverage` warning when stored usage has
unpriced tokens in runs started in the last 30 days, inclusively. Reviewer runs
use their own start time. This read-only check reprices stored evidence in memory
without scanning transcripts; use `pricing check` or `stats` to refresh evidence.
Absent usage is not counted as zero-token evidence or proof of full coverage.

## Built-in catalog, starter table and provenance

Only creation of a **new** `.corral/corral.toml` by `Store::init` installs this
small frozen catalog as an editable starter table, preceded by:

```toml
# prices as of 2026-10-15, verify
```

Existing files are never reseeded or overwritten, including files with no
pricing or locally edited rates. `Config::default_for` has empty pricing.
The same shipped table supplies the lower catalog layer to existing workspaces.
It needs no runtime dependency on an agent installation.

These values were extracted from the actual installed catalogs on
October 15, 2026, not inferred from model names:

- omp: `/home/pawis/.bun/install/global/node_modules/@oh-my-pi/pi-catalog/src/models.json`,
  provider keys `anthropic`, `openai`, and `openai-codex`.
  The installed `/home/pawis/.omp/agent/models.db` also contains materialized
  provider records in `model_cache`.
- OmO: `/home/pawis/.omo/agent/runtime/0b0c6ca88f3a3632-f176205d0bda/node_modules/@earendil-works/pi-ai/dist/models.generated.js`.
  This registry imports `providers/{anthropic,openai,chatgpt-subscription}.models.js`,
  which read `providers/data/{anthropic,openai,chatgpt-subscription}.json`.
  Rates are in each model's `cost.{input,output,cacheRead,cacheWrite}`.

| Exact model ID | Input | Output | Cache read | Cache write | Verified source |
|---|---:|---:|---:|---:|---|
| claude-haiku-5-5 | 0.1 | 0.5 | 0.01 | 0.125 | OmO, anthropic |
| claude-opus-4-6 | 5 | 25 | 0.5 | 6.25 | Both, anthropic |
| claude-opus-4-7 | 5 | 25 | 0.5 | 6.25 | Both, anthropic |
| claude-opus-4-8 | 5 | 25 | 0.5 | 6.25 | Both, anthropic |
| claude-opus-5-5 | 4 | 20 | 0.2 | 5 | Both, anthropic |
| claude-sonnet-4-6 | 3 | 15 | 0.3 | 3.75 | Both, anthropic |
| claude-sonnet-5 | 2 | 10 | 0.2 | 2.5 | OmO, anthropic |
| claude-sonnet-5-5 | 2 | 10 | 0.1 | 2.5 | OmO, anthropic |
| gpt-5.6 | 4 | 20 | 0.4 | 5 | omp, openai |
| gpt-5.6-luna | 0.2 | 1.2 | 0.02 | 0.25 | Both, API and subscription |
| gpt-5.6-luna-fast | 0.2 | 1.2 | 0.02 | 0.25 | OmO, API and subscription |
| gpt-5.6-sol | 4 | 20 | 0.4 | 5 | Both, API and subscription |
| gpt-5.6-sol-fast | 8 | 40 | 0.8 | 10 | OmO priority ID + omp 2x API premium |
| gpt-5.6-terra | 2 | 12 | 0.2 | 2.5 | Both, API and subscription |
| gpt-6-astra | 10 | 50 | 1 | 12.5 | OmO, API and subscription |
| gpt-6-astra-fast | 10 | 50 | 1 | 12.5 | OmO, API and subscription |
| gpt-6-luna | 0.1 | 0.5 | 0.01 | 0.125 | OmO, API and subscription |
| gpt-6-luna-fast | 0.1 | 0.5 | 0.01 | 0.125 | OmO, API and subscription |
| gpt-6-sol | 2 | 10 | 0.2 | 2.5 | Both, openai API; OmO subscription |
| gpt-6-sol-fast | 4 | 20 | 0.4 | 5 | OmO priority ID + omp 2x API premium |
| gpt-6.1-sol | 2 | 10 | 0.1 | 2.5 | Both, openai API; OmO subscription |
| gpt-6.1-sol-fast | 4 | 20 | 0.2 | 5 | OmO priority ID + omp 2x API premium |

The three Sol Fast records in OmO set `upstreamModelId` to the non-fast
model and `serviceTier` to `priority`, but copy its base `cost` fields.
Those fields alone do not price Fast runs correctly. The installed omp
harness explicitly applies a **2x API priority premium to all four cost
fields** in
`/home/pawis/.bun/install/global/node_modules/@oh-my-pi/pi-ai/src/providers/openai-shared.ts`
(`getOpenAIResponsesServiceTierCostMultiplier` and
`applyOpenAIResponsesServiceTierCost`). The Fast rows above include that
premium, rather than repeating the unadjusted catalog fields.

This is distinct from included Codex subscription usage: the current
`openai-codex:0.159.0` records in `models.db` and
`pi-catalog/src/compat/rules/providers/openai-codex.kdl` specify a 2.5x
priority multiplier for included usage (2x for purchased credits and
Enterprise pay-as-you-go). Corral's starter table estimates API-equivalent
cost, not subscription allowance consumption. Sol Fast buys speed at a
higher price, not higher quality, and is not selected by starter rules.

For luna, sol, and terra, the exact same ID exists with nonzero costs in
omp's `openai` and `openai-codex`, and OmO's `openai` and
`chatgpt-subscription`. The seeded rates are verified API-equivalent costs,
not subscription charges. No zero-cost subscription placeholder or arbitrary
alias is substituted. All starter alias arrays are empty; add only aliases
whose equivalence you have verified.

For `gpt-6-sol` and `gpt-6.1-sol`, omp's `openai-codex` records have
`cacheWrite: 0`, while the same exact model IDs in omp's `openai` and OmO's
`openai` and `chatgpt-subscription` all have `cacheWrite: 2.5`. The starter
table deliberately uses the verified API rates, not those zero subscription
cache-write fields. A profile label such as `gpt-sol` is not itself a model ID:
configure its exact selected model or an explicitly verified alias.

These are base-rate estimates, not an invoice calculator. The catalogs have
long-context tiers above 272,000 input tokens for these GPT models; this flat
schema does not express tiers (including Haiku's tier above 100,000 tokens).
The catalogs even disagree on some sol higher tiers
(omp input/output 10/45 versus OmO 8/30), while agreeing on the seeded base
rates. The Sol Fast rows include the API priority premium described above;
other service-tier adjustments, discounts, and billing adjustments are not encoded.
Verify and edit local prices before relying on estimates.

To reproduce an update, inspect the source records by exact provider and model
ID, compare their four cost fields, and update `starter_prices`, `CATALOG_DATE`,
and `CATALOG_SOURCE` in `crates/corral-core/src/pricing.rs` along with this table.
The `Store::init` date comment uses `CATALOG_DATE`. Do not map an unpriced
subscription ID to a merely similar API model. Keep unknown models unpriced
until verified.

## Amazon Bedrock catalog

Bedrock runs (`vendor = "amazon-bedrock"`) are priced from a second frozen
layer, `crates/corral-core/src/pricing_bedrock.json`, consulted after the
built-in catalog. It holds the input, output, cache read and cache write rates
(USD per million tokens) of all 193 chat models in pi-ai 1.3.0
`dist/providers/data/amazon-bedrock.json`, keyed by the bare Bedrock model ID
that Pi records (for example `us.anthropic.claude-sonnet-4-5-20250929-v1:0`).
Lookups are exact: Bedrock IDs are not mapped onto the `claude-*` keys because
regional and global Bedrock rates differ. Provenance is
`catalog-bedrock@pi-ai-1.3.0`; config prices still override it.

To regenerate after a Pi upgrade, rewrite the file from the new pi-ai JSON
(`type == "chat"` entries, `cost.cacheRead`/`cacheWrite` as
`cache_read`/`cache_write`), update the source constant, and run
`bun test test/bedrock-prices.test.ts` in `packages/corral-pi`, which checks
equality and that no extra IDs remain.
