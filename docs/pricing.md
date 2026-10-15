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
exact key first, then explicit aliases, and returns `None` for unknown models.
It does not normalize case, strip providers, match prefixes, or guess model
families. `Config` retains `PartialEq` but cannot implement `Eq` with floats.

`Config::validate` rejects negative or non-finite rates, aliases owned by
multiple entries, and aliases that collide with another exact model key.
Validated config saves use this same validation. Loading older configs without
`pricing` yields an empty table; loading does not silently install defaults.

## Starter table and provenance

Only creation of a **new** `.corral/corral.toml` by `Store::init` installs this
small frozen starter table, preceded by:

```toml
# prices as of 2026-10-15, verify
```

Existing files are never reseeded or overwritten, including files with no
pricing or locally edited rates. `Config::default_for` has empty pricing.
The shipped table needs no runtime dependency on an agent installation.

These values were extracted from the actual installed catalogs on
October 15, 2026, not inferred from model names:

- omp: `/home/pawis/.bun/install/global/node_modules/@oh-my-pi/pi-catalog/src/models.json`,
  provider keys `anthropic`, `openai`, and `openai-codex`.
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
| gpt-5.6-sol-fast | 4 | 20 | 0.4 | 5 | OmO, API and subscription |
| gpt-5.6-terra | 2 | 12 | 0.2 | 2.5 | Both, API and subscription |
| gpt-6-astra | 10 | 50 | 1 | 12.5 | OmO, API and subscription |
| gpt-6-astra-fast | 10 | 50 | 1 | 12.5 | OmO, API and subscription |
| gpt-6-luna | 0.1 | 0.5 | 0.01 | 0.125 | OmO, API and subscription |
| gpt-6-luna-fast | 0.1 | 0.5 | 0.01 | 0.125 | OmO, API and subscription |
| gpt-6-sol | 2 | 10 | 0.2 | 2.5 | Both, openai API; OmO subscription |
| gpt-6-sol-fast | 2 | 10 | 0.2 | 2.5 | OmO, API and subscription |
| gpt-6.1-sol | 2 | 10 | 0.1 | 2.5 | Both, openai API; OmO subscription |
| gpt-6.1-sol-fast | 2 | 10 | 0.1 | 2.5 | OmO, API and subscription |

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
rates. Discounts, service tiers, and other billing adjustments are not encoded.
Verify and edit local prices before relying on estimates.

To reproduce an update, inspect the source records by exact provider and model
ID, compare their four cost fields, and update `starter_prices` in
`crates/corral-core/src/pricing.rs` along with this table and the date comment
in `Store::init`. Do not map an unpriced subscription ID to a merely similar
API model. Keep unknown models unpriced until verified.
