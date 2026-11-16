Replaces the row `Injection callbacks (`trigger` predicate, `query`, `format`)` (the trigger predicate is covered by the existing row `Injection trigger predicate that fails`):

| Memory injection callbacks (`query`, `format`) | A throw skips injection | An `Err` skips injection | `Result` instead of exceptions |

New row:

| An agent that goes out of scope without `shutdown()` | `await using agent = ..` runs `shutdown()` when the scope exits, on a normal exit and on a throw, which saves conversation not yet stored in memory | Nothing is saved: `Drop` can't await. When the memory manager still holds unsaved conversation the drop logs `agent dropped with conversation not yet saved to memory; call shutdown().await before dropping it` | Rust has no async drop; decision #64 keeps `shutdown().await` explicit and warns when its work is pending (P-391) |
