# Shared model data

`models.json` is the source of truth for every model-id-keyed fact the SDKs ship: context windows, which models take cache points, and the id-normalization rules. Both SDKs read a generated copy of it; the harness and CLI read it through the SDK.

Edit `models.json`, then run:

```sh
npm run model-data:generate
```

That rewrites the copies under each package (`strands-ts/src/models/data/models.json` today). CI runs `npm run model-data:check` and fails when a copy differs from this file.

## Format

- `version`: format version of this file; loaders reject a version they do not know.
- `id_prefixes`: Bedrock cross-region prefixes, for code that needs to strip one (`us.anthropic.claude-sonnet-4-6` → `anthropic.claude-sonnet-4-6`).
- `models`: keyed by the vendor-prefixed base id (`anthropic.claude-sonnet-4-6`) or the provider's native id (`gpt-5.4`). A lookup tries the id as given, then removes leading dot-separated segments one at a time until it finds an entry, so prefixed ids, ARNs and inference-profile ids resolve to their base id. Fields mirror the SDK's model info record: `limits.context_window` in tokens; `capabilities` as in `ModelInfo`; an optional `family` names the family explicitly.
- `families`: facts shared by every model in a family. A model belongs to a family through its `family` field or, failing that, when its id contains one of the family's `match` substrings. Per-model fields win over family fields.

Field names are `snake_case`; each SDK re-cases on read.
