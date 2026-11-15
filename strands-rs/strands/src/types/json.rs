//! JSON value types shared by the wire structs (TS `types/json.ts`).
//!
//! TS `deepCopy` and `deepCopyWithValidation` guard against JavaScript values JSON can't carry
//! (functions, symbols, `undefined`, cycles). A [`JsonValue`] can only hold JSON, and `Clone` is a
//! deep copy, so neither helper has a Rust counterpart.
//!
//! Parsing uses `serde_json`'s default recursion limit of 128 nested arrays or objects; deeper input
//! fails to parse instead of overflowing the stack. JSON text with a lone UTF-16 surrogate escape
//! (`"\ud83d"`) fails to parse into a Rust `String`; load paths that must accept such TS-written data
//! first pass the text through the crate-internal `replace_lone_surrogates`.
//!
//! [`to_string`] and [`to_string_pretty`] write the bytes JavaScript `JSON.stringify` writes, so
//! data TS saved is written back unchanged; plain `serde_json` differs from JavaScript in how it
//! writes some floats (`1e-6` for `0.000001`, `-0.0`, `1.2345678901234567e+20`).

use std::borrow::Cow;
use std::io;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::ser::{CompactFormatter, Formatter, PrettyFormatter};

/// A JSON value (TS `JSONValue`).
pub type JsonValue = serde_json::Value;

/// Deserializes a field where TS distinguishes a present `null` from a missing key: a present `null`
/// becomes `Some(JsonValue::Null)`; pair with `#[serde(default)]` so a missing key stays `None`.
pub(crate) fn present_null<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    JsonValue::deserialize(deserializer).map(Some)
}

/// Deserializes a field TS fills with a default when the saved value is missing or `null`
/// (`value ?? default`); pair with `#[serde(default)]`.
pub(crate) fn null_as_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    deserializer: D,
) -> Result<T, D::Error> {
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Rewrites every `\uXXXX` escape of a UTF-16 surrogate that isn't part of a valid pair to `\ufffd`,
/// so `serde_json` can parse JSON text that JavaScript wrote from a string with a lone surrogate.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the storage and session load paths")
)]
pub(crate) fn replace_lone_surrogates(json: &str) -> Cow<'_, str> {
    if !json.contains("\\u") {
        return Cow::Borrowed(json);
    }
    let bytes = json.as_bytes();
    let mut out = String::with_capacity(json.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let Some(unit) = escape_unit(bytes, i) else {
            // Skip the escaped character so `\\ud83d` (an escaped backslash) isn't read as an escape.
            i += 2;
            continue;
        };
        let is_high = (0xD800..0xDC00).contains(&unit);
        let is_low = (0xDC00..0xE000).contains(&unit);
        if is_high && escape_unit(bytes, i + 6).is_some_and(|next| (0xDC00..0xE000).contains(&next)) {
            i += 12;
            continue;
        }
        if is_high || is_low {
            out.push_str(&json[copied..i]);
            out.push_str("\\ufffd");
            copied = i + 6;
        }
        i += 6;
    }
    if copied == 0 {
        return Cow::Borrowed(json);
    }
    out.push_str(&json[copied..]);
    Cow::Owned(out)
}

/// Returns the code unit of a `\uXXXX` escape starting at `at`, if there is one.
fn escape_unit(bytes: &[u8], at: usize) -> Option<u32> {
    let escape = bytes.get(at..at + 6)?;
    if escape[0] != b'\\' || escape[1] != b'u' {
        return None;
    }
    let hex = std::str::from_utf8(&escape[2..]).ok()?;
    u32::from_str_radix(hex, 16).ok()
}

/// `keys` in JavaScript `Object.keys` order: array-index keys ascending, then the rest in their
/// given order.
pub(crate) fn js_key_order<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_by_key(|key| array_index(key).map_or((1, 0), |index| (0, index)));
    keys
}

/// The index JavaScript reads from `key` when it is an array index: the canonical decimal form of an
/// integer from 0 to 4294967294.
fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || !key.starts_with('0');
    if !canonical || !key.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|index| *index < u32::MAX)
}

/// Serializes `value` to the bytes JavaScript `JSON.stringify(value)` writes for the same JSON.
///
/// `serde_json` already escapes strings as JavaScript does; this also writes non-integer numbers
/// the JavaScript way (`0.000001`, not `1e-6`; `1e+21`; `-0` as `0`), so data TS wrote is saved back
/// byte for byte.
///
/// ```
/// let value = serde_json::json!({ "small": 0.000001, "big": 1e21, "count": 3 });
/// assert_eq!(strands::types::json::to_string(&value).unwrap(), r#"{"small":0.000001,"big":1e+21,"count":3}"#);
/// ```
///
/// # Errors
///
/// Fails if `value`'s `Serialize` implementation fails or a map has a non-string key.
pub fn to_string<T: ?Sized + Serialize>(value: &T) -> crate::Result<String> {
    write_js(value, JsFormatter(CompactFormatter))
}

/// Serializes `value` like JavaScript `JSON.stringify(value, null, 2)`: two-space indentation and
/// the number formatting of [`to_string`].
///
/// ```
/// let value = serde_json::json!({ "list": [1, 0.5], "empty": {} });
/// assert_eq!(
///     strands::types::json::to_string_pretty(&value).unwrap(),
///     "{\n  \"list\": [\n    1,\n    0.5\n  ],\n  \"empty\": {}\n}"
/// );
/// ```
///
/// # Errors
///
/// Fails if `value`'s `Serialize` implementation fails or a map has a non-string key.
pub fn to_string_pretty<T: ?Sized + Serialize>(value: &T) -> crate::Result<String> {
    write_js(value, JsFormatter(PrettyFormatter::with_indent(b"  ")))
}

fn write_js<T: ?Sized + Serialize, F: Formatter>(value: &T, formatter: F) -> crate::Result<String> {
    let mut out = Vec::new();
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, formatter))?;
    String::from_utf8(out).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error).into())
}

/// Wraps a `serde_json` formatter and writes floats as JavaScript `Number.prototype.toString` does.
struct JsFormatter<F>(F);

impl<F: Formatter> Formatter for JsFormatter<F> {
    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(js_number(&format!("{value:e}")).as_bytes())
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(&format!("{value:e}")).as_bytes())
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array(writer)
    }

    fn begin_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_array_value(writer, first)
    }

    fn end_array_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_array_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object(writer)
    }

    fn end_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object(writer)
    }

    fn begin_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W, first: bool) -> io::Result<()> {
        self.0.begin_object_key(writer, first)
    }

    fn end_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_key(writer)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.0.end_object_value(writer)
    }
}

/// Rewrites Rust's shortest exponential float form (`1.5e-7`, `-0e0`) as JavaScript
/// `Number.prototype.toString` (ECMA-262 `Number::toString`): positional notation for decimal
/// exponents from -7 to 20, exponential with an explicit sign otherwise, and `0` for both zeros.
fn js_number(exponential: &str) -> String {
    let (sign, unsigned) = match exponential.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", exponential),
    };
    let Some((mantissa, exponent)) = unsigned.split_once('e') else {
        return exponential.to_owned();
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return exponential.to_owned();
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if digits == "0" {
        return "0".to_owned();
    }
    let zeros = |count: i32| "0".repeat(usize::try_from(count).unwrap_or(0));
    // ECMA-262 `n` (position of the decimal point) and `k` (number of significant digits).
    let point = exponent + 1;
    let length = i32::try_from(digits.len()).unwrap_or(i32::MAX);
    let body = if length <= point && point <= 21 {
        format!("{digits}{}", zeros(point - length))
    } else if 0 < point && point <= 21 {
        let (integer, fraction) = digits.split_at(usize::try_from(point).unwrap_or(0));
        format!("{integer}.{fraction}")
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", zeros(-point))
    } else {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let exponent_sign = if exponent < 0 { '-' } else { '+' };
        format!("{first}{fraction}e{exponent_sign}{}", exponent.unsigned_abs())
    };
    format!("{sign}{body}")
}

#[cfg(test)]
mod tests {
    //! Lone surrogate and recursion-limit behaviour (no TS spec file; see the module docs).

    use serde_json::json;

    use super::*;
    use crate::types::messages::{ContentBlock, Message};

    #[test]
    fn lone_surrogates_fail_to_parse_without_the_rewrite() {
        let text = r#"{"text":"a\ud83db"}"#;
        assert!(serde_json::from_str::<JsonValue>(text).is_err());
    }

    #[test]
    fn replace_lone_surrogates_rewrites_lone_high_and_low_surrogates() {
        let text = r#"{"a":"a\ud83db","b":"\ude00","c":"\ud83d"}"#;
        let fixed = replace_lone_surrogates(text);
        let value: JsonValue = serde_json::from_str(&fixed).unwrap();
        assert_eq!(
            value,
            json!({ "a": "a\u{fffd}b", "b": "\u{fffd}", "c": "\u{fffd}" })
        );
    }

    #[test]
    fn replace_lone_surrogates_keeps_valid_pairs_and_escaped_backslashes() {
        let text = r#"{"pair":"\ud83d\ude00","literal":"\\ud83d","plain":"\u00e9"}"#;
        assert!(matches!(replace_lone_surrogates(text), Cow::Borrowed(_)));
        let value: JsonValue = serde_json::from_str(text).unwrap();
        assert_eq!(value, json!({ "pair": "😀", "literal": "\\ud83d", "plain": "é" }));
    }

    #[test]
    fn surrogate_pairs_round_trip_in_messages() {
        let value = json!({ "role": "user", "content": [{ "text": "emoji 😀" }], "trackingId": "t" });
        let message: Message = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&message).unwrap(), value);
    }

    #[test]
    fn numbers_are_written_as_javascript_writes_them() {
        // Expected text is Node's `JSON.stringify` of the same array.
        let input = "[-0,0.1,1e+21,1.5e-7,0.000001,0.0000015,123456789012345670000,1e300,5e-324,100.5,1e16,\
            12345678901234567.5,-1.5e-10,18446744073709551616,1e-7,123e-20,-1e21,0.30000000000000004,\
            1.7976931348623157e308,27670116110564327000,-42]";
        let expected = "[0,0.1,1e+21,1.5e-7,0.000001,0.0000015,123456789012345670000,1e+300,5e-324,100.5,\
            10000000000000000,12345678901234568,-1.5e-10,18446744073709552000,1e-7,1.23e-18,-1e+21,\
            0.30000000000000004,1.7976931348623157e+308,27670116110564327000,-42]";
        let value: JsonValue = serde_json::from_str(input).unwrap();
        assert_eq!(to_string(&value).unwrap(), expected);

        // Integers keep every digit; JS would round this one to 9007199254740992.
        let exact: JsonValue = serde_json::from_str("9007199254740993").unwrap();
        assert_eq!(to_string(&exact).unwrap(), "9007199254740993");
    }

    #[test]
    fn floats_outside_json_values_use_the_same_formatting() {
        assert_eq!(
            to_string(&[-0.0_f64, 1e-6, 2.5e25]).unwrap(),
            "[0,0.000001,2.5e+25]"
        );
        assert_eq!(to_string(&[0.1_f32, 1e-6_f32]).unwrap(), "[0.1,0.000001]");
        assert_eq!(to_string(&f64::NAN).unwrap(), "null");
    }

    #[test]
    fn pretty_output_matches_json_stringify_with_two_space_indent() {
        let value = json!({ "a": [], "b": {}, "c": [1, { "d": null }], "e": "x" });
        assert_eq!(
            to_string_pretty(&value).unwrap(),
            "{\n  \"a\": [],\n  \"b\": {},\n  \"c\": [\n    1,\n    {\n      \"d\": null\n    }\n  ],\n  \"e\": \"x\"\n}"
        );
    }

    #[test]
    fn json_edge_cases_round_trip_with_lone_surrogates_replaced() {
        let fixture = include_str!("../../../conformance/fixtures/types/json_edge_cases.json");
        assert!(serde_json::from_str::<JsonValue>(fixture).is_err());
        let repaired = replace_lone_surrogates(fixture);
        let cases: indexmap::IndexMap<String, ContentBlock> = serde_json::from_str(&repaired).unwrap();
        assert_eq!(cases["loneHighSurrogate"], ContentBlock::from("a\u{fffd}b"));
        assert_eq!(cases["loneLowSurrogate"], ContentBlock::from("\u{fffd}"));
        assert_eq!(cases["surrogatePair"], ContentBlock::from("emoji 😀"));
        // JS writes the repaired text as a raw U+FFFD; every other byte must match TS.
        let expected = repaired.replace("\\ufffd", "\u{fffd}");
        assert_eq!(to_string_pretty(&cases).unwrap() + "\n", expected);
    }

    #[test]
    fn parsing_stops_at_the_recursion_limit() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(serde_json::from_str::<JsonValue>(&nested(64)).is_ok());
        let error = serde_json::from_str::<JsonValue>(&nested(200)).unwrap_err();
        assert!(error.to_string().contains("recursion limit exceeded"));
    }
}
