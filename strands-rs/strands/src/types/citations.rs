//! Citations content block (TS `types/citations.ts`).

use serde::{Deserialize, Serialize};

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
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// Where the citation points.
    pub location: CitationLocation,
    /// The source identifier.
    pub source: String,
    /// The cited source text.
    pub source_content: Vec<CitationSourceContent>,
    /// The source title.
    pub title: String,
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
        }
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
}
