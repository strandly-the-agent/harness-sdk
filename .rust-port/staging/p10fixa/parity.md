# p10fixa staged PARITY.md rows (full replacements)

| `strands-ts/src/telemetry/__tests__/json.test.ts` | `strands::telemetry::json::tests` | 9/15 | 6 not applicable: `bigint`, symbol, `ArrayBuffer`, `Uint8Array` and the two mixed-content cases have no typed Rust value (P-484); `replaces functions` stands for the failing-serializer path; `handles Date objects nested in objects` is covered by `handles_date_objects` |
