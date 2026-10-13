//! Citation blocks (`strands-ts/src/types/citations.ts`).

use serde::{Deserialize, Serialize};

/// Where a citation points. Internally tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CitationLocation {
    /// Character range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentChar {
        /// Index of the document in the request.
        document_index: u64,
        /// Start offset.
        start: u64,
        /// End offset.
        end: u64,
    },
    /// Page range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentPage {
        /// Index of the document in the request.
        document_index: u64,
        /// Start page.
        start: u64,
        /// End page.
        end: u64,
    },
    /// Chunk range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentChunk {
        /// Index of the document in the request.
        document_index: u64,
        /// Start chunk.
        start: u64,
        /// End chunk.
        end: u64,
    },
    /// Range in a search result.
    #[serde(rename_all = "camelCase")]
    SearchResult {
        /// Index of the search result.
        search_result_index: u64,
        /// Start offset.
        start: u64,
        /// End offset.
        end: u64,
    },
    /// Web page.
    Web {
        /// URL.
        url: String,
        /// Domain.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
    },
}

/// Cited source text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CitationSourceContent {
    /// Text.
    pub text: String,
}

/// Generated text annotated by citations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CitationGeneratedContent {
    /// Text.
    pub text: String,
}

/// A single citation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    /// Location in the source.
    pub location: CitationLocation,
    /// Source document name / URI.
    pub source: String,
    /// Cited source text.
    pub source_content: Vec<CitationSourceContent>,
    /// Title.
    pub title: String,
}

/// Citations block: `{"citations": {"citations": [...], "content": [...]}}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CitationsBlock {
    /// Citations.
    pub citations: Vec<Citation>,
    /// Generated text these citations annotate.
    pub content: Vec<CitationGeneratedContent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn location_is_internally_tagged() {
        let loc = CitationLocation::DocumentChar {
            document_index: 0,
            start: 10,
            end: 20,
        };
        assert_eq!(
            serde_json::to_value(&loc).unwrap(),
            json!({"type": "documentChar", "documentIndex": 0, "start": 10, "end": 20})
        );
        let web: CitationLocation =
            serde_json::from_value(json!({"type": "web", "url": "https://x"})).unwrap();
        assert_eq!(
            web,
            CitationLocation::Web {
                url: "https://x".into(),
                domain: None
            }
        );
    }
}
