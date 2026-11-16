//! Telemetry settings read from the environment (TS `telemetry/utils.ts`, the `Tracer` constructor
//! and `getOtelResource` in `telemetry/config.ts`).

/// The crate's env-lookup seam for `OTEL_*` and `LANGFUSE_*`.
pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

pub(crate) fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The service name when `OTEL_SERVICE_NAME` is unset; also the tracer and meter scope name.
pub const DEFAULT_SERVICE_NAME: &str = "strands-agents";
const DEFAULT_SERVICE_NAMESPACE: &str = "strands";
const DEFAULT_DEPLOYMENT_ENVIRONMENT: &str = "development";

fn non_empty(env: EnvLookup<'_>, name: &str) -> Option<String> {
    env(name).filter(|value| !value.is_empty())
}

/// TS `getServiceName`.
pub(crate) fn service_name(env: EnvLookup<'_>) -> String {
    non_empty(env, "OTEL_SERVICE_NAME").unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned())
}

/// The `gen_ai.*` convention switches (TS `OTEL_SEMCONV_STABILITY_OPT_IN` and Langfuse detection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Semconv {
    /// `gen_ai_latest_experimental`.
    pub(crate) latest: bool,
    /// `gen_ai_tool_definitions`.
    pub(crate) tool_definitions: bool,
    pub(crate) langfuse: bool,
    /// Langfuse, or `gen_ai_span_attributes_only`: message content goes to span attributes.
    pub(crate) span_attributes_only: bool,
    pub(crate) service_name: String,
}

impl Default for Semconv {
    fn default() -> Self {
        Self::from_env(&|_| None)
    }
}

impl Semconv {
    pub(crate) fn from_env(env: EnvLookup<'_>) -> Self {
        let opt_in = env("OTEL_SEMCONV_STABILITY_OPT_IN").unwrap_or_default();
        let has = |name: &str| opt_in.split(',').any(|value| value.trim() == name);
        let langfuse = detect_langfuse(env);
        Self {
            latest: has("gen_ai_latest_experimental"),
            tool_definitions: has("gen_ai_tool_definitions"),
            langfuse,
            span_attributes_only: langfuse || has("gen_ai_span_attributes_only"),
            service_name: service_name(env),
        }
    }
}

fn detect_langfuse(env: EnvLookup<'_>) -> bool {
    non_empty(env, "LANGFUSE_BASE_URL").is_some()
        || [
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        ]
        .iter()
        .any(|name| env(name).is_some_and(|value| value.contains("langfuse")))
}

/// TS `getOtelResource`: the defaults, then `OTEL_RESOURCE_ATTRIBUTES` (`key=value,..`) over them.
#[cfg_attr(
    not(feature = "otel"),
    expect(dead_code, reason = "used by the `otel` setup functions")
)]
pub(crate) fn resource_attributes(env: EnvLookup<'_>) -> Vec<(String, String)> {
    let mut attributes = vec![
        ("service.name".to_owned(), service_name(env)),
        (
            "service.namespace".to_owned(),
            non_empty(env, "OTEL_SERVICE_NAMESPACE").unwrap_or_else(|| DEFAULT_SERVICE_NAMESPACE.to_owned()),
        ),
        (
            "deployment.environment".to_owned(),
            non_empty(env, "OTEL_DEPLOYMENT_ENVIRONMENT")
                .unwrap_or_else(|| DEFAULT_DEPLOYMENT_ENVIRONMENT.to_owned()),
        ),
        ("telemetry.sdk.name".to_owned(), "opentelemetry".to_owned()),
        ("telemetry.sdk.language".to_owned(), "rust".to_owned()),
    ];
    let detected = env("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default();
    for (key, value) in detected.split(',').filter_map(|pair| pair.split_once('=')) {
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() {
            continue;
        }
        match attributes.iter_mut().find(|(existing, _)| existing == key) {
            Some(existing) => existing.1 = value.to_owned(),
            None => attributes.push((key.to_owned(), value.to_owned())),
        }
    }
    attributes
}

#[cfg(test)]
mod tests {
    //! Ports the environment cases of `telemetry/__tests__/tracer.test.node.ts` (semantic-convention
    //! opt-in, Langfuse detection) and the resource cases of `telemetry/__tests__/config.test.ts`.

    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[test]
    fn should_use_strands_agents_as_default_service_name() {
        assert_eq!(service_name(&|_| None), "strands-agents");
        assert_eq!(
            service_name(&env_of(&[("OTEL_SERVICE_NAME", "")])),
            "strands-agents"
        );
        assert_eq!(service_name(&env_of(&[("OTEL_SERVICE_NAME", "mine")])), "mine");
    }

    #[test]
    fn should_include_default_resource_attributes() {
        let attributes = resource_attributes(&|_| None);
        let pairs: Vec<(&str, &str)> = attributes.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            pairs,
            [
                ("service.name", "strands-agents"),
                ("service.namespace", "strands"),
                ("deployment.environment", "development"),
                ("telemetry.sdk.name", "opentelemetry"),
                ("telemetry.sdk.language", "rust"),
            ]
        );
    }

    #[test]
    fn resource_attributes_from_the_environment_override_the_defaults() {
        let env = env_of(&[
            ("OTEL_SERVICE_NAMESPACE", "team"),
            ("OTEL_DEPLOYMENT_ENVIRONMENT", "prod"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.name=from-attrs, host.name=box",
            ),
        ]);
        let attributes = resource_attributes(&env);
        let get = |key: &str| attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("service.name"), Some("from-attrs"));
        assert_eq!(get("service.namespace"), Some("team"));
        assert_eq!(get("deployment.environment"), Some("prod"));
        assert_eq!(get("host.name"), Some("box"));
    }

    #[test]
    fn uses_the_stable_conventions_by_default() {
        let semconv = Semconv::from_env(&|_| None);
        assert_eq!(
            semconv,
            Semconv {
                latest: false,
                tool_definitions: false,
                langfuse: false,
                span_attributes_only: false,
                service_name: "strands-agents".to_owned(),
            }
        );
    }

    #[test]
    fn parses_the_comma_separated_opt_in_values() {
        let env = env_of(&[(
            "OTEL_SEMCONV_STABILITY_OPT_IN",
            " gen_ai_latest_experimental , gen_ai_tool_definitions,,gen_ai_span_attributes_only",
        )]);
        let semconv = Semconv::from_env(&env);
        assert!(semconv.latest && semconv.tool_definitions && semconv.span_attributes_only);
        assert!(!semconv.langfuse);
    }

    #[test]
    fn detects_langfuse_from_the_base_url_and_the_otlp_endpoints() {
        for pairs in [
            &[("LANGFUSE_BASE_URL", "https://example.test")][..],
            &[(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "https://cloud.langfuse.example/api",
            )][..],
            &[(
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "https://langfuse.example/v1/traces",
            )][..],
        ] {
            let pairs: &'static [(&str, &str)] = Box::leak(pairs.to_vec().into_boxed_slice());
            let semconv = Semconv::from_env(&env_of(pairs));
            assert!(semconv.langfuse && semconv.span_attributes_only, "{pairs:?}");
        }
        let other = env_of(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "https://collector.example")]);
        assert!(!Semconv::from_env(&other).langfuse);
    }
}
