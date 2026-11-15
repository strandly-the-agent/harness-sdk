use super::*;

#[test]
fn should_retry_follows_the_sdk_status_rules() {
    let none = HeaderMap::new();
    for status in [408, 409, 429, 500, 502, 529] {
        assert!(should_retry(status, &none), "{status}");
    }
    for status in [400, 401, 403, 404, 413, 422] {
        assert!(!should_retry(status, &none), "{status}");
    }
    let mut headers = HeaderMap::new();
    headers.insert("x-should-retry", HeaderValue::from_static("false"));
    assert!(!should_retry(429, &headers));
    headers.insert("x-should-retry", HeaderValue::from_static("true"));
    assert!(should_retry(400, &headers));
}

#[test]
fn backoff_doubles_up_to_eight_seconds_with_up_to_a_quarter_of_jitter() {
    assert_eq!(default_retry_delay(2, 2, 0.0), Duration::from_millis(500));
    assert_eq!(default_retry_delay(1, 2, 0.0), Duration::from_secs(1));
    assert_eq!(default_retry_delay(0, 10, 0.0), Duration::from_secs(8));
    assert_eq!(default_retry_delay(2, 2, 1.0), Duration::from_millis(375));
}

#[test]
fn parse_js_float_reads_the_leading_number_like_javascript() {
    assert_eq!(parse_js_float(" 1.5s"), Some(1.5));
    assert_eq!(parse_js_float("2e3"), Some(2000.0));
    assert_eq!(parse_js_float("-1"), Some(-1.0));
    assert_eq!(parse_js_float("soon"), None);
    assert_eq!(parse_js_float("1e999"), None);
}

#[test]
fn merge_headers_replaces_same_named_headers() {
    let mut base = HeaderMap::new();
    base.insert("x-one", HeaderValue::from_static("base"));
    base.insert("x-two", HeaderValue::from_static("kept"));
    let mut overrides = HeaderMap::new();
    overrides.append("x-one", HeaderValue::from_static("a"));
    overrides.append("x-one", HeaderValue::from_static("b"));
    let merged = merge_headers(base, &overrides);
    let ones: Vec<_> = merged.get_all("x-one").iter().collect();
    assert_eq!(ones, ["a", "b"]);
    assert_eq!(merged.get("x-two").unwrap(), "kept");
}
