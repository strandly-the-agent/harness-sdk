//! Ports `harness-ts/test/models.test.ts`: the cases that need no provider feature are here,
//! the ones that read a provider's model config are in `providers`.

use std::sync::Arc;

use strands::agent::ModelSource;
use strands::models::Model;
use strands::models::routing::ModelRouter;
use strands::testing::MockMessageModel;

use super::*;

#[cfg(all(feature = "bedrock", feature = "anthropic", feature = "openai"))]
mod providers;

const DEFAULT: &str = "bedrock/global.anthropic.claude-opus-4-8";

fn spec(spec: &str) -> ModelSource {
    spec.into()
}

fn instance() -> ModelSource {
    let model: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    model.into()
}

fn router() -> ModelSource {
    let fast: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    ModelRouter::new([fast]).unwrap().into()
}

fn effort(effort: Effort) -> ResolveModelOptions {
    ResolveModelOptions {
        effort,
        ..Default::default()
    }
}

fn plan_error(spec: &str, options: ResolveModelOptions) -> String {
    match plan(spec, options) {
        Ok(_) => panic!("{spec} resolved"),
        Err(error) => error.to_string(),
    }
}

mod resolve_model {
    use super::*;

    fn no_env() -> EnvLookup {
        Arc::new(|_| None)
    }

    #[test]
    fn passes_a_model_instance_through_untouched() {
        let instance = instance();
        let resolved = resolve_model(Some(&instance), DEFAULT, effort(Effort::Auto), &no_env()).unwrap();
        let (ModelSource::Model(given), ModelSource::Model(resolved)) = (&instance, &resolved) else {
            panic!("not a model");
        };
        assert!(Arc::ptr_eq(given, resolved));
    }

    #[test]
    fn passes_a_model_router_through_untouched() {
        let fast: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let deep: Arc<dyn Model> = Arc::new(MockMessageModel::new());
        let router = ModelRouter::new([Arc::clone(&fast), deep]).unwrap();
        let resolved = resolve_model(Some(&router.into()), DEFAULT, effort(Effort::Auto), &no_env()).unwrap();
        let ModelSource::Router(resolved) = resolved else {
            panic!("not a router");
        };
        assert!(Arc::ptr_eq(&resolved.default_model(), &fast));
    }

    #[test]
    fn rejects_an_unknown_provider() {
        assert_eq!(
            plan_error("mistral/whatever", effort(Effort::Auto)),
            "Unknown model provider \"mistral\" in \"mistral/whatever\". Supported providers: anthropic, bedrock, bedrock-mantle, openai. Pass a Model instance for anything else."
        );
    }

    #[test]
    fn does_not_treat_a_gemini_prefix_as_a_provider() {
        assert!(
            plan_error("gemini/gemini-3.5-flash", effort(Effort::Auto))
                .starts_with("Unknown model provider \"gemini\" in \"gemini/gemini-3.5-flash\".")
        );
    }

    /// Rust v0.1 has no Google, Ollama or LiteLLM provider (PROMPT.md section 10).
    #[test]
    fn rejects_a_provider_typescript_has_and_rust_does_not() {
        for provider in ["google", "ollama", "litellm"] {
            assert_eq!(
                plan_error(&format!("{provider}/some-model"), effort(Effort::Auto)),
                format!("model provider \"{provider}\" is not supported in Rust yet")
            );
        }
    }

    #[cfg(not(feature = "anthropic"))]
    #[test]
    fn reports_the_cargo_feature_of_a_disabled_provider() {
        let error = resolve_spec("anthropic/claude-opus-4-8", effort(Effort::Auto), &no_env())
            .err()
            .map(|error| error.to_string());
        assert_eq!(
            error.as_deref(),
            Some("model 'anthropic/claude-opus-4-8' requires the strands feature \"anthropic\"")
        );
    }

    #[test]
    fn rejects_a_level_the_provider_does_not_support() {
        assert_eq!(
            plan_error("openai/gpt-5.6-sol", effort(Effort::Max)),
            "Effort \"max\" is not supported by model gpt-5.6-sol. Supported levels: minimal, low, medium, high, xhigh (or 'auto', 'off')."
        );
    }

    #[test]
    fn rejects_a_level_the_model_does_not_support() {
        assert!(
            plan_error(
                "bedrock/global.anthropic.claude-opus-4-8",
                effort(Effort::Minimal)
            )
            .contains("not supported by Bedrock model global.anthropic.claude-opus-4-8")
        );
    }

    #[test]
    fn rejects_explicit_thinking_for_an_unknown_bedrock_family() {
        assert_eq!(
            plan_error("bedrock/amazon.nova-pro-v1:0", effort(Effort::High)),
            "Effort \"high\" is not supported by Bedrock model amazon.nova-pro-v1:0. It supports no reasoning levels; pass 'auto' or 'off'."
        );
    }

    #[test]
    fn rejects_an_unsupported_thinking_level_for_bedrock_mantle() {
        assert!(
            plan_error("bedrock-mantle/openai.gpt-oss-120b", effort(Effort::Max))
                .contains("not supported by model openai.gpt-oss-120b")
        );
    }

    #[test]
    fn rejects_an_explicit_thinking_level_on_a_claude_below_the_thinking_floor() {
        assert!(
            plan_error(
                "bedrock/us.anthropic.claude-3-haiku-20240307-v1:0",
                effort(Effort::High)
            )
            .contains("not supported by Bedrock model")
        );
    }

    #[test]
    fn never_adds_a_native_search_tool_where_the_model_has_none() {
        for id in [
            "bedrock-mantle/qwen.qwen3-32b-v1:0",
            "bedrock-mantle/openai.gpt-oss-120b-1:0",
            DEFAULT,
        ] {
            let options = ResolveModelOptions {
                effort: Effort::High,
                web_search: true,
                ..Default::default()
            };
            assert!(!plan(id, options).unwrap().web_search, "{id}");
        }
    }
}

mod supports_thinking {
    use super::*;

    fn thinks(model: &str) -> bool {
        supports_thinking(Some(&spec(model)))
    }

    #[test]
    fn answers_for_consumers() {
        assert!(thinks("bedrock/global.anthropic.claude-opus-4-8"));
        assert!(thinks("bedrock/global.anthropic.claude-haiku-4-5-20251001-v1:0"));
        assert!(thinks("anthropic/claude-haiku-4-5-20251001"));
        assert!(!thinks("bedrock/us.anthropic.claude-3-haiku-20240307-v1:0"));
        assert!(!thinks("anthropic/claude-3-haiku-20240307"));
        assert!(!thinks("ollama/llama3"));
        assert!(supports_thinking(None));
    }

    #[test]
    fn is_true_for_a_model_instance() {
        assert!(supports_thinking(Some(&instance())));
    }

    #[test]
    fn is_true_for_a_model_router() {
        assert!(supports_thinking(Some(&router())));
    }

    #[test]
    fn is_false_for_which_takes_no_level() {
        for model_id in [
            "amazon.nova-pro-v1:0",
            "us.amazon.nova-pro-v1:0",
            "meta.llama3-3-70b-instruct-v1:0",
            "deepseek.r1-v1:0",
            "mistral.mistral-large-2407-v1:0",
        ] {
            let model = format!("bedrock/{model_id}");
            assert!(!thinks(&model), "{model}");
            assert!(plan_error(&model, effort(Effort::High)).contains("not supported by Bedrock model"));
        }
    }

    #[test]
    fn reads_the_dot_separated_version_in_as_thinking() {
        for (model_id, expected) in [
            ("claude-opus-4.8", true),
            ("claude-sonnet-4.6", true),
            ("claude-opus-4.5", true),
            ("claude-haiku-4.5", true),
            ("claude-opus-4.1", false),
        ] {
            let model = format!("bedrock/global.anthropic.{model_id}");
            assert_eq!(thinks(&model), expected, "{model}");
        }
    }

    #[test]
    fn is_true_for_an_unrecognized_claude_family() {
        assert!(thinks("bedrock/global.anthropic.claude-quasar-6"));
    }
}

mod supports_web_search {
    use super::*;

    fn searches(model: &str) -> bool {
        supports_web_search(Some(&spec(model)))
    }

    /// Google is not a provider of Rust v0.1, so its row of the TS case is `false` here.
    #[test]
    fn is_true_for_openai_google_and_bedrock_mantle() {
        assert!(searches("openai/gpt-5.6-sol"));
        assert!(!searches("google/gemini-3.5-flash"));
        assert!(searches("bedrock-mantle/openai.gpt-5.6-luna"));
        assert!(searches("bedrock-mantle/openai.gpt-6-astra"));
        assert!(searches("bedrock-mantle/openai.gpt-6.1-sol"));
        assert!(searches("bedrock-mantle/openai.gpt-daybreak-blue-5.6-sol"));
        assert!(!searches("bedrock-mantle/openai.gpt-oss-120b"));
        assert!(!searches("bedrock-mantle/qwen.qwen3-32b-v1:0"));
    }

    #[test]
    fn is_false_for_bedrock_but_true_for_anthropic_direct() {
        assert!(!searches("bedrock/global.anthropic.claude-opus-4-8"));
        assert!(searches("anthropic/claude-opus-4-8"));
    }

    #[test]
    fn uses_the_default_model_provider_for_undefined() {
        assert!(!supports_web_search(None));
    }

    #[test]
    fn is_false_for_a_model_instance() {
        assert!(!supports_web_search(Some(&instance())));
    }

    #[test]
    fn is_false_for_a_model_router() {
        assert!(!supports_web_search(Some(&router())));
    }
}

mod supports_media {
    use super::*;

    #[test]
    fn is_false_for_openai_family_models_on_bedrock_converse() {
        assert!(!supports_media(Some(&spec("bedrock/us.openai.gpt-6-astra"))));
        assert!(!supports_media(Some(&spec("bedrock/openai.gpt-5.6-sol"))));
    }

    #[test]
    fn is_true_elsewhere() {
        assert!(supports_media(Some(&spec(DEFAULT))));
        assert!(supports_media(Some(&spec("anthropic/claude-haiku-4-5-20251001"))));
        assert!(supports_media(Some(&spec("bedrock-mantle/openai.gpt-6-astra"))));
        assert!(supports_media(None));
        assert!(supports_media(Some(&instance())));
    }
}
