//! Guard content, media blocks, citations and `includeToolResultStatus` formatting.

use serde_json::json;

use super::*;
use crate::models::bedrock::IncludeToolResultStatus;
use crate::types::citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
use crate::types::media::{
    AudioBlock, AudioSource, DocumentBlock, DocumentSource, ImageBlock, ImageSource, S3Location, VideoBlock,
    VideoSource,
};
use crate::types::messages::{
    GuardContentBlock, GuardContentImage, GuardContentText, GuardImageFormat, GuardImageSource,
    GuardQualifier, ToolResultBlock, ToolResultContent,
};
use crate::types::tools::ToolResultStatus;

fn messages_of(messages: Vec<Message>) -> JsonValue {
    let (model, transport) = model(BedrockModelConfig::default());
    warnings(|| sent(&model, &transport, request(messages))).0["messages"].clone()
}

fn tool_result_message(content: Vec<ToolResultContent>) -> Vec<Message> {
    vec![user(vec![
        ToolResultBlock::new("tool-1", ToolResultStatus::Success, content).into(),
    ])]
}

fn tool_result_json(content: &JsonValue) -> JsonValue {
    json!([{ "role": "user", "content": [{ "toolResult": { "toolUseId": "tool-1", "content": content, "status": "success" } }] }])
}

mod guard_content_in_messages {
    use super::*;

    #[test]
    fn formats_guard_content_with_text_in_message() {
        let guard = GuardContentBlock::from(GuardContentText::new(
            "The capital of France is Paris.",
            vec![GuardQualifier::GroundingSource],
        ));
        assert_eq!(
            messages_of(vec![user(vec![text("Verify this information:"), guard.into()])]),
            json!([{ "role": "user", "content": [
                { "text": "Verify this information:" },
                { "guardContent": { "text": { "text": "The capital of France is Paris.", "qualifiers": ["grounding_source"] } } },
            ] }])
        );
    }

    #[test]
    fn formats_guard_content_with_image_in_message() {
        let guard = GuardContentBlock::from(GuardContentImage::new(
            GuardImageFormat::Jpeg,
            GuardImageSource::new(vec![1, 2, 3, 4]),
        ));
        assert_eq!(
            messages_of(vec![user(vec![text("Is this image safe?"), guard.into()])]),
            json!([{ "role": "user", "content": [
                { "text": "Is this image safe?" },
                { "guardContent": { "image": { "format": "jpeg", "source": { "bytes": "AQIDBA==" } } } },
            ] }])
        );
    }
}

mod media_blocks_in_tool_results {
    use super::*;

    #[test]
    fn formats_image_block_in_tool_result() {
        let image = ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(
            messages_of(tool_result_message(vec![image.into()])),
            tool_result_json(&json!([{ "image": { "format": "png", "source": { "bytes": "AQID" } } }]))
        );
    }

    #[test]
    fn formats_video_block_in_tool_result_with_3gp_format_mapping() {
        let video = VideoBlock::new("3gp", VideoSource::Bytes(vec![4, 5, 6]));
        assert_eq!(
            messages_of(tool_result_message(vec![video.into()])),
            tool_result_json(&json!([{ "video": { "format": "three_gp", "source": { "bytes": "BAUG" } } }]))
        );
    }

    #[test]
    fn formats_document_block_in_tool_result() {
        let document = DocumentBlock::new("report.pdf", "pdf", DocumentSource::Bytes(vec![7, 8, 9]));
        assert_eq!(
            messages_of(tool_result_message(vec![document.into()])),
            tool_result_json(
                &json!([{ "document": { "name": "report.pdf", "format": "pdf", "source": { "bytes": "BwgJ" } } }])
            )
        );
    }

    #[test]
    fn formats_mixed_text_and_media_content_in_tool_result() {
        let image = ImageBlock::new("jpeg", ImageSource::Bytes(vec![1, 2]));
        assert_eq!(
            messages_of(tool_result_message(vec![
                "Here is the image:".into(),
                image.into()
            ])),
            tool_result_json(&json!([
                { "text": "Here is the image:" },
                { "image": { "format": "jpeg", "source": { "bytes": "AQI=" } } },
            ]))
        );
    }
}

mod media_blocks_in_messages {
    use super::*;

    fn single(block: ContentBlock) -> JsonValue {
        messages_of(vec![user(vec![block])])[0]["content"].clone()
    }

    #[test]
    fn formats_top_level_audio_block() {
        assert_eq!(
            single(AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3])).into()),
            json!([{ "audio": { "format": "mp3", "source": { "bytes": "AQID" } } }])
        );
    }

    #[test]
    fn formats_top_level_audio_block_with_s3_source() {
        let source = AudioSource::Location(S3Location::new("s3://bucket/audio.wav"));
        assert_eq!(
            single(AudioBlock::new("wav", source).into()),
            json!([{ "audio": { "format": "wav", "source": { "s3Location": { "uri": "s3://bucket/audio.wav" } } } }])
        );
    }

    #[test]
    fn formats_top_level_image_block() {
        assert_eq!(
            single(ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3])).into()),
            json!([{ "image": { "format": "png", "source": { "bytes": "AQID" } } }])
        );
    }

    #[test]
    fn formats_top_level_image_block_with_s3_source() {
        let source = ImageSource::Location(S3Location::new("s3://bucket/image.png"));
        assert_eq!(
            single(ImageBlock::new("png", source).into()),
            json!([{ "image": { "format": "png", "source": { "s3Location": { "uri": "s3://bucket/image.png" } } } }])
        );
    }

    #[test]
    fn formats_an_s3_image_url_as_an_s3_location_and_drops_other_urls() {
        assert_eq!(
            single(ImageBlock::new("png", ImageSource::Url("s3://bucket/a.png".into())).into()),
            json!([{ "image": { "format": "png", "source": { "s3Location": { "uri": "s3://bucket/a.png" } } } }])
        );
        let (model, transport) = model(BedrockModelConfig::default());
        let messages = vec![user(vec![
            ImageBlock::new("png", ImageSource::Url("https://x/a.png".into())).into(),
        ])];
        let (sent, warned) = warnings(|| sent(&model, &transport, request(messages)));
        assert_eq!(
            sent["messages"][0]["content"],
            json!([{ "image": { "format": "png" } }])
        );
        assert_eq!(
            warned,
            ["source_type=<imageSourceUrl> | not supported by bedrock | skipping"]
        );
    }

    #[test]
    fn formats_top_level_video_block_with_3gp_format_mapping() {
        assert_eq!(
            single(VideoBlock::new("3gp", VideoSource::Bytes(vec![4, 5, 6])).into()),
            json!([{ "video": { "format": "three_gp", "source": { "bytes": "BAUG" } } }])
        );
    }

    #[test]
    fn formats_top_level_document_block_with_text_source_converted_to_bytes() {
        let document = DocumentBlock::new("notes.txt", "txt", DocumentSource::Text("Hello world".into()));
        assert_eq!(
            single(document.into()),
            json!([{ "document": {
                "name": "notes.txt",
                "format": "txt",
                "source": { "bytes": crate::types::media::encode_base64(b"Hello world") },
            } }])
        );
    }
}

mod citations_content_block_formatting {
    use super::*;

    fn citation(location: CitationLocation, source: &str, content: &str, title: &str) -> Citation {
        Citation::new(location, source, vec![CitationSourceContent::new(content)], title)
    }

    #[test]
    fn maps_sdk_citation_location_types_to_bedrock_object_key_format_through_formatting_pipeline() {
        let block = CitationsBlock::new(
            vec![
                citation(
                    CitationLocation::document_char(0, 150, 300),
                    "doc-0",
                    "char source",
                    "Text Document",
                ),
                citation(
                    CitationLocation::document_page(0, 2, 3),
                    "doc-0",
                    "page source",
                    "PDF Document",
                ),
                citation(
                    CitationLocation::document_chunk(1, 5, 8),
                    "doc-1",
                    "chunk source",
                    "Chunked Document",
                ),
                citation(
                    CitationLocation::search_result(0, 25, 150),
                    "search-0",
                    "search source",
                    "Search Result",
                ),
                citation(
                    CitationLocation::web_with_domain("https://example.com/doc", "example.com"),
                    "web-0",
                    "web source",
                    "Web Page",
                ),
            ],
            vec![CitationGeneratedContent::new(
                "generated text with all citation types",
            )],
        );
        let messages = vec![assistant(vec![block.into()]), user(vec![text("Follow up")])];
        assert_eq!(
            messages_of(messages),
            json!([
                { "role": "assistant", "content": [{ "citationsContent": {
                    "citations": [
                        { "location": { "documentChar": { "documentIndex": 0, "start": 150, "end": 300 } }, "sourceContent": [{ "text": "char source" }], "source": "doc-0", "title": "Text Document" },
                        { "location": { "documentPage": { "documentIndex": 0, "start": 2, "end": 3 } }, "sourceContent": [{ "text": "page source" }], "source": "doc-0", "title": "PDF Document" },
                        { "location": { "documentChunk": { "documentIndex": 1, "start": 5, "end": 8 } }, "sourceContent": [{ "text": "chunk source" }], "source": "doc-1", "title": "Chunked Document" },
                        { "location": { "searchResultLocation": { "searchResultIndex": 0, "start": 25, "end": 150 } }, "sourceContent": [{ "text": "search source" }], "source": "search-0", "title": "Search Result" },
                        { "location": { "web": { "url": "https://example.com/doc", "domain": "example.com" } }, "sourceContent": [{ "text": "web source" }], "source": "web-0", "title": "Web Page" },
                    ],
                    "content": [{ "text": "generated text with all citation types" }],
                } }] },
                { "role": "user", "content": [{ "text": "Follow up" }] },
            ])
        );
    }

    #[test]
    fn omits_an_empty_source_when_replaying_a_citation() {
        let block = CitationsBlock::new(
            vec![citation(
                CitationLocation::document_page(0, 1, 2),
                "",
                "page source",
                "letter",
            )],
            vec![CitationGeneratedContent::new("summary")],
        );
        let messages = vec![assistant(vec![block.into()]), user(vec![text("Follow up")])];
        assert_eq!(
            messages_of(messages)[0]["content"][0]["citationsContent"]["citations"][0],
            json!({
                "location": { "documentPage": { "documentIndex": 0, "start": 1, "end": 2 } },
                "sourceContent": [{ "text": "page source" }],
                "title": "letter",
            })
        );
    }
}

mod include_tool_result_status_configuration {
    use super::*;

    fn tool_result_sent(config: BedrockModelConfig, status: ToolResultStatus) -> JsonValue {
        let (model, transport) = model(config);
        let messages = vec![user(vec![
            ToolResultBlock::new("tool-123", status, vec!["Result".into()]).into(),
        ])];
        let sent = warnings(|| sent(&model, &transport, request(messages))).0;
        assert_eq!(sent["toolConfig"], noop_tool_config());
        sent["messages"][0]["content"][0]["toolResult"].clone()
    }

    #[test]
    fn always_includes_status_field_in_tool_results() {
        let config = BedrockModelConfig {
            include_tool_result_status: Some(IncludeToolResultStatus::Enabled(true)),
            ..Default::default()
        };
        assert_eq!(
            tool_result_sent(config, ToolResultStatus::Success),
            json!({ "toolUseId": "tool-123", "content": [{ "text": "Result" }], "status": "success" })
        );
    }

    #[test]
    fn never_includes_status_field_in_tool_results() {
        let config = BedrockModelConfig {
            include_tool_result_status: Some(IncludeToolResultStatus::Enabled(false)),
            ..Default::default()
        };
        assert_eq!(
            tool_result_sent(config, ToolResultStatus::Success),
            json!({ "toolUseId": "tool-123", "content": [{ "text": "Result" }] })
        );
    }

    #[test]
    fn includes_status_field_for_claude_models() {
        let config = BedrockModelConfig {
            include_tool_result_status: Some(IncludeToolResultStatus::Auto),
            ..config("anthropic.claude-3-5-sonnet-20241022-v2:0")
        };
        assert_eq!(
            tool_result_sent(config, ToolResultStatus::Success)["status"],
            "success"
        );
    }

    #[test]
    fn includes_status_field_for_amazon_nova_models() {
        let config = BedrockModelConfig {
            include_tool_result_status: Some(IncludeToolResultStatus::Auto),
            ..config("us.amazon.nova-lite-v1:0")
        };
        assert_eq!(
            tool_result_sent(config, ToolResultStatus::Error)["status"],
            "error"
        );
    }

    #[test]
    fn omits_status_field_for_models_bedrock_does_not_document_as_supporting_it() {
        for model_id in [
            "us.writer.palmyra-x5-v1:0",
            "us.meta.llama4-scout-17b-instruct-v1:0",
        ] {
            let sent = tool_result_sent(config(model_id), ToolResultStatus::Success);
            assert!(sent.get("status").is_none(), "{model_id}");
        }
    }
}
