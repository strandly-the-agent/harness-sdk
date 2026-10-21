# Shared model data

`models.json` is the source of truth for what the SDKs know about models: context windows, which models take cache points, and the id rules that map a provider's model id to the entry that describes it. Each entry is a partial model info record, the same shape as the SDK's `ModelInfo`; a missing field means unknown. Both SDKs read a generated copy; the harness and CLI read it through the SDK.

Edit `models.json`, then run:

```sh
npm run model-data:generate
```

That rewrites the copies under each package (`strands-ts/src/models/data/models.json` today). `npm run model-data:check` fails when a copy differs from this file; the TypeScript test suite asserts the same.

## Format

Keys are camelCase, matching the TypeScript `ModelInfoPatch` type; the Python loader re-cases them on read.

- `version`: format version. A loader rejects a version it does not know.
- `idPrefixes`: Bedrock cross-region prefixes, for code that needs to strip exactly one (`us.anthropic.claude-opus-5-5` → `anthropic.claude-opus-5-5`).
- `models`: one entry per model, keyed by the shortest id that names it (`claude-opus-5-5`, `gpt-5.4`, `amazon.nova-pro-v1:0`). A lookup tries the id as given, then without a trailing Bedrock version suffix (`-v1:0`), then with leading dot-separated segments removed one at a time. So `us.anthropic.claude-opus-5-5`, `anthropic.claude-opus-5-5` and `claude-opus-5-5` all resolve to the one `claude-opus-5-5` entry, and an ARN or inference-profile id resolves to its base id. Add a separate entry only when the same model differs between providers. Fields: `limits.contextWindow` in tokens; `capabilities` as in `ModelInfo`; an optional `family` names the family explicitly.
- `families`: a partial record shared by every model in a family, plus `match`, the id substrings that put a model in it when its entry does not say. Model fields win over family fields.
