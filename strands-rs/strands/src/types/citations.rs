//! Citation types (TS `types/citations.ts`).

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::json::{JsonObject, JsonValue};

/// Where a citation points (TS `CitationLocation`), tagged by `type` on the wire.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CitationLocation {
    /// A character range in a document (`documentChar`).
    #[non_exhaustive]
    DocumentChar {
        /// Index of the cited document.
        document_index: u64,
        /// Start offset.
        start: u64,
        /// End offset.
        end: u64,
    },
    /// A page range in a document (`documentPage`).
    #[non_exhaustive]
    DocumentPage {
        /// Index of the cited document.
        document_index: u64,
        /// Start page.
        start: u64,
        /// End page.
        end: u64,
    },
    /// A chunk range in a document (`documentChunk`).
    #[non_exhaustive]
    DocumentChunk {
        /// Index of the cited document.
        document_index: u64,
        /// Start chunk.
        start: u64,
        /// End chunk.
        end: u64,
    },
    /// A range in a search result (`searchResult`).
    #[non_exhaustive]
    SearchResult {
        /// Index of the cited search result.
        search_result_index: u64,
        /// Start offset.
        start: u64,
        /// End offset.
        end: u64,
    },
    /// A web page (`web`).
    #[non_exhaustive]
    Web {
        /// The page URL.
        url: String,
        /// The page domain.
        domain: Option<String>,
    },
    /// A location type this SDK version doesn't know, kept verbatim.
    Other(UnknownCitationLocation),
}

/// A [`CitationLocation`] this SDK version can't read: the raw JSON object of an unknown `type`, or
/// of a known `type` with missing or invalid fields.
///
/// Built only by deserialization, so it never holds a location that reads as a known variant.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownCitationLocation(JsonObject);

impl UnknownCitationLocation {
    /// The raw location object.
    pub fn as_object(&self) -> &JsonObject {
        &self.0
    }

    /// The raw `type` value, when it is a string.
    pub fn location_type(&self) -> Option<&str> {
        self.0.get("type").and_then(JsonValue::as_str)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum KnownLocation {
    DocumentChar {
        document_index: u64,
        start: u64,
        end: u64,
    },
    DocumentPage {
        document_index: u64,
        start: u64,
        end: u64,
    },
    DocumentChunk {
        document_index: u64,
        start: u64,
        end: u64,
    },
    SearchResult {
        search_result_index: u64,
        start: u64,
        end: u64,
    },
    Web {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
    },
}

const KNOWN_LOCATION_TYPES: [&str; 5] = [
    "documentChar",
    "documentPage",
    "documentChunk",
    "searchResult",
    "web",
];

impl Serialize for CitationLocation {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let known = match self.clone() {
            Self::DocumentChar {
                document_index,
                start,
                end,
            } => KnownLocation::DocumentChar {
                document_index,
                start,
                end,
            },
            Self::DocumentPage {
                document_index,
                start,
                end,
            } => KnownLocation::DocumentPage {
                document_index,
                start,
                end,
            },
            Self::DocumentChunk {
                document_index,
                start,
                end,
            } => KnownLocation::DocumentChunk {
                document_index,
                start,
                end,
            },
            Self::SearchResult {
                search_result_index,
                start,
                end,
            } => KnownLocation::SearchResult {
                search_result_index,
                start,
                end,
            },
            Self::Web { url, domain } => KnownLocation::Web { url, domain },
            Self::Other(other) => return other.0.serialize(serializer),
        };
        known.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CitationLocation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let object = JsonObject::deserialize(deserializer)?;
        let is_known = object
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|t| KNOWN_LOCATION_TYPES.contains(&t));
        if is_known && let Ok(known) = serde_json::from_value(JsonValue::Object(object.clone())) {
            return Ok(Self::from_known(known));
        }
        Ok(Self::Other(UnknownCitationLocation(object)))
    }
}

impl CitationLocation {
    /// A character range in a document.
    pub fn document_char(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentChar {
            document_index,
            start,
            end,
        }
    }

    /// A page range in a document.
    pub fn document_page(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentPage {
            document_index,
            start,
            end,
        }
    }

    /// A chunk range in a document.
    pub fn document_chunk(document_index: u64, start: u64, end: u64) -> Self {
        Self::DocumentChunk {
            document_index,
            start,
            end,
        }
    }

    /// A range in a search result.
    pub fn search_result(search_result_index: u64, start: u64, end: u64) -> Self {
        Self::SearchResult {
            search_result_index,
            start,
            end,
        }
    }

    /// A web page without a domain.
    pub fn web(url: impl Into<String>) -> Self {
        Self::Web {
            url: url.into(),
            domain: None,
        }
    }

    /// A web page with its domain.
    pub fn web_with_domain(url: impl Into<String>, domain: impl Into<String>) -> Self {
        Self::Web {
            url: url.into(),
            domain: Some(domain.into()),
        }
    }

    fn from_known(known: KnownLocation) -> Self {
        match known {
            KnownLocation::DocumentChar {
                document_index,
                start,
                end,
            } => Self::DocumentChar {
                document_index,
                start,
                end,
            },
            KnownLocation::DocumentPage {
                document_index,
                start,
                end,
            } => Self::DocumentPage {
                document_index,
                start,
                end,
            },
            KnownLocation::DocumentChunk {
                document_index,
                start,
                end,
            } => Self::DocumentChunk {
                document_index,
                start,
                end,
            },
            KnownLocation::SearchResult {
                search_result_index,
                start,
                end,
            } => Self::SearchResult {
                search_result_index,
                start,
                end,
            },
            KnownLocation::Web { url, domain } => Self::Web { url, domain },
        }
    }
}

/// Source text a citation quotes (TS `CitationSourceContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationSourceContent {
    text: String,
}

impl CitationSourceContent {
    /// Source content with this text.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The quoted text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Generated text backed by citations (TS `CitationGeneratedContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationGeneratedContent {
    text: String,
}

impl CitationGeneratedContent {
    /// Generated content with this text.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    /// The generated text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// One citation (TS `Citation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Citation {
    location: CitationLocation,
    source: String,
    source_content: Vec<CitationSourceContent>,
    title: String,
}

impl Citation {
    /// A citation.
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
        }
    }

    /// Where the citation points.
    pub fn location(&self) -> &CitationLocation {
        &self.location
    }

    /// The source identifier.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The quoted source content.
    pub fn source_content(&self) -> &[CitationSourceContent] {
        &self.source_content
    }

    /// The source title.
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// A content block of generated text with citations (TS `CitationsBlock`); `ContentBlock` writes it
/// as `{"citations": ..}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationsBlock {
    citations: Vec<Citation>,
    content: Vec<CitationGeneratedContent>,
}

impl CitationsBlock {
    /// A citations block.
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }

    /// The citations.
    pub fn citations(&self) -> &[Citation] {
        &self.citations
    }

    /// The generated content.
    pub fn content(&self) -> &[CitationGeneratedContent] {
        &self.content
    }
}

/// Ported from strands-ts/src/types/__tests__/citations.test.ts
#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::messages::ContentBlock;

    fn every_location() -> Vec<CitationLocation> {
        vec![
            CitationLocation::document_char(0, 0, 10),
            CitationLocation::document_page(1, 2, 3),
            CitationLocation::document_chunk(2, 4, 5),
            CitationLocation::search_result(0, 1, 2),
            CitationLocation::web_with_domain("https://example.com", "example.com"),
            CitationLocation::web("https://example.org"),
        ]
    }

    fn sample() -> CitationsBlock {
        let citations = every_location()
            .into_iter()
            .enumerate()
            .map(|(i, location)| {
                Citation::new(
                    location,
                    format!("src-{i}"),
                    vec![CitationSourceContent::new("quote")],
                    "Title",
                )
            })
            .collect();
        CitationsBlock::new(
            citations,
            vec![
                CitationGeneratedContent::new("a"),
                CitationGeneratedContent::new("b"),
            ],
        )
    }

    mod citations_block {
        use super::*;

        #[test]
        fn creates_block_with_correct_type_discriminator() {
            let block = ContentBlock::from(sample());
            assert!(matches!(block, ContentBlock::Citations(_)));
        }

        #[test]
        fn stores_citations_and_content() {
            let block = sample();
            assert_eq!(block.citations().len(), 6);
            assert_eq!(block.citations()[0].source(), "src-0");
            assert_eq!(block.citations()[0].source_content()[0].text(), "quote");
            assert_eq!(block.citations()[0].title(), "Title");
            assert_eq!(block.content()[1].text(), "b");
        }

        #[test]
        fn round_trips_all_citation_location_variants_multiple_citations_and_multiple_content_blocks() {
            let block = ContentBlock::from(sample());
            let restored: ContentBlock =
                serde_json::from_value(serde_json::to_value(&block).unwrap()).unwrap();
            assert_eq!(restored, block);
        }

        #[test]
        fn handles_empty_arrays() {
            let block = ContentBlock::from(CitationsBlock::new(vec![], vec![]));
            let value = serde_json::to_value(&block).unwrap();
            assert_eq!(value, json!({"citations": {"citations": [], "content": []}}));
            assert_eq!(serde_json::from_value::<ContentBlock>(value).unwrap(), block);
        }

        #[test]
        fn to_json_returns_wrapped_format() {
            let block = CitationsBlock::new(
                vec![Citation::new(
                    CitationLocation::document_char(0, 0, 10),
                    "doc-0",
                    vec![CitationSourceContent::new("source")],
                    "Test",
                )],
                vec![CitationGeneratedContent::new("generated")],
            );
            assert_eq!(
                serde_json::to_value(ContentBlock::from(block)).unwrap(),
                json!({"citations": {
                    "citations": [{
                        "location": {"type": "documentChar", "documentIndex": 0, "start": 0, "end": 10},
                        "source": "doc-0",
                        "sourceContent": [{"text": "source"}],
                        "title": "Test"
                    }],
                    "content": [{"text": "generated"}]
                }})
            );
        }

        #[test]
        fn works_with_json_stringify() {
            let block = ContentBlock::from(sample());
            let text = serde_json::to_string(&block).unwrap();
            assert_eq!(serde_json::from_str::<ContentBlock>(&text).unwrap(), block);
        }
    }

    #[test]
    fn unknown_location_types_round_trip_unchanged() {
        let raw = json!({"type": "videoTimestamp", "seconds": 12, "documentIndex": 0});
        let location: CitationLocation = serde_json::from_value(raw.clone()).unwrap();
        let CitationLocation::Other(other) = &location else {
            panic!("expected Other, got {location:?}")
        };
        assert_eq!(other.location_type(), Some("videoTimestamp"));
        assert_eq!(serde_json::to_value(&location).unwrap(), raw);
    }

    #[test]
    fn a_known_location_type_with_missing_or_invalid_fields_stays_other() {
        for raw in [
            json!({"type": "documentChar", "start": 0, "end": 5}),
            json!({"type": "documentChar", "documentIndex": 0, "start": "x", "end": 5}),
        ] {
            let location: CitationLocation = serde_json::from_value(raw.clone()).unwrap();
            let CitationLocation::Other(other) = &location else {
                panic!("expected Other, got {location:?}")
            };
            assert_eq!(other.location_type(), Some("documentChar"));
            assert_eq!(serde_json::to_value(&location).unwrap(), raw);
        }
    }
}
