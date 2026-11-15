//! Tests for the request, config and cache types of strands-ts/src/models/model.ts.

use super::*;

#[test]
fn merge_replaces_only_the_fields_the_update_sets() {
    let mut config = BaseModelConfig::builder()
        .model_id("first")
        .max_tokens(100)
        .temperature(0.5)
        .build();
    config.merge(BaseModelConfig {
        max_tokens: Some(200),
        top_p: Some(0.9),
        ..Default::default()
    });
    assert_eq!(
        config,
        BaseModelConfig {
            model_id: Some("first".into()),
            max_tokens: Some(200),
            temperature: Some(0.5),
            top_p: Some(0.9),
            ..Default::default()
        }
    );
}

#[test]
fn a_request_builds_with_defaults() {
    let request = ModelRequest::builder().system_prompt("You help.").build();
    assert!(request.messages.is_empty() && request.tool_specs.is_empty());
    assert_eq!(request.system_prompt, Some(SystemPrompt::from("You help.")));
    assert!(request.model_state.keys().is_empty());
    assert!(request.cancel_signal.is_none());
}

mod resolve_cache_section {
    use super::*;

    #[test]
    fn a_missing_section_is_enabled_with_the_fallback_ttl() {
        let hour = CacheTtl::OneHour;
        assert_eq!(
            resolve_cache_section(None, &[Some(&hour)]),
            ResolvedCacheSection {
                enabled: true,
                ttl: Some(CacheTtl::OneHour)
            }
        );
    }

    #[test]
    fn false_disables_and_true_keeps_the_fallback_ttl() {
        let hour = CacheTtl::OneHour;
        assert!(!resolve_cache_section(Some(&CacheSection::Enabled(false)), &[Some(&hour)]).enabled);
        assert_eq!(
            resolve_cache_section(Some(&CacheSection::Enabled(true)), &[None, Some(&hour)]).ttl,
            Some(CacheTtl::OneHour)
        );
    }

    #[test]
    fn the_section_ttl_wins_over_fallbacks_and_empty_ttls_are_skipped() {
        let hour = CacheTtl::OneHour;
        let section = CacheSection::Ttl(CacheTtl::FiveMinutes);
        assert_eq!(
            resolve_cache_section(Some(&section), &[Some(&hour)]).ttl,
            Some(CacheTtl::FiveMinutes)
        );
        let empty = CacheTtl::from("");
        assert_eq!(
            resolve_cache_section(Some(&CacheSection::Ttl(empty)), &[None]),
            ResolvedCacheSection {
                enabled: true,
                ttl: None
            }
        );
    }

    #[test]
    fn unknown_ttls_are_kept() {
        let ttl = CacheTtl::from("24h");
        assert_eq!(ttl.as_str(), "24h");
        assert!(matches!(ttl, CacheTtl::Other(_)));
    }
}
