# Shared model info

`models.json` is the source of truth for what the SDKs know about models: context windows, which models take cache points, and the id rules that map a provider's model id to the entry that describes it. Each entry is a `ModelInfo`, the same type the SDK exposes; every field is optional and a missing field means unknown. Each SDK ships a copy of this file; the harness and CLI read it through the SDK.

Edit `models.json`; each SDK copies it into its own package when it installs and builds, and does not commit the copy:

- `strands-ts`: `scripts/copy-model-info.mjs` runs on `prepare` (any `npm ci` / `npm install`) and `prebuild`, writing `strands-ts/src/models/model-info.json` (git-ignored). Run `npm run prepare -w strands-ts` after editing this file to refresh it.
- `strands-py`: a hatch build hook does the same for the Python package (comes with the Python PR).

Each SDK's unit tests assert that the copy it was built with is identical to this file.

## Format

Keys and multi-word values are camelCase, matching the TypeScript `ModelInfo` type (`cachePoints`); the Python loader re-cases both on read (`cache_points`). `capabilities.stateful` is owned by the provider and is not read from this file.

- `version`: format version. A loader rejects a version it does not know.
- `idPrefixes`: Bedrock cross-region prefixes, for code that needs to strip exactly one (`us.anthropic.claude-opus-5-5` → `anthropic.claude-opus-5-5`).
- `models`: one entry per model, keyed by the shortest id that names it (`claude-opus-5-5`, `gpt-5.4`, `amazon.nova-pro-v1:0`). A lookup tries the id as given, then without a trailing Bedrock version suffix (`-v1:0`), then without a trailing date (`-20260401`, `@20251001`), then with leading dot-separated segments removed one at a time. So `us.anthropic.claude-opus-5-5`, `anthropic.claude-opus-5-5` and `claude-opus-5-5` all resolve to the one `claude-opus-5-5` entry, and an ARN or inference-profile id resolves to its base id. Add a separate entry only when the same model differs between providers. Fields: `limits.contextWindow` in tokens; `capabilities` as in `ModelInfo`. A model the file does not list is unknown: the SDK uses its defaults for it, so new ids need an entry.
