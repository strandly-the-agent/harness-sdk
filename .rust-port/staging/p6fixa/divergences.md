replaces the row `` `SKILL.md` frontmatter syntax ``

| `SKILL.md` frontmatter syntax | Full YAML (`yaml` package), with a colon-quoting retry | A YAML subset: plain and quoted scalars (with the YAML double-quote escapes), `\|` and `>` block scalars, flow and block sequences of scalars, nested mappings; multi-line plain scalars, anchors, tags, flow mappings and sequences of mappings are skipped | No YAML crate in the workspace (P-384, P-603) |

replaces the row `` `MemoryStore.add` return value ``

| `MemoryStore.add` return value | `Promise<unknown>` | `Result<()>`; `FileMemoryStore::add_entry` (inherent) returns the key | No caller reads the value (P-606) |

new rows

| `MemoryManager.flush` and detached `add_memory` writes | `flush` waits for extraction only; a `waitForWrites: false` write is kept alive by the event loop | `flush` (and so `Agent::shutdown`) also waits for those writes | A Tokio runtime that stops drops a spawned task (P-600) |
| Memory store or extractor that panics | Not applicable (a throw rejects the save) | A panic is a failed save (`memory store save panicked`): rolled back, counted, retried | A panic would poison the store's save queue (P-601) |
