//! Telemetry settings read from the environment (TS `telemetry/utils.ts`, the `Tracer` constructor
//! and `getOtelResource` in `telemetry/config.ts`).

#[cfg(feature = "otel")]
pub(crate) mod setup;

/// The crate's env-lookup seam for `OTEL_*` and `LANGFUSE_*`.
pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;

pub(crate) fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The service name when `OTEL_SERVICE_NAME` is unset; also the tracer and meter scope name.
const DEFAULT_SERVICE_NAME: &str = "strands-agents";

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
    not(any(test, feature = "otel")),
    expect(dead_code, reason = "used by the `otel` setup")
)]
pub(crate) fn resource_attributes(env: EnvLookup<'_>) -> Vec<(String, String)> {
    const DEFAULT_SERVICE_NAMESPACE: &str = "strands";
    const DEFAULT_DEPLOYMENT_ENVIRONMENT: &str = "development";
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
    let mut set =
        |key: String, value: String| match attributes.iter_mut().find(|(existing, _)| *existing == key) {
            Some(existing) => existing.1 = value,
            None => attributes.push((key, value)),
        };
    let detected = env("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default();
    for (key, value) in detected_attributes(&detected).unwrap_or_default() {
        set(key, value);
    }
    // The detector applies `OTEL_SERVICE_NAME` after the pairs, so it wins over a `service.name` pair.
    if let Some(name) = env("OTEL_SERVICE_NAME").filter(|name| !name.trim().is_empty()) {
        set("service.name".to_owned(), name);
    }
    attributes
}

/// The pairs of `OTEL_RESOURCE_ATTRIBUTES` as the `@opentelemetry/resources` `envDetector` reads
/// them: percent-decoded, and `None` (the whole variable is ignored) when an entry is not exactly
/// `key=value`, has an empty key, doesn't decode or is longer than 255 UTF-16 units.
fn detected_attributes(raw: &str) -> Option<Vec<(String, String)>> {
    const MAX_LENGTH: usize = 255;
    raw.split(',')
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| {
            let (key, value) = entry.split_once('=').filter(|(_, value)| !value.contains('='))?;
            let (key, value) = (percent_decode(key.trim())?, percent_decode(value.trim())?);
            let fits = |text: &str| text.encode_utf16().count() <= MAX_LENGTH;
            (!key.is_empty() && fits(&key) && fits(&value)).then_some((key, value))
        })
        .collect()
}

/// JS `decodeURIComponent`: `None` for a `%` without two hex digits or bytes that aren't UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        if byte == b'%' {
            let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            rest = tail.get(2..)?;
        } else {
            bytes.push(byte);
            rest = tail;
        }
    }
    String::from_utf8(bytes).ok()
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

    // No TS case: the three cases below pin what `envDetector` of `@opentelemetry/resources` does
    // with `OTEL_RESOURCE_ATTRIBUTES`, which TS `getOtelResource` spreads over the defaults.
    #[test]
    fn otel_service_name_wins_over_a_service_name_resource_attribute() {
        let env = env_of(&[
            ("OTEL_SERVICE_NAME", "svc-env"),
            ("OTEL_RESOURCE_ATTRIBUTES", "service.name=svc-attr,team=a"),
        ]);
        let attributes = resource_attributes(&env);
        let get = |key: &str| attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("service.name"), Some("svc-env"));
        assert_eq!(get("team"), Some("a"));
    }

    #[test]
    fn resource_attributes_are_percent_decoded() {
        let env = env_of(&[(
            "OTEL_RESOURCE_ATTRIBUTES",
            "te%61m=a%20b%2Cc%3Dd, caf%C3%A9 = 1 ,,",
        )]);
        let attributes = resource_attributes(&env);
        let get = |key: &str| attributes.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("team"), Some("a b,c=d"));
        assert_eq!(get("café"), Some("1"));
    }

    #[test]
    fn a_malformed_resource_attributes_variable_is_ignored_as_a_whole() {
        let long = format!("env=prod,key={}", "v".repeat(256));
        let malformed = [
            "team=a=b,env=prod",
            "env=prod,novalue",
            "=x,env=prod",
            "env=prod,team=%zz",
            "env=prod,team=%ff",
            long.as_str(),
        ];
        for raw in malformed {
            let env = |name: &str| (name == "OTEL_RESOURCE_ATTRIBUTES").then(|| raw.to_owned());
            let attributes = resource_attributes(&env);
            let keys: Vec<&str> = attributes.iter().map(|(key, _)| key.as_str()).collect();
            assert_eq!(
                keys,
                [
                    "service.name",
                    "service.namespace",
                    "deployment.environment",
                    "telemetry.sdk.name",
                    "telemetry.sdk.language"
                ],
                "{raw}"
            );
        }
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
