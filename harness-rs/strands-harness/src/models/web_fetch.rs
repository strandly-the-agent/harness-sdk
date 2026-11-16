//! The model the `web_fetch` summarizer and memory extraction run on (TS `resolveWebFetchModel`).

use std::sync::Arc;

use strands::agent::ModelSource;
use strands::models::Model;

use super::{
    ResolveModelOptions, UNSUPPORTED_PROVIDERS, bedrock_family, lookup, resolve_spec, split_provider,
};
use crate::defaults::DEFAULT_MODEL;
use crate::env::EnvLookup;
use crate::error::{HarnessError, Result};
use crate::logging::warn_once;
use crate::types::Effort;

/// Small, fast model per provider for the `web_fetch` summarizer, keyed by the main agent's
/// provider so the summarizer shares its credentials.
const WEB_FETCH_MODELS: [(&str, &str); 4] = [
    ("bedrock", "global.anthropic.claude-haiku-4-5-20251001-v1:0"),
    ("bedrock-mantle", "openai.gpt-5.6-luna"),
    ("anthropic", "claude-haiku-4-5-20251001"),
    ("openai", "gpt-5.6-luna"),
];
/// Providers whose endpoint an env var can repoint; such an endpoint serves its own model list.
const CUSTOM_ENDPOINT_VARS: [(&str, &str); 2] =
    [("anthropic", "ANTHROPIC_BASE_URL"), ("openai", "OPENAI_BASE_URL")];

/// Resolves the model the `web_fetch` summarizer runs on.
///
/// An explicit `web_fetch_model` wins. With none set, the small fast model of the main agent's
/// provider is used, so the summarizer shares its credentials; a main model instance is reused.
/// On Bedrock the summarizer follows the main model's family, and a family that can't be
/// identified reuses the main model with a warning. Thinking and caching are never applied.
pub(crate) fn resolve_web_fetch_model(
    main_model: Option<&ModelSource>,
    web_fetch_model: Option<&ModelSource>,
    env: &EnvLookup,
) -> Result<Arc<dyn Model>> {
    let chosen = web_fetch_model.or(main_model);
    let main = match chosen {
        Some(ModelSource::Model(model)) => return Ok(Arc::clone(model)),
        Some(ModelSource::Router(router)) => return Ok(router.default_model()),
        Some(ModelSource::Id(spec)) if web_fetch_model.is_some() => return summarizer(spec, env),
        Some(ModelSource::Id(spec)) => spec,
        _ => DEFAULT_MODEL,
    };
    match web_fetch_spec(main, env)? {
        Some(small) => summarizer(&small, env),
        None => summarizer(main, env),
    }
}

fn summarizer(spec: &str, env: &EnvLookup) -> Result<Arc<dyn Model>> {
    let options = ResolveModelOptions {
        effort: Effort::Off,
        ..Default::default()
    };
    resolve_spec(spec, options, env)
}

/// The summarizer's model string for the main model string `main`, or `None` to reuse `main`.
pub(super) fn web_fetch_spec(main: &str, env: &EnvLookup) -> Result<Option<String>> {
    let (provider_name, name) = split_provider(main);
    if provider_name == "bedrock" {
        let small = bedrock_web_fetch_model(name);
        if small.is_none() {
            warn_once(&format!(
                "model=<{main}> | could not identify the Bedrock model family for the web_fetch summarizer; reusing the main model. Pass builtinTools.web_fetch.model to choose a smaller one."
            ));
        }
        return Ok(small.map(|small| format!("bedrock/{small}")));
    }
    let custom_endpoint = lookup(&CUSTOM_ENDPOINT_VARS, provider_name)
        .filter(|var| env(var).is_some_and(|value| !value.is_empty()));
    if let Some(base_url_var) = custom_endpoint {
        warn_once(&format!(
            "model=<{main}> | {base_url_var} points provider <{provider_name}> at a non-default endpoint, which serves its own model list, so the vended summarizer may not exist there; reusing the main model. Pass builtinTools.web_fetch.model to choose a smaller one."
        ));
        return Ok(None);
    }
    if let Some(small) = lookup(&WEB_FETCH_MODELS, provider_name) {
        return Ok(Some(format!("{provider_name}/{small}")));
    }
    if UNSUPPORTED_PROVIDERS.contains(&provider_name) {
        // Resolving the main model reports the provider as unsupported.
        return Ok(None);
    }
    Err(HarnessError::NoWebFetchModel {
        provider: provider_name.to_owned(),
    })
}

/// The small Bedrock summarizer id for a Bedrock main model, following its family: Haiku for
/// Anthropic, the OpenAI small model (keeping any cross-region prefix) for OpenAI, else `None`.
fn bedrock_web_fetch_model(name: &str) -> Option<String> {
    let family = bedrock_family(name);
    if family.starts_with("anthropic.") {
        return lookup(&WEB_FETCH_MODELS, "bedrock").map(str::to_owned);
    }
    if family.starts_with("openai.") {
        let prefix = name.strip_suffix(family).unwrap_or_default();
        return lookup(&WEB_FETCH_MODELS, "openai").map(|small| format!("{prefix}openai.{small}"));
    }
    None
}
