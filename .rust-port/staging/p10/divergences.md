| Span events and dynamic span attributes | Always recorded on the OpenTelemetry span | Recorded only with the `otel` feature; without it the `tracing` spans carry the static `gen_ai.*` fields only | `tracing` field names are static and it has no span events with dynamic names |
| `exception` span event | `exception.type`, `exception.message`, `exception.stacktrace` | `exception.message` only | A Rust error has no class name or stack |
| `telemetry.sdk.language` resource attribute | `typescript` | `rust` | Names the SDK language |
| Extra span attributes | None | A `tracing-opentelemetry` layer adds its own (`code.*`, `thread.*`, `busy_ns`, `idle_ns`, `target`) unless the application turns them off on the layer | The layer belongs to the application |
