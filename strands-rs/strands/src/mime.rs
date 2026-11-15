//! Media formats and MIME type conversion (TS `mime.ts`).
//!
//! The format unions are open: TS accepts any string at runtime, so an unknown format (for example
//! `heic`) loads as `Other` and is written back unchanged instead of failing the whole message.

open_string_enum! {
    /// An audio format (TS `AudioFormat`).
    AudioFormat, UnknownAudioFormat {
        Mp3 => "mp3",
        Opus => "opus",
        Wav => "wav",
        Aac => "aac",
        Flac => "flac",
        Mp4 => "mp4",
        Ogg => "ogg",
        Mkv => "mkv",
        Mka => "mka",
        XAac => "x-aac",
        M4a => "m4a",
        Mpeg => "mpeg",
        Mpga => "mpga",
        Pcm => "pcm",
        Webm => "webm",
    }
}

open_string_enum! {
    /// An image format (TS `ImageFormat`).
    ImageFormat, UnknownImageFormat {
        Png => "png",
        Jpg => "jpg",
        Jpeg => "jpeg",
        Gif => "gif",
        Webp => "webp",
    }
}

open_string_enum! {
    /// A video format (TS `VideoFormat`).
    VideoFormat, UnknownVideoFormat {
        Mkv => "mkv",
        Mov => "mov",
        Mp4 => "mp4",
        Webm => "webm",
        Flv => "flv",
        Mpeg => "mpeg",
        Mpg => "mpg",
        Wmv => "wmv",
        ThreeGp => "3gp",
    }
}

open_string_enum! {
    /// A document format (TS `DocumentFormat`).
    DocumentFormat, UnknownDocumentFormat {
        Pdf => "pdf",
        Csv => "csv",
        Doc => "doc",
        Docx => "docx",
        Xls => "xls",
        Xlsx => "xlsx",
        Html => "html",
        Txt => "txt",
        Md => "md",
        Json => "json",
        Xml => "xml",
    }
}

/// Returns the MIME type for an image, video or document format, ignoring case (TS `toMimeType`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by the model providers"))]
pub(crate) fn to_mime_type(format: &str) -> Option<&'static str> {
    let mime = match format.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "flv" => "video/x-flv",
        "mpeg" | "mpg" => "video/mpeg",
        "wmv" => "video/x-ms-wmv",
        "3gp" => "video/3gpp",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "html" => "text/html",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        _ => return None,
    };
    Some(mime)
}

/// Returns the image, video or document format for a MIME type, ignoring case (TS `toMediaFormat`).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the model providers and file tools")
)]
pub(crate) fn to_media_format(mime_type: &str) -> Option<&'static str> {
    let format = match mime_type.to_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-flv" => "flv",
        "video/mpeg" => "mpeg",
        "video/x-ms-wmv" => "wmv",
        "video/3gpp" => "3gp",
        "application/pdf" => "pdf",
        "text/csv" => "csv",
        "application/msword" => "doc",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "text/html" => "html",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "application/json" => "json",
        "application/xml" => "xml",
        _ => return None,
    };
    Some(format)
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/__tests__/mime.test.ts

    use super::*;

    const FORMAT_TO_MIME: &[(&str, &str)] = &[
        ("png", "image/png"),
        ("jpg", "image/jpeg"),
        ("jpeg", "image/jpeg"),
        ("gif", "image/gif"),
        ("webp", "image/webp"),
        ("mkv", "video/x-matroska"),
        ("mov", "video/quicktime"),
        ("mp4", "video/mp4"),
        ("webm", "video/webm"),
        ("flv", "video/x-flv"),
        ("mpeg", "video/mpeg"),
        ("mpg", "video/mpeg"),
        ("wmv", "video/x-ms-wmv"),
        ("3gp", "video/3gpp"),
        ("pdf", "application/pdf"),
        ("csv", "text/csv"),
        ("doc", "application/msword"),
        (
            "docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        ("xls", "application/vnd.ms-excel"),
        (
            "xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
        ("html", "text/html"),
        ("txt", "text/plain"),
        ("md", "text/markdown"),
        ("json", "application/json"),
        ("xml", "application/xml"),
    ];

    const MIME_TO_FORMAT: &[(&str, &str)] = &[
        ("image/png", "png"),
        ("image/jpeg", "jpeg"),
        ("image/gif", "gif"),
        ("image/webp", "webp"),
        ("video/x-matroska", "mkv"),
        ("video/quicktime", "mov"),
        ("video/mp4", "mp4"),
        ("video/webm", "webm"),
        ("video/x-flv", "flv"),
        ("video/mpeg", "mpeg"),
        ("video/x-ms-wmv", "wmv"),
        ("video/3gpp", "3gp"),
        ("application/pdf", "pdf"),
        ("text/csv", "csv"),
        ("application/msword", "doc"),
        (
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "docx",
        ),
        ("application/vnd.ms-excel", "xls"),
        (
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "xlsx",
        ),
        ("text/html", "html"),
        ("text/plain", "txt"),
        ("text/markdown", "md"),
        ("application/json", "json"),
        ("application/xml", "xml"),
    ];

    #[test]
    fn to_mime_type_converts_s_to_s() {
        for (format, mime) in FORMAT_TO_MIME {
            assert_eq!(to_mime_type(format), Some(*mime), "{format}");
        }
    }

    #[test]
    fn to_mime_type_is_case_insensitive() {
        assert_eq!(to_mime_type("PNG"), Some("image/png"));
        assert_eq!(to_mime_type("Mp4"), Some("video/mp4"));
        assert_eq!(to_mime_type("PDF"), Some("application/pdf"));
    }

    #[test]
    fn returns_undefined_for_unknown_formats() {
        assert_eq!(to_mime_type("unknown"), None);
        assert_eq!(to_mime_type("bmp"), None);
        assert_eq!(to_mime_type(""), None);
        assert_eq!(to_mime_type("constructor"), None);
    }

    #[test]
    fn to_media_format_converts_s_to_s() {
        for (mime, format) in MIME_TO_FORMAT {
            assert_eq!(to_media_format(mime), Some(*format), "{mime}");
        }
    }

    #[test]
    fn to_media_format_is_case_insensitive() {
        assert_eq!(to_media_format("IMAGE/PNG"), Some("png"));
        assert_eq!(to_media_format("Video/Mp4"), Some("mp4"));
        assert_eq!(to_media_format("Application/PDF"), Some("pdf"));
    }

    #[test]
    fn returns_undefined_for_unknown_mime_types() {
        assert_eq!(to_media_format("image/bmp"), None);
        assert_eq!(to_media_format("application/octet-stream"), None);
        assert_eq!(to_media_format(""), None);
    }

    #[test]
    fn unknown_formats_round_trip_as_other() {
        let format: ImageFormat = serde_json::from_str("\"heic\"").unwrap();
        assert_eq!(format.as_str(), "heic");
        assert!(matches!(format, ImageFormat::Other(_)));
        assert_eq!(serde_json::to_string(&format).unwrap(), "\"heic\"");
        assert_eq!(ImageFormat::from("png"), ImageFormat::Png);
        assert_eq!(VideoFormat::from("3gp"), VideoFormat::ThreeGp);
        assert_eq!(AudioFormat::from("x-aac").to_string(), "x-aac");
    }

    #[test]
    fn mime_maps_match_the_conformance_fixture() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../conformance/fixtures/types/mime.json")).unwrap();
        let cases = [
            ("toMimeType", to_mime_type as fn(&str) -> Option<&'static str>),
            ("toMediaFormat", to_media_format),
        ];
        for (name, convert) in cases {
            for (input, expected) in fixture[name].as_object().unwrap() {
                assert_eq!(convert(input), expected.as_str(), "{name}({input})");
            }
        }
    }
}
