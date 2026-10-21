# Shared model data

`models.json` is the source of truth for every model-id-keyed fact the SDKs ship: context windows, which models take cache points, and the id-normalization rules. Both SDKs read a generated copy of it; the harness and CLI read it through the SDK.

Edit `models.json`, then run:

```sh
npm run model-data:generate
```

That rewrites the copies under each package (`strands-ts/src/models/data/models.json` today). CI runs `npm run model-data:check` and fails when a copy differs from this file.

## Format

- `id_prefixes`: Bedrock cross-region prefixes. A lookup tries the id as given, then with one leading prefix removed (`us.anthropic.claude-sonnet-4-6` → `anthropic.claude-sonnet-4-6`).
- `families`: facts shared by every model whose id contains one of the `match` substrings. Per-model entries win over family entries.
- `models`: keyed by the vendor-prefixed base id (`anthropic.claude-sonnet-4-6`) or the provider's native id (`gpt-5.4`). Fields mirror the SDK's model info record: `limits.context_window` in tokens; `capabilities` as in `ModelInfo`.

Field names are `snake_case`; each SDK re-cases on read.
