//! Can a downstream helper name the builders of the Phase 10 config structs?
use strands::telemetry::{MeterConfig, TracerConfig};
pub fn a() -> strands::telemetry::TracerConfigBuilder { TracerConfig::builder() }
pub fn b() -> strands::telemetry::MeterConfigBuilder { MeterConfig::builder() }
// control: the configs and their builders work when the type is inferred
pub fn ok() -> (TracerConfig, MeterConfig) { (TracerConfig::builder().build(), MeterConfig::builder().build()) }
