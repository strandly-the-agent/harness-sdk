//! Ports the `setupTracer`/`setupMeter` cases of `strands-ts/src/telemetry/__tests__/config.test.ts`
//! and `config.test.node.ts`. Each test uses its own provider slot and a no-op registration, so no
//! process-global provider or subscriber is touched; the environment comes through the env seam.

use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use opentelemetry::baggage::BaggageExt as _;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
use opentelemetry::{Context, Key, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics, SumDataPoint,
};
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};

use super::{
    BaggageKeys, BaggageSpanProcessor, Exporters, MeterConfig, Slot, TracerConfig, resource, setup_meter_in,
    setup_tracer_in, subscriber,
};
use crate::test_support::WarnRecorder;

fn no_env(_: &str) -> Option<String> {
    None
}

fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

fn setup_tracer(slot: &Slot<SdkTracerProvider>, config: TracerConfig) -> SdkTracerProvider {
    setup_tracer_in(slot, config, &no_env, |_, _| {}).expect("a tracer provider")
}

fn setup_meter(slot: &Slot<SdkMeterProvider>, config: MeterConfig) -> SdkMeterProvider {
    setup_meter_in(slot, config, &no_env, |_| {}).expect("a meter provider")
}

#[derive(Debug, Clone, Default)]
struct SpanRecorder(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for SpanRecorder {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(batch);
        Ok(())
    }
}

impl SpanRecorder {
    fn provider(&self) -> SdkTracerProvider {
        SdkTracerProvider::builder()
            .with_simple_exporter(self.clone())
            .build()
    }

    fn names(&self) -> Vec<String> {
        let spans = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        spans.iter().map(|span| span.name.to_string()).collect()
    }
}

/// The latest export: the resource's `service.name` and the sum of each `u64` counter.
#[derive(Debug, Default)]
struct Export {
    service_name: Option<String>,
    sums: Vec<(String, u64)>,
}

#[derive(Debug, Clone, Default)]
struct MetricRecorder(Arc<Mutex<Export>>);

impl PushMetricExporter for MetricRecorder {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let service_name = metrics
            .resource()
            .get(&Key::new("service.name"))
            .map(|name| name.to_string());
        let mut sums = Vec::new();
        for metric in metrics.scope_metrics().flat_map(ScopeMetrics::metrics) {
            if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                let total = sum.data_points().map(SumDataPoint::value).sum();
                sums.push((metric.name().to_owned(), total));
            }
        }
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Export { service_name, sums };
        Ok(())
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }

    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

impl MetricRecorder {
    fn provider(&self, resource: Resource) -> SdkMeterProvider {
        SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(PeriodicReader::builder(self.clone()).build())
            .build()
    }

    fn service_name(&self) -> Option<String> {
        let export = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        export.service_name.clone()
    }

    fn sum(&self, name: &str) -> Option<u64> {
        let export = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        export
            .sums
            .iter()
            .find(|(metric, _)| metric == name)
            .map(|(_, sum)| *sum)
    }
}

fn emit_span(provider: &SdkTracerProvider, name: &'static str) {
    provider.tracer("test").start(name).end();
}

mod setup_tracer {
    use super::*;

    /// TS compares identity; here a span started on the second call's provider reaches the
    /// first call's exporter.
    #[test]
    fn should_return_the_same_provider_instance_when_called_twice() {
        let slot = Slot::default();
        let recorder = SpanRecorder::default();
        let first = TracerConfig {
            provider: Some(recorder.provider()),
            ..TracerConfig::default()
        };
        setup_tracer(&slot, first);
        let second = setup_tracer(&slot, TracerConfig::default());

        emit_span(&second, "probe");
        assert_eq!(recorder.names(), ["probe"]);
    }

    #[test]
    fn should_log_a_warning_when_called_twice() {
        let slot = Slot::default();
        let ((), warnings) = WarnRecorder::capture(|| {
            setup_tracer(&slot, TracerConfig::default());
            setup_tracer(&slot, TracerConfig::default());
        });
        assert_eq!(
            warnings,
            ["tracer provider already initialized, returning existing provider"]
        );
    }

    #[test]
    fn should_accept_a_custom_node_tracer_provider() {
        let recorder = SpanRecorder::default();
        let config = TracerConfig {
            provider: Some(recorder.provider()),
            ..TracerConfig::default()
        };
        let provider = setup_tracer(&Slot::default(), config);
        emit_span(&provider, "custom");
        assert_eq!(recorder.names(), ["custom"]);
    }

    // No TS case: TS returns right after `trace.setGlobalTracerProvider(customProvider)`
    // (`telemetry/config.ts:191-199`), so only a provider the SDK builds brings the propagators.
    #[test]
    fn registers_the_global_propagator_only_for_a_provider_it_builds() {
        let registered_as_built = |config: TracerConfig| {
            let mut seen = None;
            setup_tracer_in(&Slot::default(), config, &no_env, |_, built| seen = Some(built))
                .expect("a tracer provider");
            seen
        };
        let custom = TracerConfig {
            provider: Some(SdkTracerProvider::builder().build()),
            ..TracerConfig::default()
        };
        assert_eq!(registered_as_built(custom), Some(false));
        assert_eq!(registered_as_built(TracerConfig::default()), Some(true));
    }

    /// The processors of the provider `exporters` builds, by their debug names.
    fn processors(exporters: Exporters) -> (bool, bool) {
        let config = TracerConfig {
            exporters,
            ..TracerConfig::default()
        };
        let provider = format!("{:?}", setup_tracer(&Slot::default(), config));
        (
            provider.contains("BatchSpanProcessor"),
            provider.contains("SimpleSpanProcessor"),
        )
    }

    #[test]
    fn should_add_otlp_exporter_when_exporters_otlp_is_true() {
        let exporters = Exporters {
            otlp: true,
            console: false,
        };
        assert_eq!(processors(exporters), (true, false));
    }

    #[test]
    fn should_add_console_exporter_when_exporters_console_is_true() {
        let exporters = Exporters {
            otlp: false,
            console: true,
        };
        assert_eq!(processors(exporters), (false, true));
    }

    #[test]
    fn should_add_both_exporters_when_both_are_true() {
        let exporters = Exporters {
            otlp: true,
            console: true,
        };
        assert_eq!(processors(exporters), (true, true));
    }

    #[test]
    fn should_add_no_exporters_when_both_are_false() {
        let exporters = Exporters {
            otlp: false,
            console: false,
        };
        assert_eq!(processors(exporters), (false, false));
    }

    #[test]
    fn should_add_no_exporters_when_exporters_config_is_empty() {
        assert_eq!(processors(Exporters::default()), (false, false));
    }

    fn resource_value(env: &dyn Fn(&str) -> Option<String>, key: &'static str) -> Option<String> {
        resource(env).get(&Key::new(key)).map(|value| value.to_string())
    }

    #[test]
    fn should_use_otel_service_name_when_set() {
        let env = env_of(&[("OTEL_SERVICE_NAME", "my-custom-service")]);
        assert_eq!(
            resource_value(&env, "service.name").as_deref(),
            Some("my-custom-service")
        );
    }

    #[test]
    fn should_use_otel_service_namespace_when_set() {
        let env = env_of(&[("OTEL_SERVICE_NAMESPACE", "my-namespace")]);
        assert_eq!(
            resource_value(&env, "service.namespace").as_deref(),
            Some("my-namespace")
        );
    }

    #[test]
    fn should_use_otel_deployment_environment_when_set() {
        let env = env_of(&[("OTEL_DEPLOYMENT_ENVIRONMENT", "production")]);
        assert_eq!(
            resource_value(&env, "deployment.environment").as_deref(),
            Some("production")
        );
    }

    #[test]
    fn should_merge_otel_resource_attributes_with_defaults() {
        let env = env_of(&[(
            "OTEL_RESOURCE_ATTRIBUTES",
            "service.version=1.0.0,custom.team=platform",
        )]);
        assert_eq!(resource_value(&env, "service.version").as_deref(), Some("1.0.0"));
        assert_eq!(resource_value(&env, "custom.team").as_deref(), Some("platform"));
        assert_eq!(
            resource_value(&env, "service.name").as_deref(),
            Some("strands-agents")
        );
        assert_eq!(
            resource_value(&env, "service.namespace").as_deref(),
            Some("strands")
        );
    }

    #[test]
    fn should_allow_otel_resource_attributes_to_override_defaults() {
        let env = env_of(&[("OTEL_RESOURCE_ATTRIBUTES", "service.namespace=overridden")]);
        assert_eq!(
            resource_value(&env, "service.namespace").as_deref(),
            Some("overridden")
        );
    }

    /// The OTLP exporter posts OTLP JSON (as the TS exporter does) to the endpoint the env seam gives, from the batch
    /// processor's own thread.
    #[test]
    fn the_otlp_exporter_posts_spans_to_the_endpoint_from_the_environment() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a local port");
        let port = listener.local_addr().expect("the address").port();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("a connection");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).expect("request bytes");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
            }
            let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
            sender.send(head).ok();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .ok();
        });

        let endpoint = format!("http://127.0.0.1:{port}/");
        let env = move |name: &str| (name == "OTEL_EXPORTER_OTLP_ENDPOINT").then(|| endpoint.clone());
        let config = TracerConfig {
            exporters: Exporters {
                otlp: true,
                console: false,
            },
            ..TracerConfig::default()
        };
        let provider = setup_tracer_in(&Slot::default(), config, &env, |_, _| {}).expect("a tracer provider");
        emit_span(&provider, "probe");
        let _ = provider.force_flush();

        let head = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("an export request");
        assert!(head.starts_with("post /v1/traces http/1.1"), "{head}");
        assert!(head.contains("content-type: application/json"), "{head}");
    }
}

mod setup_meter {
    use super::*;

    #[test]
    fn returns_the_same_provider_instance_when_called_twice() {
        let slot = Slot::default();
        let recorder = MetricRecorder::default();
        let first = MeterConfig {
            provider: Some(recorder.provider(Resource::builder_empty().build())),
            ..MeterConfig::default()
        };
        setup_meter(&slot, first);
        let second = setup_meter(&slot, MeterConfig::default());

        second.meter("test").u64_counter("probe").build().add(1, &[]);
        second.force_flush().expect("flush");
        assert_eq!(recorder.sum("probe"), Some(1));
    }

    #[test]
    fn logs_a_warning_when_called_twice() {
        let slot = Slot::default();
        let ((), warnings) = WarnRecorder::capture(|| {
            setup_meter(&slot, MeterConfig::default());
            setup_meter(&slot, MeterConfig::default());
        });
        assert_eq!(
            warnings,
            ["meter provider already initialized, returning existing provider"]
        );
    }

    #[test]
    fn should_use_otel_service_name_when_set() {
        let env = env_of(&[("OTEL_SERVICE_NAME", "my-meter-service")]);
        let recorder = MetricRecorder::default();
        let config = MeterConfig {
            provider: Some(recorder.provider(resource(&env))),
            ..MeterConfig::default()
        };
        let provider = setup_meter(&Slot::default(), config);
        provider.meter("test").u64_counter("probe").build().add(1, &[]);
        provider.force_flush().expect("flush");
        assert_eq!(recorder.service_name().as_deref(), Some("my-meter-service"));
    }

    #[test]
    fn returns_a_provider_that_produces_real_metrics_via_its_own_meter() {
        let recorder = MetricRecorder::default();
        let config = MeterConfig {
            provider: Some(recorder.provider(Resource::builder_empty().build())),
            ..MeterConfig::default()
        };
        let provider = setup_meter(&Slot::default(), config);
        let counter = provider
            .meter("test-registration")
            .u64_counter("test_registration_counter")
            .build();
        counter.add(42, &[]);
        provider.force_flush().expect("flush");
        assert_eq!(recorder.sum("test_registration_counter"), Some(42));
    }

    #[test]
    fn accepts_a_custom_meter_provider() {
        let recorder = MetricRecorder::default();
        let config = MeterConfig {
            provider: Some(recorder.provider(Resource::builder_empty().build())),
            ..MeterConfig::default()
        };
        let provider = setup_meter(&Slot::default(), config);
        provider.meter("test").u64_counter("custom").build().add(2, &[]);
        provider.force_flush().expect("flush");
        assert_eq!(recorder.sum("custom"), Some(2));
    }
}

mod setup_tracer_baggage_processor {
    use super::*;

    /// Starts a span under baggage `{tenant.id, session.id, other}` on a provider with the
    /// processor and returns the attributes it stamped, sorted by key.
    fn stamped_attributes(keys: BaggageKeys) -> Vec<(String, String)> {
        let recorder = SpanRecorder::default();
        let provider = SdkTracerProvider::builder()
            .with_span_processor(BaggageSpanProcessor::new(keys))
            .with_simple_exporter(recorder.clone())
            .build();
        let baggage = [
            KeyValue::new("tenant.id", "acme"),
            KeyValue::new("session.id", "s-1"),
            KeyValue::new("other", "x"),
        ];
        let cx = Context::new().with_baggage(baggage);
        provider.tracer("test").start_with_context("probe", &cx).end();

        let spans = recorder.0.lock().unwrap_or_else(PoisonError::into_inner);
        let mut attributes: Vec<(String, String)> = spans[0]
            .attributes
            .iter()
            .map(|pair| (pair.key.to_string(), pair.value.to_string()))
            .collect();
        attributes.sort();
        attributes
    }

    fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
        expected
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn stamps_baggage_selected_by_true_onto_spans() {
        assert_eq!(
            stamped_attributes(BaggageKeys::All),
            pairs(&[("other", "x"), ("session.id", "s-1"), ("tenant.id", "acme")])
        );
    }

    #[test]
    fn stamps_baggage_selected_by_a_single_predicate_onto_spans() {
        let keys = BaggageKeys::matching(|key| key.starts_with("tenant."));
        assert_eq!(stamped_attributes(keys), pairs(&[("tenant.id", "acme")]));
    }

    #[test]
    fn stamps_baggage_selected_by_a_predicate_list_onto_spans() {
        let keys = BaggageKeys::Matching(vec![
            Arc::new(|key| key == "session.id"),
            Arc::new(|key| key.starts_with("tenant.")),
        ]);
        assert_eq!(
            stamped_attributes(keys),
            pairs(&[("session.id", "s-1"), ("tenant.id", "acme")])
        );
    }

    #[test]
    fn setup_tracer_adds_the_baggage_processor_to_the_provider_it_builds() {
        let config = TracerConfig {
            baggage_processor: Some(BaggageKeys::All),
            ..TracerConfig::default()
        };
        let provider = setup_tracer(&Slot::default(), config);
        assert!(format!("{provider:?}").contains("BaggageSpanProcessor"));
    }

    #[test]
    fn warns_that_it_is_ignored_when_a_custom_provider_is_passed() {
        let config = TracerConfig {
            provider: Some(SdkTracerProvider::builder().build()),
            baggage_processor: Some(BaggageKeys::All),
            ..TracerConfig::default()
        };
        let (provider, warnings) = WarnRecorder::capture(|| setup_tracer(&Slot::default(), config));
        assert_eq!(
            warnings,
            [
                "baggage_processor=<ignored> | custom provider is owned by the caller, add BaggageSpanProcessor to it"
            ]
        );
        assert!(!format!("{provider:?}").contains("BaggageSpanProcessor"));
    }
}

mod get_tracer_and_get_meter {
    use opentelemetry::trace::TraceContextExt as _;

    use super::*;
    use crate::telemetry::{get_meter, get_tracer};

    // No TS case: `getTracer` has no unit case in `config.test*.ts`. No test registers a global
    // provider, so the tracer is the no-op one TS documents.
    #[test]
    fn get_tracer_returns_a_tracer_of_the_global_provider() {
        let span = get_tracer().start("probe");
        let context = Context::current_with_span(span);
        assert!(!context.span().span_context().is_valid());
    }

    // No TS case: `getMeter` has no unit case in `config.test*.ts`.
    #[test]
    fn get_meter_returns_a_meter_of_the_global_provider() {
        get_meter().u64_counter("probe").build().add(1, &[]);
    }
}

mod setup_tracer_subscriber {
    use opentelemetry::trace::Status;

    use super::*;

    // No TS case: in TS only spans started through the OpenTelemetry tracer are exported and
    // `logger` calls never reach a span; this pins the same for the subscriber `setup_tracer`
    // installs.
    #[test]
    fn exports_info_spans_without_layer_attributes_debug_spans_or_log_lines() {
        let recorder = SpanRecorder::default();
        let provider = recorder.provider();
        tracing::subscriber::with_default(subscriber(&provider), || {
            tracing::info_span!("agent_span", custom = 1).in_scope(|| {
                tracing::error!("a log line");
                tracing::trace_span!("dependency_span").in_scope(|| tracing::trace!("a dependency log line"));
            });
        });

        assert_eq!(recorder.names(), ["agent_span"]);
        let spans = recorder.0.lock().unwrap_or_else(PoisonError::into_inner);
        let span = spans.first().expect("the agent span");
        let keys: Vec<&str> = span.attributes.iter().map(|pair| pair.key.as_str()).collect();
        assert_eq!(keys, ["custom"]);
        assert_eq!(span.events.len(), 0);
        assert_eq!(span.status, Status::Unset);
    }
}
