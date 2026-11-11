//! Ported from strands-ts/src/types/__tests__/media.test.ts

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::types::media::{decode_base64, encode_base64};
use crate::types::{
    AudioBlock, AudioFormat, AudioSource, CitationsConfig, ContentBlock, DocumentBlock, DocumentFormat,
    DocumentSource, ImageBlock, ImageFormat, ImageSource, S3Location, TextBlock, VideoBlock, VideoFormat,
    VideoSource,
};

fn round_trip<T: Serialize + DeserializeOwned>(value: &T) -> T {
    serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
}

fn location() -> S3Location {
    S3Location::new("s3://my-bucket/file").with_bucket_owner("123456789012")
}

mod s3_location {
    use super::*;

    #[test]
    fn creates_instance_with_uri_only() {
        let location = S3Location::new("s3://my-bucket/image.jpg");
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "s3", "uri": "s3://my-bucket/image.jpg" })
        );
    }

    #[test]
    fn creates_instance_with_uri_and_bucket_owner() {
        let location = S3Location::new("s3://my-bucket/image.jpg").with_bucket_owner("123456789012");
        assert_eq!(
            serde_json::to_value(&location).unwrap(),
            json!({ "type": "s3", "uri": "s3://my-bucket/image.jpg", "bucketOwner": "123456789012" })
        );
    }
}

mod audio_block {
    use super::*;

    #[test]
    fn creates_instance_with_bytes_source() {
        let block = AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, AudioFormat::Mp3);
        assert_eq!(block.source, AudioSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn creates_instance_with_s3_location_source() {
        let block = AudioBlock::new(AudioFormat::Wav, AudioSource::Location(location()));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "format": "wav", "source": { "location": { "type": "s3", "uri": "s3://my-bucket/file", "bucketOwner": "123456789012" } } })
        );
    }

    #[test]
    fn throws_error_for_invalid_source() {
        let err = serde_json::from_value::<AudioBlock>(json!({ "format": "mp3", "source": {} })).unwrap_err();
        assert_eq!(err.to_string(), "Invalid audio source");
    }
}

mod image_block {
    use super::*;

    #[test]
    fn creates_instance_with_bytes_source() {
        let block = ImageBlock::new("jpeg", ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, ImageFormat::Jpeg);
        assert_eq!(block.source, ImageSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn creates_instance_with_s3_location_source() {
        let block = ImageBlock::new("png", ImageSource::Location(location()));
        assert_eq!(block.source, ImageSource::Location(location()));
    }

    #[test]
    fn creates_instance_with_url_source() {
        let block = ImageBlock::new("png", ImageSource::Url("https://example.com/image.png".into()));
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({ "format": "png", "source": { "url": "https://example.com/image.png" } })
        );
    }

    #[test]
    fn throws_error_for_invalid_source() {
        let err = serde_json::from_value::<ImageBlock>(json!({ "format": "png", "source": {} })).unwrap_err();
        assert_eq!(err.to_string(), "Invalid image source");
    }
}

mod video_block {
    use super::*;

    #[test]
    fn creates_instance_with_bytes_source() {
        let block = VideoBlock::new("mp4", VideoSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.format, VideoFormat::Mp4);
        assert_eq!(block.source, VideoSource::Bytes(vec![1, 2, 3]));
    }

    #[test]
    fn creates_instance_with_s3_location_source() {
        let block = VideoBlock::new("mov", VideoSource::Location(location()));
        assert_eq!(block.source, VideoSource::Location(location()));
    }

    #[test]
    fn throws_error_for_invalid_source() {
        let err = serde_json::from_value::<VideoBlock>(json!({ "format": "mp4", "source": {} })).unwrap_err();
        assert_eq!(err.to_string(), "Invalid video source");
    }
}

mod document_block {
    use super::*;

    #[test]
    fn creates_instance_with_bytes_source() {
        let block = DocumentBlock::new("document.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.name, "document.pdf");
        assert_eq!(block.format, DocumentFormat::Pdf);
        assert_eq!(block.source, DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!((block.citations, block.context), (None, None));
    }

    #[test]
    fn creates_instance_with_text_source() {
        let block = DocumentBlock::new("note.txt", "txt", DocumentSource::Text("Hello world".into()));
        assert_eq!(block.source, DocumentSource::Text("Hello world".into()));
    }

    #[test]
    fn creates_instance_with_content_source() {
        let block: DocumentBlock = serde_json::from_value(json!({
            "name": "report.html",
            "format": "html",
            "source": { "content": [{ "text": "Introduction" }, { "text": "Conclusion" }] },
        }))
        .unwrap();
        assert_eq!(
            block.source,
            DocumentSource::Content(vec![TextBlock::new("Introduction"), TextBlock::new("Conclusion")])
        );
    }

    #[test]
    fn creates_instance_with_s3_location_source() {
        let block = DocumentBlock::new("report.pdf", "pdf", DocumentSource::Location(location()));
        assert_eq!(block.source, DocumentSource::Location(location()));
    }

    #[test]
    fn creates_instance_with_bytes_and_filename() {
        let block = DocumentBlock::new("upload.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!(block.name, "upload.pdf");
    }

    #[test]
    fn creates_instance_with_text_and_filename() {
        let block = DocumentBlock::new("note.txt", "txt", DocumentSource::Text("Hello world".into()));
        assert_eq!(block.name, "note.txt");
    }

    #[test]
    fn creates_instance_with_citations_and_context() {
        let block = DocumentBlock::new("research.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]))
            .with_citations(CitationsConfig::new(true))
            .with_context("Research paper about AI");
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            json!({
                "name": "research.pdf",
                "format": "pdf",
                "source": { "bytes": "AQID" },
                "citations": { "enabled": true },
                "context": "Research paper about AI",
            })
        );
    }

    #[test]
    fn throws_error_for_invalid_source() {
        let err = serde_json::from_value::<DocumentBlock>(
            json!({ "name": "doc.pdf", "format": "pdf", "source": {} }),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Invalid document source");
    }
}

mod encode_base64_and_decode_base64 {
    use super::*;

    fn check(original: &[u8]) {
        assert_eq!(decode_base64(&encode_base64(original)), original);
    }

    #[test]
    fn round_trips_empty_array() {
        check(&[]);
    }

    #[test]
    fn round_trips_single_byte() {
        check(&[42]);
    }

    #[test]
    fn round_trips_multi_byte_array() {
        check(&[1, 2, 3, 255, 0, 128]);
    }

    #[test]
    fn round_trips_large_array() {
        let original: Vec<u8> = (0..1000_u32).map(|i| u8::try_from(i % 256).unwrap()).collect();
        check(&original);
    }
}

mod from_json_with_serialized_base64_string_input {
    use super::*;

    const BYTES: [u8; 5] = [1, 2, 3, 4, 5];

    fn parse(key: &str, extra: &serde_json::Value, bytes: &serde_json::Value) -> ContentBlock {
        let mut inner = extra.as_object().unwrap().clone();
        inner.insert("source".into(), json!({ "bytes": bytes }));
        let mut block = serde_json::Map::new();
        block.insert(key.to_owned(), inner.into());
        serde_json::from_value(block.into()).unwrap()
    }

    fn source_bytes(block: &ContentBlock) -> Vec<u8> {
        match block {
            ContentBlock::Audio(AudioBlock {
                source: AudioSource::Bytes(b),
                ..
            })
            | ContentBlock::Image(ImageBlock {
                source: ImageSource::Bytes(b),
                ..
            })
            | ContentBlock::Video(VideoBlock {
                source: VideoSource::Bytes(b),
                ..
            })
            | ContentBlock::Document(DocumentBlock {
                source: DocumentSource::Bytes(b),
                ..
            }) => b.clone(),
            other => panic!("unexpected block {other:?}"),
        }
    }

    fn check(key: &str, extra: &serde_json::Value) {
        let from_string = parse(key, extra, &json!(encode_base64(&BYTES)));
        assert_eq!(source_bytes(&from_string), BYTES);
        let from_array = parse(key, extra, &json!(BYTES));
        assert_eq!(source_bytes(&from_array), BYTES);
    }

    #[test]
    fn audio_block_from_json_accepts_base64_string_for_bytes() {
        check("audio", &json!({ "format": "mp3" }));
    }

    #[test]
    fn audio_block_from_json_accepts_uint8_array_for_bytes() {
        let block = parse(
            "audio",
            &json!({ "format": "mp3" }),
            &json!({ "0": 1, "1": 2, "2": 3, "3": 4, "4": 5 }),
        );
        assert_eq!(source_bytes(&block), BYTES);
    }

    #[test]
    fn image_block_from_json_accepts_base64_string_for_bytes() {
        check("image", &json!({ "format": "png" }));
    }

    #[test]
    fn image_block_from_json_accepts_uint8_array_for_bytes() {
        let block = parse("image", &json!({ "format": "png" }), &json!(BYTES));
        assert_eq!(source_bytes(&block), BYTES);
    }

    #[test]
    fn video_block_from_json_accepts_base64_string_for_bytes() {
        check("video", &json!({ "format": "mp4" }));
    }

    #[test]
    fn video_block_from_json_accepts_uint8_array_for_bytes() {
        let block = parse("video", &json!({ "format": "mp4" }), &json!(BYTES));
        assert_eq!(source_bytes(&block), BYTES);
    }

    #[test]
    fn document_block_from_json_accepts_base64_string_for_bytes() {
        check("document", &json!({ "name": "doc", "format": "pdf" }));
    }

    #[test]
    fn document_block_from_json_accepts_uint8_array_for_bytes() {
        let block = parse(
            "document",
            &json!({ "name": "doc", "format": "pdf" }),
            &json!(BYTES),
        );
        assert_eq!(source_bytes(&block), BYTES);
    }
}

mod s3_location_to_json_from_json {
    use super::*;

    #[test]
    fn round_trips_with_uri_only() {
        let original = S3Location::new("s3://bucket/key.jpg");
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_uri_and_bucket_owner() {
        let original = S3Location::new("s3://bucket/key.jpg").with_bucket_owner("123456789012");
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn includes_type_in_json_output() {
        let json = serde_json::to_string(&S3Location::new("s3://bucket/key.jpg")).unwrap();
        assert_eq!(json, r#"{"type":"s3","uri":"s3://bucket/key.jpg"}"#);
    }
}

mod audio_block_to_json_from_json {
    use super::*;

    #[test]
    fn round_trips_with_bytes_source() {
        let original = AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_s3_location_source() {
        let original = AudioBlock::new("wav", AudioSource::Location(location()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn encodes_bytes_as_base64_in_json_output() {
        let json = serde_json::to_value(AudioBlock::new("mp3", AudioSource::Bytes(vec![1, 2, 3]))).unwrap();
        assert_eq!(json["source"]["bytes"], "AQID");
    }
}

mod image_block_to_json_from_json {
    use super::*;

    #[test]
    fn round_trips_with_bytes_source() {
        let original = ImageBlock::new("jpeg", ImageSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_url_source() {
        let original = ImageBlock::new("png", ImageSource::Url("https://example.com/a.png".into()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_s3_location_source() {
        let original = ImageBlock::new("png", ImageSource::Location(location()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn encodes_bytes_as_base64_in_json_output() {
        let json = serde_json::to_value(ImageBlock::new("png", ImageSource::Bytes(vec![1, 2, 3]))).unwrap();
        assert_eq!(json["source"]["bytes"], "AQID");
    }
}

mod video_block_to_json_from_json {
    use super::*;

    #[test]
    fn round_trips_with_bytes_source() {
        let original = VideoBlock::new("mp4", VideoSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_s3_location_source() {
        let original = VideoBlock::new("mp4", VideoSource::Location(location()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn encodes_bytes_as_base64_in_json_output() {
        let json = serde_json::to_value(VideoBlock::new("mp4", VideoSource::Bytes(vec![1, 2, 3]))).unwrap();
        assert_eq!(json["source"]["bytes"], "AQID");
    }
}

mod document_block_to_json_from_json {
    use super::*;

    #[test]
    fn round_trips_with_bytes_source() {
        let original = DocumentBlock::new("doc.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_text_source() {
        let original = DocumentBlock::new("doc.txt", "txt", DocumentSource::Text("hello".into()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_content_source() {
        let original = DocumentBlock::new("doc.md", "md", DocumentSource::Content(vec![TextBlock::new("a")]));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_s3_location_source() {
        let original = DocumentBlock::new("doc.pdf", "pdf", DocumentSource::Location(location()));
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn round_trips_with_citations_and_context() {
        let original = DocumentBlock::new("research.pdf", "pdf", DocumentSource::Bytes(vec![1, 2, 3]))
            .with_citations(CitationsConfig::new(true))
            .with_context("Research paper about AI");
        assert_eq!(round_trip(&original), original);
    }

    #[test]
    fn omits_undefined_citations_and_context_from_json() {
        let json = serde_json::to_value(DocumentBlock::new(
            "doc.pdf",
            "pdf",
            DocumentSource::Bytes(vec![1]),
        ))
        .unwrap();
        assert!(json.get("citations").is_none());
        assert!(json.get("context").is_none());
    }
}

#[test]
fn unknown_formats_load_as_other_and_round_trip() {
    let block: ImageBlock =
        serde_json::from_value(json!({ "format": "heic", "source": { "bytes": "aGVpYw==" } })).unwrap();
    assert_eq!(block.format.as_str(), "heic");
    assert!(matches!(block.format, ImageFormat::Other(_)));
    assert_ne!(ImageFormat::from("heic"), ImageFormat::Png);
    assert_eq!(ImageFormat::from("png"), ImageFormat::Png);
    assert_eq!(serde_json::to_value(&block).unwrap()["format"], "heic");
}
