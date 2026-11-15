//! Citations content block (TS `types/citations.ts`).

use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::types::json::JsonValue;

/// Where a citation points (TS `CitationLocation`), written with a `type` tag.
///
/// The union is open, and TS keeps a saved location without checking it: a location with an
/// unknown `type`, or with a known `type` whose fields don't fit that variant (a missing `end`, a
/// fractional `start`, an extra key, a `null` `domain`), loads as [`CitationLocation::Other`] and
/// is written back unchanged. A known variant writes `type` first, then its fields in the order
/// declared here.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CitationLocation {
    /// A character range in a document.
    #[non_exhaustive]
    DocumentChar {
        /// The index of the cited document.
        document_index: u64,
        /// The start of the range.
        start: u64,
        /// The end of the range.
        end: u64,
    },
    /// A page range in a document.
    #[non_exhaustive]
    DocumentPage {
        /// The index of the cited document.
        document_index: u64,
        /// The first page.
        start: u64,
        /// The last page.
        end: u64,
    },
    /// A chunk range in a document.
    #[non_exhaustive]
    DocumentChunk {
        /// The index of the cited document.
        document_index: u64,
        /// The first chunk.
        start: u64,
        /// The last chunk.
        end: u64,
    },
    /// A range in a search result.
    #[non_exhaustive]
    SearchResult {
        /// The index of the cited search result.
        search_result_index: u64,
        /// The start of the range.
        start: u64,
        /// The end of the range.
        end: u64,
    },
    /// A web page.
    #[non_exhaustive]
    Web {
        /// The page URL.
        url: String,
        /// The page domain.
        #[serde(
            default,
            deserialize_with = "present_string",
            skip_serializing_if = "Option::is_none"
        )]
        domain: Option<String>,
    },
    /// A location this version doesn't know, or a known `type` with fields that don't fit it, kept
    /// as written.
    #[serde(untagged)]
    Other(UnknownCitationLocation),
}

impl CitationLocation {
    /// A character range in a document.
    #[must_use]
    pub fn document_char(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentChar {
            document_index,
            start,
            end,
        }
    }

    /// A page range in a document.
    #[must_use]
    pub fn document_page(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentPage {
            document_index,
            start,
            end,
        }
    }

    /// A chunk range in a document.
    #[must_use]
    pub fn document_chunk(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentChunk {
            document_index,
            start,
            end,
        }
    }

    /// A range in a search result.
    #[must_use]
    pub fn search_result(search_result_index: u64, start: u64, end: u64) -> Self {
        Self::SearchResult {
            search_result_index,
            start,
            end,
        }
    }

    /// A web page, without a domain.
    #[must_use]
    pub fn web(url: impl Into<String>) -> Self {
        Self::Web {
            url: url.into(),
            domain: None,
        }
    }

    /// Sets the domain of a [`CitationLocation::Web`] location; other locations are returned unchanged.
    #[must_use]
    pub fn with_domain(self, domain: impl Into<String>) -> Self {
        match self {
            Self::Web { url, .. } => Self::Web {
                url,
                domain: Some(domain.into()),
            },
            other => other,
        }
    }
}

/// Reads an optional string that, when present, must be a string: a `null` fails, so the location
/// falls through to [`CitationLocation::Other`] and keeps the `null`.
fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

/// A [`CitationLocation`] this version can't read as a known variant: the raw JSON object, `type`
/// included. Its `type` may be a known tag when the other fields don't fit that variant.
///
/// It is only built by deserialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnknownCitationLocation(serde_json::Map<String, JsonValue>);

impl UnknownCitationLocation {
    /// Returns the location object as written.
    #[must_use]
    pub fn as_json(&self) -> &serde_json::Map<String, JsonValue> {
        &self.0
    }
}

/// Source text a citation refers to (TS `CitationSourceContent`).
///
/// TS keeps a saved item as read, so a loaded item is written back with its keys in the order they
/// were read, an unknown key in its place, and a `text` read as `null` as `null` while it is unset.
/// Equality compares `text` and the unknown keys, and ignores key order and whether an unset `text`
/// was read as `null` or missing.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CitationSourceContent {
    /// The source text.
    pub text: Option<String>,
    entries: Vec<Entry>,
}

impl CitationSourceContent {
    /// Creates source content.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            entries: Vec::new(),
        }
    }
}

/// Generated text a citation supports (TS `CitationGeneratedContent`).
///
/// A loaded item keeps its key order, unknown keys and `null` `text` as
/// [`CitationSourceContent`] does, and compares the same way.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CitationGeneratedContent {
    /// The generated text.
    pub text: Option<String>,
    entries: Vec<Entry>,
}

impl CitationGeneratedContent {
    /// Creates generated content.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
            entries: Vec::new(),
        }
    }
}

/// One citation (TS `Citation`).
///
/// TS keeps a saved citation as read, and its providers build citations in different key orders.
/// So a loaded citation is written back with its keys in the order they were read, an unknown key in
/// its place, and a field read as `null` as `null` while it is unset; a missing field stays missing.
/// A new citation, and any field set after loading that wasn't read, writes in the order `location`,
/// `source`, `sourceContent`, `title`. Equality compares the fields and the unknown keys, and ignores
/// key order and whether an unset field was read as `null` or missing.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Citation {
    /// Where the citation points.
    pub location: Option<CitationLocation>,
    /// The source identifier.
    pub source: Option<String>,
    /// The cited source text.
    pub source_content: Option<Vec<CitationSourceContent>>,
    /// The source title.
    pub title: Option<String>,
    entries: Vec<Entry>,
}

impl Citation {
    /// Creates a citation.
    #[must_use]
    pub fn new(
        location: CitationLocation,
        source: impl Into<String>,
        source_content: Vec<CitationSourceContent>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            location: Some(location),
            source: Some(source.into()),
            source_content: Some(source_content),
            title: Some(title.into()),
            entries: Vec::new(),
        }
    }
}

impl PartialEq for Citation {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
            && self.source == other.source
            && self.source_content == other.source_content
            && self.title == other.title
            && same_extras(&self.entries, &other.entries)
    }
}

impl PartialEq for CitationSourceContent {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && same_extras(&self.entries, &other.entries)
    }
}

impl Eq for CitationSourceContent {}

impl PartialEq for CitationGeneratedContent {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && same_extras(&self.entries, &other.entries)
    }
}

impl Eq for CitationGeneratedContent {}

/// One key of a loaded object, in the order it was read.
#[derive(Debug, Clone)]
enum Entry {
    /// A known field, written from its typed value when that is set.
    Field(&'static str),
    /// A known field read as `null`, written as `null` while its typed value is unset.
    Null(&'static str),
    /// A key this version doesn't know, written back as read.
    Extra(String, JsonValue),
}

impl Entry {
    fn key(&self) -> &str {
        match self {
            Self::Field(key) | Self::Null(key) => key,
            Self::Extra(key, _) => key,
        }
    }
}

fn same_extras(left: &[Entry], right: &[Entry]) -> bool {
    sorted_extras(left) == sorted_extras(right)
}

fn sorted_extras(entries: &[Entry]) -> Vec<(&str, &JsonValue)> {
    let mut extras: Vec<(&str, &JsonValue)> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Extra(key, value) => Some((key.as_str(), value)),
            Entry::Field(_) | Entry::Null(_) => None,
        })
        .collect();
    extras.sort_by_key(|(key, _)| *key);
    extras
}

/// A JSON object with known optional fields that keeps the key order, `null` fields and unknown keys
/// it was read with.
trait OrderedObject: Sized {
    /// The known keys, in the order a new value writes them.
    const KEYS: &'static [&'static str];

    fn unset() -> Self;

    fn entries(&self) -> &[Entry];

    fn set_entries(&mut self, entries: Vec<Entry>);

    /// Writes the field when it is set and returns whether it was.
    fn write_field<M: SerializeMap>(&self, key: &str, map: &mut M) -> Result<bool, M::Error>;

    /// Reads the field's value and returns whether it was set (not `null`).
    fn read_field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error>;
}

fn write_set<M: SerializeMap, T: Serialize>(
    map: &mut M,
    key: &str,
    value: Option<&T>,
) -> Result<bool, M::Error> {
    let Some(value) = value else {
        return Ok(false);
    };
    map.serialize_entry(key, value)?;
    Ok(true)
}

fn read_set<'de, A: MapAccess<'de>, T: Deserialize<'de>>(
    map: &mut A,
    slot: &mut Option<T>,
) -> Result<bool, A::Error> {
    *slot = map.next_value()?;
    Ok(slot.is_some())
}

impl OrderedObject for Citation {
    const KEYS: &'static [&'static str] = &["location", "source", "sourceContent", "title"];

    fn unset() -> Self {
        Self {
            location: None,
            source: None,
            source_content: None,
            title: None,
            entries: Vec::new(),
        }
    }

    fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = entries;
    }

    fn write_field<M: SerializeMap>(&self, key: &str, map: &mut M) -> Result<bool, M::Error> {
        match key {
            "location" => write_set(map, key, self.location.as_ref()),
            "source" => write_set(map, key, self.source.as_ref()),
            "sourceContent" => write_set(map, key, self.source_content.as_ref()),
            _ => write_set(map, key, self.title.as_ref()),
        }
    }

    fn read_field<'de, A: MapAccess<'de>>(&mut self, key: &str, map: &mut A) -> Result<bool, A::Error> {
        match key {
            "location" => read_set(map, &mut self.location),
            "source" => read_set(map, &mut self.source),
            "sourceContent" => read_set(map, &mut self.source_content),
            _ => read_set(map, &mut self.title),
        }
    }
}

macro_rules! text_item {
    ($ty:ident) => {
        impl OrderedObject for $ty {
            const KEYS: &'static [&'static str] = &["text"];

            fn unset() -> Self {
                Self {
                    text: None,
                    entries: Vec::new(),
                }
            }

            fn entries(&self) -> &[Entry] {
                &self.entries
            }

            fn set_entries(&mut self, entries: Vec<Entry>) {
                self.entries = entries;
            }

            fn write_field<M: SerializeMap>(&self, key: &str, map: &mut M) -> Result<bool, M::Error> {
                write_set(map, key, self.text.as_ref())
            }

            fn read_field<'de, A: MapAccess<'de>>(
                &mut self,
                _key: &str,
                map: &mut A,
            ) -> Result<bool, A::Error> {
                read_set(map, &mut self.text)
            }
        }
    };
}

text_item!(CitationSourceContent);
text_item!(CitationGeneratedContent);

fn serialize_ordered<T: OrderedObject, S: Serializer>(value: &T, serializer: S) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(None)?;
    for entry in value.entries() {
        match entry {
            Entry::Field(key) => {
                value.write_field(key, &mut map)?;
            }
            Entry::Null(key) => {
                if !value.write_field(key, &mut map)? {
                    map.serialize_entry(key, &JsonValue::Null)?;
                }
            }
            Entry::Extra(key, raw) => map.serialize_entry(key, raw)?,
        }
    }
    for key in T::KEYS {
        if !value.entries().iter().any(|entry| entry.key() == *key) {
            value.write_field(key, &mut map)?;
        }
    }
    map.end()
}

struct OrderedVisitor<T>(PhantomData<T>);

impl<'de, T: OrderedObject> Visitor<'de> for OrderedVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<T, A::Error> {
        let mut value = T::unset();
        let mut entries: Vec<Entry> = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            if entries.iter().any(|entry| entry.key() == key) {
                return Err(de::Error::custom(format_args!("duplicate field `{key}`")));
            }
            let entry = match T::KEYS.iter().find(|known| **known == key) {
                Some(known) => {
                    if value.read_field(known, &mut map)? {
                        Entry::Field(known)
                    } else {
                        Entry::Null(known)
                    }
                }
                None => Entry::Extra(key, map.next_value()?),
            };
            entries.push(entry);
        }
        value.set_entries(entries);
        Ok(value)
    }
}

macro_rules! ordered_serde {
    ($ty:ident) => {
        impl Serialize for $ty {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serialize_ordered(self, serializer)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                deserializer.deserialize_map(OrderedVisitor(PhantomData))
            }
        }
    };
}

ordered_serde!(Citation);
ordered_serde!(CitationSourceContent);
ordered_serde!(CitationGeneratedContent);

/// Generated text with its citations (TS `CitationsBlock`), written as
/// `{"citations":{"citations":[..],"content":[..]}}`.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub struct CitationsBlock {
    /// The citations.
    pub citations: Vec<Citation>,
    /// The generated content the citations support.
    pub content: Vec<CitationGeneratedContent>,
}

impl CitationsBlock {
    /// Creates a citations block.
    #[must_use]
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "CitationsBlock")]
struct CitationsBlockDef {
    citations: Vec<Citation>,
    content: Vec<CitationGeneratedContent>,
}

wrapped_serde!(CitationsBlock, CitationsBlockDef, "citations");

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/types/__tests__/citations.test.ts
    //!
    //! The expected bytes in the `ts_*` regression tests are what TS writes back for the same input:
    //! `JSON.stringify(Message.fromJSON({ role: 'assistant', content: [block] }).toJSON().content[0])`
    //! (or the whole message) on the built `strands-ts/dist`.

    use serde_json::json;

    use super::*;

    fn single_citation_data() -> JsonValue {
        json!({
            "citations": [{
                "location": { "type": "documentChar", "documentIndex": 0, "start": 10, "end": 50 },
                "source": "doc-0",
                "sourceContent": [{ "text": "source text from document" }],
                "title": "Test Document"
            }],
            "content": [{ "text": "generated text with citation" }]
        })
    }

    fn all_variants_data() -> JsonValue {
        json!({
            "citations": [
                {
                    "location": { "type": "documentChar", "documentIndex": 0, "start": 150, "end": 300 },
                    "source": "doc-0", "sourceContent": [{ "text": "char source" }], "title": "Text Document"
                },
                {
                    "location": { "type": "documentPage", "documentIndex": 0, "start": 2, "end": 3 },
                    "source": "doc-0", "sourceContent": [{ "text": "page source" }], "title": "PDF Document"
                },
                {
                    "location": { "type": "documentChunk", "documentIndex": 1, "start": 5, "end": 8 },
                    "source": "doc-1", "sourceContent": [{ "text": "chunk source" }], "title": "Chunked Document"
                },
                {
                    "location": { "type": "searchResult", "searchResultIndex": 0, "start": 25, "end": 150 },
                    "source": "search-0", "sourceContent": [{ "text": "search source" }], "title": "Search Result"
                },
                {
                    "location": { "type": "web", "url": "https://example.com/doc", "domain": "example.com" },
                    "source": "web-0",
                    "sourceContent": [{ "text": "web source" }, { "text": "additional source" }],
                    "title": "Web Page"
                }
            ],
            "content": [{ "text": "first generated" }, { "text": "second generated" }]
        })
    }

    fn block_from(data: JsonValue) -> CitationsBlock {
        let mut wrapped = serde_json::Map::new();
        wrapped.insert("citations".into(), data);
        serde_json::from_value(JsonValue::Object(wrapped)).unwrap()
    }

    #[test]
    fn stores_citations_and_content() {
        let block = block_from(single_citation_data());
        assert_eq!(
            block.citations,
            vec![Citation::new(
                CitationLocation::DocumentChar {
                    document_index: 0,
                    start: 10,
                    end: 50
                },
                "doc-0",
                vec![CitationSourceContent::new("source text from document")],
                "Test Document",
            )]
        );
        assert_eq!(
            block.content,
            vec![CitationGeneratedContent::new("generated text with citation")]
        );
    }

    #[test]
    fn round_trips_all_citation_location_variants_multiple_citations_and_multiple_content_blocks() {
        let original = block_from(all_variants_data());
        let restored: CitationsBlock =
            serde_json::from_value(serde_json::to_value(&original).unwrap()).unwrap();
        assert_eq!(restored, original);
        assert_eq!(restored.citations.len(), 5);
        assert!(matches!(
            restored.citations[0].location,
            Some(CitationLocation::DocumentChar { .. })
        ));
        assert!(matches!(
            restored.citations[1].location,
            Some(CitationLocation::DocumentPage { .. })
        ));
        assert!(matches!(
            restored.citations[2].location,
            Some(CitationLocation::DocumentChunk { .. })
        ));
        assert!(matches!(
            restored.citations[3].location,
            Some(CitationLocation::SearchResult { .. })
        ));
        let Some(CitationLocation::Web { domain, .. }) = &restored.citations[4].location else {
            panic!("expected a web location")
        };
        assert_eq!(domain.as_deref(), Some("example.com"));
    }

    #[test]
    fn handles_empty_arrays() {
        let block = CitationsBlock::new(vec![], vec![]);
        let value = serde_json::to_value(&block).unwrap();
        assert_eq!(value, json!({ "citations": { "citations": [], "content": [] } }));
        assert_eq!(serde_json::from_value::<CitationsBlock>(value).unwrap(), block);
    }

    #[test]
    fn to_json_returns_wrapped_format() {
        let block = block_from(single_citation_data());
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "citations": single_citation_data() })
        );
    }

    #[test]
    fn works_with_json_stringify() {
        let original = block_from(all_variants_data());
        let text = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<CitationsBlock>(&text).unwrap(), original);
        assert_eq!(
            serde_json::from_str::<JsonValue>(&text).unwrap(),
            json!({ "citations": all_variants_data() })
        );
    }

    #[test]
    fn unknown_location_types_are_kept_as_written() {
        let location = json!({ "type": "futureLocation", "x": 1 });
        let parsed: CitationLocation = serde_json::from_value(location.clone()).unwrap();
        let CitationLocation::Other(unknown) = &parsed else {
            panic!("expected an unknown location")
        };
        assert_eq!(unknown.as_json().get("x"), Some(&json!(1)));
        assert_eq!(serde_json::to_value(&parsed).unwrap(), location);
    }

    #[test]
    fn a_loaded_citation_is_written_back_in_its_key_order() {
        let citation = json!({
            "source": "doc-0",
            "title": "Text Document",
            "sourceContent": [{ "text": "char source" }],
            "location": { "type": "documentChar", "documentIndex": 0, "start": 1, "end": 2 }
        });
        let parsed: Citation = serde_json::from_str(&citation.to_string()).unwrap();
        assert_eq!(serde_json::to_string(&parsed).unwrap(), citation.to_string());
    }

    #[test]
    fn a_new_citation_writes_location_source_source_content_title() {
        let citation = Citation::new(
            CitationLocation::Web {
                url: "https://example.com".into(),
                domain: None,
            },
            "web-0",
            vec![],
            "Web Page",
        );
        assert_eq!(
            serde_json::to_string(&citation).unwrap(),
            r#"{"location":{"type":"web","url":"https://example.com"},"source":"web-0","sourceContent":[],"title":"Web Page"}"#
        );
    }

    #[test]
    fn citation_equality_ignores_key_order() {
        let loaded: Citation = serde_json::from_value(json!({
            "title": "t", "source": "s", "sourceContent": [],
            "location": { "type": "web", "url": "u" }
        }))
        .unwrap();
        let built = Citation::new(
            CitationLocation::Web {
                url: "u".into(),
                domain: None,
            },
            "s",
            vec![],
            "t",
        );
        assert_eq!(loaded, built);
    }

    #[test]
    fn a_citation_with_a_duplicate_key_fails_to_load() {
        let duplicate = r#"{"source":"a","source":"b","title":"t","sourceContent":[],"location":{"type":"web","url":"u"}}"#;
        assert!(
            serde_json::from_str::<Citation>(duplicate)
                .unwrap_err()
                .to_string()
                .starts_with("duplicate field `source`")
        );
    }

    #[test]
    fn a_known_location_type_with_fields_that_dont_fit_is_kept_as_written() {
        for location in [
            json!({ "type": "documentChar", "documentIndex": 0, "start": 1 }),
            json!({ "type": "documentChar", "documentIndex": 0, "start": 1.5, "end": 2 }),
        ] {
            let parsed: CitationLocation = serde_json::from_str(&location.to_string()).unwrap();
            assert!(matches!(parsed, CitationLocation::Other(_)), "{location}");
            assert_eq!(serde_json::to_string(&parsed).unwrap(), location.to_string());
        }
    }

    fn assert_block_round_trips_as_ts_writes(ts_written: &str) {
        let block: CitationsBlock = serde_json::from_str(ts_written).unwrap();
        assert_eq!(crate::types::json::to_string(&block).unwrap(), ts_written);
    }

    #[test]
    fn ts_a_known_location_with_an_extra_key_or_a_null_domain_is_kept_as_written() {
        let ts_written = r#"{"citations":{"citations":[{"location":{"type":"web","url":"u","extra":1},"source":"s","sourceContent":[],"title":"t"},{"location":{"type":"documentChar","documentIndex":0,"start":1,"end":2,"note":"x"},"source":"s","sourceContent":[],"title":"t"},{"location":{"type":"web","url":"u","domain":null},"source":"s","sourceContent":[],"title":"t"}],"content":[]}}"#;
        let block: CitationsBlock = serde_json::from_str(ts_written).unwrap();
        for citation in &block.citations {
            assert!(matches!(citation.location, Some(CitationLocation::Other(_))));
        }
        assert_block_round_trips_as_ts_writes(ts_written);
    }

    #[test]
    fn ts_unknown_citation_and_item_keys_are_kept_in_place() {
        let ts_written = r#"{"citations":{"citations":[{"meta":{"k":1},"location":{"type":"web","url":"u"},"source":"s","extra":[1,2],"sourceContent":[{"text":"a","page":3}],"title":"t","z":null}],"content":[{"cite":0,"text":"g"}]}}"#;
        let block: CitationsBlock = serde_json::from_str(ts_written).unwrap();
        assert_eq!(block.citations[0].location, Some(CitationLocation::web("u")));
        assert_eq!(block.citations[0].title.as_deref(), Some("t"));
        assert_eq!(block.content[0].text.as_deref(), Some("g"));
        assert_block_round_trips_as_ts_writes(ts_written);
    }

    #[test]
    fn ts_null_and_missing_citation_fields_are_kept_as_written() {
        let ts_written = r#"{"citations":{"citations":[{"location":null,"source":"s","sourceContent":null,"title":null},{"location":{"type":"web","url":"u"}}],"content":[{"text":null},{}]}}"#;
        let block: CitationsBlock = serde_json::from_str(ts_written).unwrap();
        assert_eq!(block.citations[0].title, None);
        assert_eq!(block.citations[1].source, None);
        assert_eq!(block.content[1].text, None);
        assert_block_round_trips_as_ts_writes(ts_written);
    }

    #[test]
    fn ts_a_message_with_unfitting_citation_data_loads_and_is_written_back_unchanged() {
        let ts_written = r#"{"role":"assistant","content":[{"citations":{"citations":[{"source":"s","title":null,"location":{"type":"web","url":"u","extra":1},"sourceContent":[{}]}],"content":[{"text":null}]}}],"trackingId":"00000000-0000-4000-8000-000000000001"}"#;
        let message: crate::types::messages::Message = serde_json::from_str(ts_written).unwrap();
        assert_eq!(crate::types::json::to_string(&message).unwrap(), ts_written);
    }

    #[test]
    fn a_field_set_after_loading_is_written_in_place_of_its_null_or_after_the_read_keys() {
        let mut citation: Citation =
            serde_json::from_str(r#"{"title":null,"x":1,"location":{"type":"web","url":"u"}}"#).unwrap();
        citation.title = Some("t".into());
        citation.source = Some("s".into());
        citation.location = None;
        assert_eq!(
            serde_json::to_string(&citation).unwrap(),
            r#"{"title":"t","x":1,"source":"s"}"#
        );
    }

    #[test]
    fn equality_compares_unknown_keys_and_ignores_their_order_and_null_versus_missing() {
        let parse = |text: &str| serde_json::from_str::<Citation>(text).unwrap();
        assert_eq!(parse(r#"{"a":1,"b":2,"title":null}"#), parse(r#"{"b":2,"a":1}"#));
        assert_ne!(parse(r#"{"a":1}"#), parse(r#"{"a":2}"#));
        assert_ne!(parse(r#"{"a":1}"#), parse("{}"));
    }

    #[test]
    fn a_known_location_is_written_with_type_first_and_its_fields_in_declared_order() {
        let block: CitationsBlock = serde_json::from_str(
            r#"{"citations":{"citations":[{"location":{"url":"u","type":"web"}}],"content":[]}}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_string(&block).unwrap(),
            r#"{"citations":{"citations":[{"location":{"type":"web","url":"u"}}],"content":[]}}"#
        );
    }
}
