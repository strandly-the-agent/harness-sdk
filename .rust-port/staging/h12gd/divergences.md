# h12gd divergence rows

Rows for `strands-rs/docs/DIVERGENCES.md` (first column says so) and `harness-rs/docs/DIVERGENCES.md`.

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| (strands-rs/docs/DIVERGENCES.md) Typed tool input with a key the schema does not declare | zod drops it at every level of a `z.object` (nested objects, array items, union branches, record values) and the tool runs; `.strict()` rejects it | Dropped under `properties` and `items` of an object schema with `additionalProperties: false`, and the tool runs; inside an `anyOf`/`oneOf` branch, a record value or a recursive `$ref` type it is still `Invalid input ...: Additional properties are not allowed`; an input type with `#[serde(deny_unknown_fields)]` is the strict form | The keys are dropped by walking the published JSON schema, which has no parser per union branch (P-1195) |
