//! Bedrock Mantle: an OpenAI-compatible Bedrock endpoint (`strands-ts/src/models/openai/mantle.ts`).

use crate::error::{Error, Result};

/// Model id prefixes served under the `/openai/v1` base path.
const OPENAI_BASE_PATH_PREFIXES: &[&str] = &[
    "openai.gpt-5.",
    "openai.gpt-6-",
    "openai.gpt-6.",
    "xai.grok-4.",
    "google.gemma-4-",
];

/// Configuration for talking to Bedrock Mantle instead of api.openai.com.
#[derive(Clone, Debug, Default)]
pub struct BedrockMantleConfig {
    /// AWS region; falls back to `AWS_REGION` / `AWS_DEFAULT_REGION`.
    pub region: Option<String>,
    /// Lifetime requested for the minted bearer token.
    pub expires_in_seconds: Option<u64>,
}

/// `/openai/v1` for the OpenAI-shaped model families, `/v1` otherwise.
pub fn resolve_mantle_base_path(model_id: &str) -> &'static str {
    if OPENAI_BASE_PATH_PREFIXES
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
    {
        "/openai/v1"
    } else {
        "/v1"
    }
}

/// `https://bedrock-mantle.<region>.api.aws<base path>`, validating the region.
pub fn resolve_mantle_base_url(config: &BedrockMantleConfig, model_id: &str) -> Result<String> {
    let region = resolve_region(config)?;
    Ok(format!(
        "https://bedrock-mantle.{region}.api.aws{}",
        resolve_mantle_base_path(model_id)
    ))
}

fn resolve_region(config: &BedrockMantleConfig) -> Result<String> {
    let region = config
        .region
        .clone()
        .or_else(|| std::env::var("AWS_REGION").ok())
        .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
        .ok_or_else(|| {
            Error::model(
                "could not resolve an AWS region for Bedrock Mantle. Set 'region' in bedrockMantleConfig or the AWS_REGION environment variable.",
            )
        })?;
    if !is_valid_region(&region) {
        return Err(Error::model(format!("invalid AWS region: '{region}'")));
    }
    Ok(region)
}

/// Anchored equivalent of the TS `/^[a-z]{2}(-[a-z]+)+-[0-9]+$/` region check.
fn is_valid_region(region: &str) -> bool {
    let mut parts = region.split('-');
    let Some(first) = parts.next() else {
        return false;
    };
    if first.len() != 2 || !first.chars().all(|c| c.is_ascii_lowercase()) {
        return false;
    }
    let rest: Vec<&str> = parts.collect();
    if rest.len() < 2 {
        return false;
    }
    let (last, middle) = rest.split_last().expect("rest is non-empty");
    if last.is_empty() || !last.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    middle
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_lowercase()))
}

/// Mint a Mantle bearer token.
///
/// Gap vs TS: the TS provider lazily imports `@aws/bedrock-token-generator` and mints a fresh
/// short-lived token per request. No Rust equivalent exists yet, so the token must be supplied
/// out of band via `api_key` or `AWS_BEARER_TOKEN_BEDROCK`.
pub(crate) fn mint_bearer_token() -> Result<Option<String>> {
    std::env::var("AWS_BEARER_TOKEN_BEDROCK").map(Some).map_err(|_| {
        Error::model(
            "failed to mint Bedrock Mantle bearer token | token minting is not implemented in Rust yet: set AWS_BEARER_TOKEN_BEDROCK or pass api_key explicitly",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_openai_base_path_for_known_prefixes() {
        for id in [
            "openai.gpt-5.4",
            "openai.gpt-6-preview",
            "openai.gpt-6.1",
            "xai.grok-4.1",
            "google.gemma-4-12b",
        ] {
            assert_eq!(resolve_mantle_base_path(id), "/openai/v1", "{id}");
        }
    }

    #[test]
    fn falls_back_to_v1_base_path() {
        for id in ["anthropic.claude-sonnet-4-6", "openai.gpt-4.1", "meta.llama4"] {
            assert_eq!(resolve_mantle_base_path(id), "/v1", "{id}");
        }
    }

    #[test]
    fn builds_base_url_from_region_and_model() {
        let config = BedrockMantleConfig {
            region: Some("us-west-2".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_mantle_base_url(&config, "openai.gpt-5.4").unwrap(),
            "https://bedrock-mantle.us-west-2.api.aws/openai/v1"
        );
        assert_eq!(
            resolve_mantle_base_url(&config, "anthropic.claude-sonnet-4-6").unwrap(),
            "https://bedrock-mantle.us-west-2.api.aws/v1"
        );
    }

    #[test]
    fn rejects_invalid_regions() {
        for region in [
            "US-WEST-2",
            "us-west",
            "uswest2",
            "us-west-2/../evil",
            "us-west-2.example.com",
            "u-west-2",
        ] {
            let config = BedrockMantleConfig {
                region: Some(region.into()),
                ..Default::default()
            };
            let err = resolve_mantle_base_url(&config, "openai.gpt-5.4").unwrap_err();
            assert_eq!(err.to_string(), format!("invalid AWS region: '{region}'"));
        }
    }

    #[test]
    fn accepts_multi_segment_regions() {
        assert!(is_valid_region("ap-southeast-1"));
        assert!(is_valid_region("us-gov-west-1"));
    }
}
