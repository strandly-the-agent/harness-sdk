//! Ported from strands-ts/src/types/__tests__/citations.test.ts

use serde_json::json;

use crate::types::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock, ContentBlock,
};

fn single_citation() -> CitationsBlock {
    CitationsBlock::new(
        vec![Citation::new(
            CitationLocation::DocumentChar {
                document_index: 0,
                start: 10,
                end: 50,
            },
            "doc-0",
            vec![CitationSourceContent::new("source text from document")],
            "Test Document",
        )],
        vec![CitationGeneratedContent::new("generated text with citation")],
    )
}

fn all_variants() -> CitationsBlock {
    let citation = |location, source: &str, texts: &[&str], title: &str| {
        Citation::new(
            location,
            source,
            texts.iter().map(|t| CitationSourceContent::new(*t)).collect(),
            title,
        )
    };
    CitationsBlock::new(
        vec![
            citation(
                CitationLocation::DocumentChar {
                    document_index: 0,
                    start: 150,
                    end: 300,
                },
                "doc-0",
                &["char source"],
                "Text Document",
            ),
            citation(
                CitationLocation::DocumentPage {
                    document_index: 0,
                    start: 2,
                    end: 3,
                },
                "doc-0",
                &["page source"],
                "PDF Document",
            ),
            citation(
                CitationLocation::DocumentChunk {
                    document_index: 1,
                    start: 5,
                    end: 8,
                },
                "doc-1",
                &["chunk source"],
                "Chunked Document",
            ),
            citation(
                CitationLocation::SearchResult {
                    search_result_index: 0,
                    start: 25,
                    end: 150,
                },
                "search-0",
                &["search source"],
                "Search Result",
            ),
            citation(
                CitationLocation::Web {
                    url: "https://example.com/doc".into(),
                    domain: Some("example.com".into()),
                },
                "web-0",
                &["web source", "additional source"],
                "Web Page",
            ),
        ],
        vec![
            CitationGeneratedContent::new("first generated"),
            CitationGeneratedContent::new("second generated"),
        ],
    )
}

fn round_trip(block: &CitationsBlock) -> CitationsBlock {
    let json = serde_json::to_value(ContentBlock::from(block.clone())).unwrap();
    match serde_json::from_value(json).unwrap() {
        ContentBlock::Citations(restored) => restored,
        other => panic!("unexpected block {other:?}"),
    }
}

#[test]
fn creates_block_with_correct_type_discriminator() {
    assert!(matches!(
        ContentBlock::from(single_citation()),
        ContentBlock::Citations(_)
    ));
}

#[test]
fn stores_citations_and_content() {
    let block = single_citation();
    assert_eq!(block.citations.len(), 1);
    assert_eq!(block.citations[0].source, "doc-0");
    assert_eq!(
        block.content,
        [CitationGeneratedContent::new("generated text with citation")]
    );
}

#[test]
fn round_trips_all_citation_location_variants_multiple_citations_and_multiple_content_blocks() {
    let original = all_variants();
    let restored = round_trip(&original);
    assert_eq!(restored, original);
    assert_eq!(restored.citations.len(), 5);
    let types: Vec<String> = restored
        .citations
        .iter()
        .map(|c| {
            serde_json::to_value(&c.location).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        types,
        [
            "documentChar",
            "documentPage",
            "documentChunk",
            "searchResult",
            "web"
        ]
    );
    assert!(
        matches!(&restored.citations[4].location, CitationLocation::Web { domain: Some(d), .. } if d == "example.com")
    );
}

#[test]
fn handles_empty_arrays() {
    let block = CitationsBlock::new(vec![], vec![]);
    assert!(block.citations.is_empty() && block.content.is_empty());
    assert_eq!(round_trip(&block), block);
}

#[test]
fn to_json_returns_wrapped_format() {
    let json = serde_json::to_value(ContentBlock::from(single_citation())).unwrap();
    assert_eq!(
        json,
        json!({
            "citations": {
                "citations": [{
                    "location": { "type": "documentChar", "documentIndex": 0, "start": 10, "end": 50 },
                    "source": "doc-0",
                    "sourceContent": [{ "text": "source text from document" }],
                    "title": "Test Document",
                }],
                "content": [{ "text": "generated text with citation" }],
            }
        })
    );
}

#[test]
fn works_with_json_stringify() {
    let original = all_variants();
    let text = serde_json::to_string(&ContentBlock::from(original.clone())).unwrap();
    assert_eq!(
        serde_json::from_str::<ContentBlock>(&text).unwrap(),
        ContentBlock::Citations(original)
    );
}

#[test]
fn unknown_location_types_round_trip_unchanged() {
    let data = json!({ "type": "videoTimestamp", "startMs": 5, "extra": { "a": 1 } });
    let location: CitationLocation = serde_json::from_value(data.clone()).unwrap();
    assert!(matches!(&location, CitationLocation::Other(other) if other.as_map()["startMs"] == 5));
    assert_eq!(serde_json::to_value(&location).unwrap(), data);
}

#[test]
fn web_location_without_domain_omits_it() {
    let location = CitationLocation::Web {
        url: "https://example.com".into(),
        domain: None,
    };
    assert_eq!(
        serde_json::to_string(&location).unwrap(),
        r#"{"type":"web","url":"https://example.com"}"#
    );
}
