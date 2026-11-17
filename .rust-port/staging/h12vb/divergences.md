# Track h12vb divergence rows

| Area | TS | Rust | Why |
|---|---|---|---|
| (replaces the row of the same first column in `strands-rs/docs/DIVERGENCES.md`, line 206) Typed tool input with a key the schema does not declare | zod drops it at every level of a `z.object` (nested objects, array items, tuple items, union branches, record values) and the tool runs; `.strict()` rejects it | Dropped under `properties`, `items` and `prefixItems` (a tuple item, by position) of an object schema with `additionalProperties: false`, and the tool runs; inside an `anyOf`/`oneOf` branch, a record value or a recursive `$ref` type it is still `Invalid input ...: Additional properties are not allowed`; an input type with `#[serde(deny_unknown_fields)]` is the strict form | The keys are dropped by walking the published JSON schema, which has no parser per union branch (P-1195, P-1220) |
