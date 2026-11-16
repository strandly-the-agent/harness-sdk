# PARITY rows to replace (track p11titles)

Each row replaces the `strands-rs/docs/PARITY.md` row with the same first column.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | `strands::models::model_proxy::tests` | 18/19 | `toBe` identity checks compare by value; `does not change the messages it was given` (1) does not apply: a `ModelRequest` owns its messages, so the proxy has no caller copy to change |
