//! Citations returned by models that cite their sources (TS `types/citations.ts`).

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};

use crate::types::json::JsonValue;

/// Where a citation points in its source (TS `CitationLocation`).
///
/// Location types this SDK version doesn't know load as [`CitationLocation::Other`] and are written
/// back unchanged.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[non_exhaustive]
pub enum CitationLocation {
    /// A character range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentChar {
        /// Index of the document in the request.
        document_index: u64,
        /// First character, inclusive.
        start: u64,
        /// Last character, exclusive.
        end: u64,
    },
    /// A page range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentPage {
        /// Index of the document in the request.
        document_index: u64,
        /// First page, inclusive.
        start: u64,
        /// Last page, exclusive.
        end: u64,
    },
    /// A chunk range in a document.
    #[serde(rename_all = "camelCase")]
    DocumentChunk {
        /// Index of the document in the request.
        document_index: u64,
        /// First chunk, inclusive.
        start: u64,
        /// Last chunk, exclusive.
        end: u64,
    },
    /// A range in a search result.
    #[serde(rename_all = "camelCase")]
    SearchResult {
        /// Index of the search result.
        search_result_index: u64,
        /// Start position, inclusive.
        start: u64,
        /// End position, exclusive.
        end: u64,
    },
    /// A web page.
    Web {
        /// The page URL.
        url: String,
        /// The page's domain.
        #[serde(skip_serializing_if = "Option::is_none")]
        domain: Option<String>,
    },
    /// A location type this SDK version doesn't know, kept as written.
    #[serde(untagged)]
    Other(UnknownCitationLocation),
}

/// A [`CitationLocation`] whose `type` matches no known variant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UnknownCitationLocation(serde_json::Map<String, JsonValue>);

impl UnknownCitationLocation {
    /// The location object as written, including its `type`.
    pub fn as_map(&self) -> &serde_json::Map<String, JsonValue> {
        &self.0
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum KnownCitationLocation {
    #[serde(rename_all = "camelCase")]
    DocumentChar {
        document_index: u64,
        start: u64,
        end: u64,
    },
    #[serde(rename_all = "camelCase")]
    DocumentPage {
        document_index: u64,
        start: u64,
        end: u64,
    },
    #[serde(rename_all = "camelCase")]
    DocumentChunk {
        document_index: u64,
        start: u64,
        end: u64,
    },
    #[serde(rename_all = "camelCase")]
    SearchResult {
        search_result_index: u64,
        start: u64,
        end: u64,
    },
    Web {
        url: String,
        #[serde(default)]
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

impl From<KnownCitationLocation> for CitationLocation {
    fn from(known: KnownCitationLocation) -> Self {
        match known {
            KnownCitationLocation::DocumentChar {
                document_index,
                start,
                end,
            } => Self::DocumentChar {
                document_index,
                start,
                end,
            },
            KnownCitationLocation::DocumentPage {
                document_index,
                start,
                end,
            } => Self::DocumentPage {
                document_index,
                start,
                end,
            },
            KnownCitationLocation::DocumentChunk {
                document_index,
                start,
                end,
            } => Self::DocumentChunk {
                document_index,
                start,
                end,
            },
            KnownCitationLocation::SearchResult {
                search_result_index,
                start,
                end,
            } => Self::SearchResult {
                search_result_index,
                start,
                end,
            },
            KnownCitationLocation::Web { url, domain } => Self::Web { url, domain },
        }
    }
}

impl<'de> Deserialize<'de> for CitationLocation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = serde_json::Map::<String, JsonValue>::deserialize(deserializer)?;
        let known = map
            .get("type")
            .and_then(JsonValue::as_str)
            .is_some_and(|t| KNOWN_LOCATION_TYPES.contains(&t));
        if !known {
            return Ok(Self::Other(UnknownCitationLocation(map)));
        }
        KnownCitationLocation::deserialize(JsonValue::Object(map))
            .map(Self::from)
            .map_err(de::Error::custom)
    }
}

/// Source text a citation refers to (TS `CitationSourceContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationSourceContent {
    /// The cited source text.
    pub text: String,
}

impl CitationSourceContent {
    /// Source content with `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Generated text that carries citations (TS `CitationGeneratedContent`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationGeneratedContent {
    /// The generated text.
    pub text: String,
}

impl CitationGeneratedContent {
    /// Generated content with `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// One citation (TS `Citation`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
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
}

/// Generated text with the citations behind it (TS `CitationsBlock`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CitationsBlock {
    /// The citations.
    pub citations: Vec<Citation>,
    /// The generated text.
    pub content: Vec<CitationGeneratedContent>,
}

impl CitationsBlock {
    /// A citations block.
    pub fn new(citations: Vec<Citation>, content: Vec<CitationGeneratedContent>) -> Self {
        Self { citations, content }
    }
}
