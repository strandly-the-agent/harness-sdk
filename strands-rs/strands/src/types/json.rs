//! JSON value aliases and the JSON reading and writing rules shared with the TS SDK.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the persisted-data readers and writers use these")
)]

use std::borrow::Cow;
use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::ser::Formatter;

/// Any JSON value (TS `JSONValue`).
pub type JsonValue = serde_json::Value;

/// Serializes `value` as compact JSON with JavaScript number formatting.
///
/// `serde_json` writes floats in Rust's style (`1e20`, `1e-6`, `3.0`); `JSON.stringify` writes
/// `100000000000000000000`, `0.000001` and `3`. Persisted data goes through this so both SDKs write
/// the same bytes.
pub(crate) fn to_vec<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::new();
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, JsFormatter))?;
    Ok(out)
}

/// Parses JSON written by either SDK, replacing lone UTF-16 surrogate escapes with U+FFFD.
///
/// JS strings may hold lone surrogates and `JSON.stringify` escapes them (`"\ud83d"`); a Rust
/// `String` can't hold one, and `serde_json` rejects the whole document. Replacing them keeps the
/// rest of the data loadable.
pub(crate) fn from_slice<T: DeserializeOwned>(bytes: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice(&replace_lone_surrogates(bytes))
}

/// Deserializes a field where TS tells a present `null` apart from a missing one.
///
/// Use with `#[serde(default, deserialize_with = "present")]`: missing is `None`, `null` is
/// `Some(JsonValue::Null)`.
pub(crate) fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<JsonValue>, D::Error> {
    serde::Deserialize::deserialize(deserializer).map(Some)
}

/// Deserializes a field that TS reads with `?? default`: `null` and missing both give the default.
///
/// Use with `#[serde(default, deserialize_with = "null_as_default")]`.
pub(crate) fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Option::unwrap_or_default)
}

struct JsFormatter;

impl Formatter for JsFormatter {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }

    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        self.write_f64(writer, f64::from(value))
    }
}

/// Formats a finite `f64` the way ECMAScript `Number.prototype.toString` does.
pub(crate) fn js_number(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    // `{:e}` gives the shortest round-trip digits, for example `1.5e-7`.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let sign = if value < 0.0 { "-" } else { "" };
    let k = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    let n = exponent.saturating_add(1);
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat(usize::try_from(n - k).unwrap_or(0)))
    } else if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(usize::try_from(n).unwrap_or(0));
        format!("{int}.{frac}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat(usize::try_from(-n).unwrap_or(0)))
    } else {
        let (first, rest) = digits.split_at(1.min(digits.len()));
        let point = if rest.is_empty() { "" } else { "." };
        let exp_sign = if n - 1 < 0 { "-" } else { "+" };
        format!("{first}{point}{rest}e{exp_sign}{}", (n - 1).abs())
    };
    format!("{sign}{body}")
}

fn replace_lone_surrogates(bytes: &[u8]) -> Cow<'_, [u8]> {
    if !bytes
        .windows(3)
        .any(|w| w.first() == Some(&b'\\') && w.get(1) == Some(&b'u'))
    {
        return Cow::Borrowed(bytes);
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut changed = false;
    while let Some(&byte) = bytes.get(i) {
        if byte != b'\\' {
            out.push(byte);
            i += 1;
            continue;
        }
        let Some(unit) = escaped_unit(bytes, i) else {
            // Any other escape: copy the backslash and the escaped byte so `\\u` stays literal.
            out.extend(bytes.get(i..(i + 2).min(bytes.len())).unwrap_or_default());
            i += 2;
            continue;
        };
        let paired = match unit {
            0xD800..=0xDBFF => {
                escaped_unit(bytes, i + 6).is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
            }
            0xDC00..=0xDFFF => false,
            _ => true,
        };
        let len = if paired && (0xD800..=0xDBFF).contains(&unit) {
            12
        } else {
            6
        };
        if paired {
            out.extend(bytes.get(i..i + len).unwrap_or_default());
        } else {
            out.extend_from_slice(b"\\ufffd");
            changed = true;
        }
        i += len;
    }
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(bytes)
    }
}

/// The UTF-16 code unit of a `\uXXXX` escape starting at `at`, if there is one.
fn escaped_unit(bytes: &[u8], at: usize) -> Option<u16> {
    let escape = bytes.get(at..at + 6)?;
    if escape.get(..2)? != b"\\u" {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(escape.get(2..)?).ok()?, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_use_javascript_formatting() {
        let cases = [
            (json!(0.0), "0"),
            (json!(-0.0), "0"),
            (json!(3.0), "3"),
            (json!(1.5), "1.5"),
            (json!(0.1), "0.1"),
            (json!(0.000001), "0.000001"),
            (json!(0.0000001), "1e-7"),
            (json!(1.5e-7), "1.5e-7"),
            (json!(1e20), "100000000000000000000"),
            (json!(1e21), "1e+21"),
            (json!(1.2345e22), "1.2345e+22"),
            (json!(-2.5), "-2.5"),
            (json!(123_456.789), "123456.789"),
            (json!(9_007_199_254_740_991_u64), "9007199254740991"),
            (json!(-7), "-7"),
            (json!(f64::NAN), "null"),
        ];
        for (value, expected) in cases {
            assert_eq!(
                String::from_utf8(to_vec(&value).unwrap()).unwrap(),
                expected,
                "{value}"
            );
        }
    }

    #[test]
    fn lone_surrogates_become_replacement_characters() {
        let input =
            br#"["lone high \ud83d and lone low \ude00","pair \ud83d\ude00","\\ud83d","\ud83d\ud83d\ude00"]"#;
        let parsed: Vec<String> = from_slice(input).unwrap();
        assert_eq!(
            parsed,
            [
                "lone high \u{fffd} and lone low \u{fffd}",
                "pair \u{1F600}",
                "\\ud83d",
                "\u{fffd}\u{1F600}"
            ]
        );
    }

    #[test]
    fn input_without_lone_surrogates_is_parsed_unchanged() {
        let input = br#"{"a":"\u00e9\n"}"#;
        assert!(matches!(replace_lone_surrogates(input), Cow::Borrowed(_)));
        assert_eq!(from_slice::<JsonValue>(input).unwrap(), json!({"a": "\u{e9}\n"}));
    }

    #[test]
    fn nesting_beyond_the_recursion_limit_is_an_error() {
        let deep = format!("{}{}", "[".repeat(200), "]".repeat(200));
        let err = from_slice::<JsonValue>(deep.as_bytes()).unwrap_err();
        assert!(err.to_string().starts_with("recursion limit exceeded"), "{err}");
        let shallow = format!("{}{}", "[".repeat(100), "]".repeat(100));
        assert!(from_slice::<JsonValue>(shallow.as_bytes()).is_ok());
    }
}
