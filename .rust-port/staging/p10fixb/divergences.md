Replaces the row `Extra span attributes`:

| Spans exported besides the agent's | Only spans started through the OpenTelemetry tracer; `logger` calls never reach a span | `tracing` spans at `INFO` and above that the application opens are exported too (they parent under the agent's); with an application-built subscriber, `tracing_layer` exports whatever that subscriber lets through, log lines as span events included, unless the application filters the layer. The layer adds no attributes of its own | `tracing` is the span API of the Rust crate; `setup_tracer` filters its own subscriber to spans at `INFO` and above (P-721) |

Replaces the row `Memory spans`:

| Memory spans | `memory.extract` records `memory.extractor` (the extractor's class name) | No `memory.extractor` attribute | A trait object has no class name (P-494) |

New row:

| Logging | `configureLogging(logger)` and the `Logger` interface, exported from the package root; the default logger prints warnings and errors to the console | No `configure_logging` and no `Logger`: the crate logs through `tracing` events, each with one `message` field in the TS text format, and prints nothing until the application installs a subscriber. The subscriber `setup_tracer` installs exports spans only, so it prints no log lines either | `tracing` is the logging facade of Rust libraries; a second logger hook would duplicate it |
