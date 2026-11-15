//! Citations content block (TS `types/citations.ts`).

use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::types::json::JsonValue;

/// Where a citation points (TS `CitationLocation`), written with a `type` tag.
///
/// The union is open: a location with an unknown `type` loads as [`CitationLocation::Other`] and is
/// written back unchanged.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CitationLocation {
    /// A character range in a document.
    DocumentChar {
        /// The index of the cited document.
        document_index: u64,
        /// The start of the range.
        start: u64,
        /// The end of the range.
        end: u64,
    },
    /// A page range in a document.
    DocumentPage {
        /// The index of the cited document.
        document_index: u64,
        /// The first page.
        start: u64,
        /// The last page.
        end: u64,
    },
    /// A chunk range in a document.
    DocumentChunk {
        /// The index of the cited document.
        document_index: u64,
        /// The first chunk.
        start: u64,
        /// The last chunk.
        end: u64,
    },
    /// A range in a search result.
    SearchResult {
        /// The index of the cited search result.
        search_result_index: u64,
        /// The start of the range.
        start: u64,
        /// The end of the range.
        end: u64,
    },
    /// A web page.
    Web {
        /// The page URL.
        url: String,
        /// The page domain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
    },
    /// A location this version doesn't know, kept as written.
    #[serde(untagged)]
    Other(UnknownCitationLocation),
}

/// A [`CitationLocation`] this version doesn't know: the raw JSON object, `type` included.
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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationSourceContent {
    /// The source text.
    pub text: String,
}

impl CitationSourceContent {
    /// Creates source content.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Generated text a citation supports (TS `CitationGeneratedContent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationGeneratedContent {
    /// The generated text.
    pub text: String,
}

impl CitationGeneratedContent {
    /// Creates generated content.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// One citation (TS `Citation`).
///
/// A loaded citation is written back with its keys in the order they were read, because TS writes
/// citations in the order its provider built them. A new citation writes `location`, `source`,
/// `sourceContent`, `title`. Equality ignores that key order.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Citation {
    /// Where the citation points.
    pub location: CitationLocation,
    /// The source identifier.
    pub source: String,
    /// The cited source text.
    pub source_content: Vec<CitationSourceContent>,
    /// The source title.
    pub title: String,
    key_order: [CitationKey; 4],
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
            location,
            source: source.into(),
            source_content,
            title: title.into(),
            key_order: [
                CitationKey::Location,
                CitationKey::Source,
                CitationKey::SourceContent,
                CitationKey::Title,
            ],
        }
    }
}

impl PartialEq for Citation {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
            && self.source == other.source
            && self.source_content == other.source_content
            && self.title == other.title
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(field_identifier, rename_all = "camelCase")]
enum CitationKey {
    Location,
    Source,
    SourceContent,
    Title,
    #[serde(other)]
    Ignored,
}

impl Serialize for Citation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("Citation", 4)?;
        for key in self.key_order {
            match key {
                CitationKey::Location => state.serialize_field("location", &self.location)?,
                CitationKey::Source => state.serialize_field("source", &self.source)?,
                CitationKey::SourceContent => {
                    state.serialize_field("sourceContent", &self.source_content)?;
                }
                CitationKey::Title => state.serialize_field("title", &self.title)?,
                CitationKey::Ignored => {}
            }
        }
        state.end()
    }
}

impl<'de> Deserialize<'de> for Citation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(CitationVisitor)
    }
}

struct CitationVisitor;

impl<'de> Visitor<'de> for CitationVisitor {
    type Value = Citation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a citation object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Citation, A::Error> {
        let mut location = None;
        let mut source: Option<String> = None;
        let mut source_content = None;
        let mut title: Option<String> = None;
        let mut keys = Vec::with_capacity(4);
        while let Some(key) = map.next_key::<CitationKey>()? {
            if keys.contains(&key) {
                return Err(de::Error::duplicate_field(key_name(key)));
            }
            match key {
                CitationKey::Location => location = Some(map.next_value()?),
                CitationKey::Source => source = Some(map.next_value()?),
                CitationKey::SourceContent => source_content = Some(map.next_value()?),
                CitationKey::Title => title = Some(map.next_value()?),
                CitationKey::Ignored => {
                    map.next_value::<de::IgnoredAny>()?;
                    continue;
                }
            }
            keys.push(key);
        }
        let citation = Citation::new(
            location.ok_or_else(|| de::Error::missing_field("location"))?,
            source.ok_or_else(|| de::Error::missing_field("source"))?,
            source_content.ok_or_else(|| de::Error::missing_field("sourceContent"))?,
            title.ok_or_else(|| de::Error::missing_field("title"))?,
        );
        let key_order = <[CitationKey; 4]>::try_from(keys)
            .map_err(|keys| de::Error::invalid_length(keys.len(), &"four citation fields"))?;
        Ok(Citation {
            key_order,
            ..citation
        })
    }
}

fn key_name(key: CitationKey) -> &'static str {
    match key {
        CitationKey::Location => "location",
        CitationKey::Source => "source",
        CitationKey::SourceContent => "sourceContent",
        CitationKey::Title => "title",
        CitationKey::Ignored => "",
    }
}

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
            CitationLocation::DocumentChar { .. }
        ));
        assert!(matches!(
            restored.citations[1].location,
            CitationLocation::DocumentPage { .. }
        ));
        assert!(matches!(
            restored.citations[2].location,
            CitationLocation::DocumentChunk { .. }
        ));
        assert!(matches!(
            restored.citations[3].location,
            CitationLocation::SearchResult { .. }
        ));
        let CitationLocation::Web { domain, .. } = &restored.citations[4].location else {
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
    fn a_citation_with_a_duplicate_or_missing_key_fails_to_load() {
        let duplicate = r#"{"source":"a","source":"b","title":"t","sourceContent":[],"location":{"type":"web","url":"u"}}"#;
        assert!(
            serde_json::from_str::<Citation>(duplicate)
                .unwrap_err()
                .to_string()
                .starts_with("duplicate field `source`")
        );
        let missing = r#"{"source":"a","sourceContent":[],"location":{"type":"web","url":"u"}}"#;
        assert!(
            serde_json::from_str::<Citation>(missing)
                .unwrap_err()
                .to_string()
                .starts_with("missing field `title`")
        );
    }
}
