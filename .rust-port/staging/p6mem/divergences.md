| `MemoryStore` optional methods | `add`, `addMessages`, `initialize` and `getTools` may be absent | Always present with defaults; `supports_add()` and `supports_add_messages()` say which write methods the store implements | An object-safe trait can't omit methods |
| `MemoryStore.add` return value | `Promise<unknown>` | `Result<()>`; `FileMemoryStore::add` (inherent) returns the key | No caller reads the value |
| Memory `boolean \| Config` options | `true`, `false` or a config object | `MemorySetting::{On, Off, Config}` (from `bool` or the config); `extraction` is an `Option` | Rust has no untagged unions |
| `addToolConfig.stores` | Names or store instances | Names | An instance resolves to its name in TS |
| `ExtractionConfig.trigger` | One trigger or a list | A list | One shape instead of a union |
| Memory `AggregateError` | `add` rejects with an `AggregateError` whose `errors` hold the store failures | One `Error::Other` with the same message; the store failures are logged | `strands::Error` has no aggregate variant |
| Memory extraction scheduling | A promise chain per store on the event loop | A spawned tokio task per save, one at a time per store; without a runtime the save waits for `flush` | Tokio tasks replace the event loop |
| `add_memory` with `waitForWrites: false` | Writes continue on the event loop | Writes run on a spawned tokio task | Same |
| Memory spans | Search, add, inject and extract spans | None yet | Lands with telemetry (Phase 10) |
| `ModelExtractor` entry metadata | Any JSON value the model returns under `metadata` is passed to `store.add` | Only an object is kept | `MemoryStore::add` takes a string-keyed map, as the TS type says |
| `ModelExtractor` warnings | `model=<constructor name>` | `model=<model id>` | Rust trait objects have no class name |
| `search_memory` `maxSearchResults` | Any number is forwarded | Rounded up to a whole number, negatives become 0 | The Rust limit is a `usize` |
| `FileMemoryStore` `search` option | Deprecated store-level search strategy | Not available; set the strategy on the storage backend | Decision #24: deprecated TS API is not ported |
| Injection callbacks (`trigger` predicate, `query`, `format`) | A throw skips injection | An `Err` skips injection | `Result` instead of exceptions |
