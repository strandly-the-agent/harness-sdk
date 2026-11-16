//! The latest-convention parts of blocks `telemetry/__tests__/tracer.test.node.ts` doesn't cover,
//! checked against the output of the built TS `Tracer`.

use serde_json::json;

use super::{otel_parts, system_part};
use crate::types::messages::{
    ContentBlock, GuardContentBlock, GuardContentImage, GuardContentText, GuardImageFormat, GuardQualifier,
    SystemContentBlock, UnknownBlock,
};

#[test]
fn an_unknown_block_is_reported_as_unknown_block() {
    let block = ContentBlock::Unknown(UnknownBlock::new(
        json!({"futureThing": {"a": 1}})
            .as_object()
            .cloned()
            .expect("an object"),
    ));
    assert_eq!(otel_parts(&[block]), vec![json!({"type": "unknownBlock"})]);
}

#[test]
fn a_guard_content_system_block_has_no_null_member() {
    let text = GuardContentText::new("guarded", vec![GuardQualifier::Query]);
    let block = SystemContentBlock::GuardContent(GuardContentBlock::from(text));
    assert_eq!(
        system_part(&block),
        json!({"type": "guard_content", "text": {"qualifiers": ["query"], "text": "guarded"}})
    );
}

#[test]
fn a_guard_content_image_in_system_instructions_has_its_bytes_replaced() {
    let image = GuardContentImage::new(GuardImageFormat::Png, b"hello world".to_vec());
    let block = SystemContentBlock::GuardContent(GuardContentBlock::from(image));
    assert_eq!(
        system_part(&block),
        json!({"type": "guard_content", "image": {"format": "png", "source": {"bytes": "<replaced>"}}})
    );
}
