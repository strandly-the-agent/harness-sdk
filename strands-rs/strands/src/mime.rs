//! Media formats and MIME type mapping (TS `src/mime.ts`).
//!
//! The four format enums are public through [`crate::types::media`]; the MIME mapping is
//! crate-internal.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the MIME mapping is used by the model providers")
)]

open_enum! {
    /// Audio formats (TS `AudioFormat`).
    pub enum AudioFormat / UnknownAudioFormat {
        /// `mp3`
        Mp3 => "mp3",
        /// `opus`
        Opus => "opus",
        /// `wav`
        Wav => "wav",
        /// `aac`
        Aac => "aac",
        /// `flac`
        Flac => "flac",
        /// `mp4`
        Mp4 => "mp4",
        /// `ogg`
        Ogg => "ogg",
        /// `mkv`
        Mkv => "mkv",
        /// `mka`
        Mka => "mka",
        /// `x-aac`
        XAac => "x-aac",
        /// `m4a`
        M4a => "m4a",
        /// `mpeg`
        Mpeg => "mpeg",
        /// `mpga`
        Mpga => "mpga",
        /// `pcm`
        Pcm => "pcm",
        /// `webm`
        Webm => "webm",
    }
}

open_enum! {
    /// Image formats (TS `ImageFormat`, from `IMAGE_FORMATS`).
    pub enum ImageFormat / UnknownImageFormat {
        /// `png`
        Png => "png",
        /// `jpg`
        Jpg => "jpg",
        /// `jpeg`
        Jpeg => "jpeg",
        /// `gif`
        Gif => "gif",
        /// `webp`
        Webp => "webp",
    }
}

open_enum! {
    /// Video formats (TS `VideoFormat`).
    pub enum VideoFormat / UnknownVideoFormat {
        /// `mkv`
        Mkv => "mkv",
        /// `mov`
        Mov => "mov",
        /// `mp4`
        Mp4 => "mp4",
        /// `webm`
        Webm => "webm",
        /// `flv`
        Flv => "flv",
        /// `mpeg`
        Mpeg => "mpeg",
        /// `mpg`
        Mpg => "mpg",
        /// `wmv`
        Wmv => "wmv",
        /// `3gp`
        ThreeGp => "3gp",
    }
}

open_enum! {
    /// Document formats (TS `DocumentFormat`).
    pub enum DocumentFormat / UnknownDocumentFormat {
        /// `pdf`
        Pdf => "pdf",
        /// `csv`
        Csv => "csv",
        /// `doc`
        Doc => "doc",
        /// `docx`
        Docx => "docx",
        /// `xls`
        Xls => "xls",
        /// `xlsx`
        Xlsx => "xlsx",
        /// `html`
        Html => "html",
        /// `txt`
        Txt => "txt",
        /// `md`
        Md => "md",
        /// `json`
        Json => "json",
        /// `xml`
        Xml => "xml",
    }
}

/// The image formats, in TS `IMAGE_FORMATS` order.
pub(crate) const IMAGE_FORMATS: [ImageFormat; 5] = [
    ImageFormat::Png,
    ImageFormat::Jpg,
    ImageFormat::Jpeg,
    ImageFormat::Gif,
    ImageFormat::Webp,
];

/// A document, image or video format (TS `MediaFormat`, a union of the three).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(crate) enum MediaFormat {
    /// A document format.
    Document(DocumentFormat),
    /// An image format.
    Image(ImageFormat),
    /// A video format.
    Video(VideoFormat),
}

impl MediaFormat {
    /// The wire string.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Document(format) => format.as_str(),
            Self::Image(format) => format.as_str(),
            Self::Video(format) => format.as_str(),
        }
    }
}

impl std::fmt::Display for MediaFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

const TO_MIME_TYPE: [(&str, &str); 25] = [
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

const TO_MEDIA_FORMAT: [(&str, MediaFormat); 23] = [
    ("image/png", MediaFormat::Image(ImageFormat::Png)),
    ("image/jpeg", MediaFormat::Image(ImageFormat::Jpeg)),
    ("image/gif", MediaFormat::Image(ImageFormat::Gif)),
    ("image/webp", MediaFormat::Image(ImageFormat::Webp)),
    ("video/x-matroska", MediaFormat::Video(VideoFormat::Mkv)),
    ("video/quicktime", MediaFormat::Video(VideoFormat::Mov)),
    ("video/mp4", MediaFormat::Video(VideoFormat::Mp4)),
    ("video/webm", MediaFormat::Video(VideoFormat::Webm)),
    ("video/x-flv", MediaFormat::Video(VideoFormat::Flv)),
    ("video/mpeg", MediaFormat::Video(VideoFormat::Mpeg)),
    ("video/x-ms-wmv", MediaFormat::Video(VideoFormat::Wmv)),
    ("video/3gpp", MediaFormat::Video(VideoFormat::ThreeGp)),
    ("application/pdf", MediaFormat::Document(DocumentFormat::Pdf)),
    ("text/csv", MediaFormat::Document(DocumentFormat::Csv)),
    ("application/msword", MediaFormat::Document(DocumentFormat::Doc)),
    (
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        MediaFormat::Document(DocumentFormat::Docx),
    ),
    (
        "application/vnd.ms-excel",
        MediaFormat::Document(DocumentFormat::Xls),
    ),
    (
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        MediaFormat::Document(DocumentFormat::Xlsx),
    ),
    ("text/html", MediaFormat::Document(DocumentFormat::Html)),
    ("text/plain", MediaFormat::Document(DocumentFormat::Txt)),
    ("text/markdown", MediaFormat::Document(DocumentFormat::Md)),
    ("application/json", MediaFormat::Document(DocumentFormat::Json)),
    ("application/xml", MediaFormat::Document(DocumentFormat::Xml)),
];

/// The MIME type for a document, image or video format, case-insensitively (TS `toMimeType`).
pub(crate) fn to_mime_type(format: &str) -> Option<&'static str> {
    let format = format.to_lowercase();
    TO_MIME_TYPE
        .iter()
        .find(|(name, _)| *name == format)
        .map(|(_, mime)| *mime)
}

/// The media format for a MIME type, case-insensitively (TS `toMediaFormat`).
pub(crate) fn to_media_format(mime_type: &str) -> Option<MediaFormat> {
    let mime_type = mime_type.to_lowercase();
    TO_MEDIA_FORMAT
        .iter()
        .find(|(mime, _)| *mime == mime_type)
        .map(|(_, format)| format.clone())
}

/// Ported from strands-ts/src/__tests__/mime.test.ts
#[cfg(test)]
mod tests {
    use super::*;

    mod to_mime_type {
        use super::*;

        #[test]
        fn converts_s_to_s() {
            let cases = [
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
            for (format, mime) in cases {
                assert_eq!(to_mime_type(format), Some(mime), "{format}");
            }
        }

        #[test]
        fn is_case_insensitive() {
            assert_eq!(to_mime_type("PNG"), Some("image/png"));
            assert_eq!(to_mime_type("Pdf"), Some("application/pdf"));
        }

        #[test]
        fn returns_undefined_for_unknown_formats() {
            assert_eq!(to_mime_type("unknown"), None);
            assert_eq!(to_mime_type(""), None);
        }
    }

    mod to_media_format {
        use super::*;

        #[test]
        fn converts_s_to_s() {
            let cases = [
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
            for (mime, format) in cases {
                assert_eq!(
                    to_media_format(mime).as_ref().map(MediaFormat::as_str),
                    Some(format),
                    "{mime}"
                );
            }
        }

        #[test]
        fn is_case_insensitive() {
            assert_eq!(
                to_media_format("IMAGE/PNG"),
                Some(MediaFormat::Image(ImageFormat::Png))
            );
            assert_eq!(
                to_media_format("Application/PDF"),
                Some(MediaFormat::Document(DocumentFormat::Pdf))
            );
        }

        #[test]
        fn returns_undefined_for_unknown_mime_types() {
            assert_eq!(to_media_format("application/unknown"), None);
            assert_eq!(to_media_format(""), None);
        }
    }

    #[test]
    fn image_formats_lists_the_runtime_image_formats_in_order() {
        let names: Vec<&str> = IMAGE_FORMATS.iter().map(ImageFormat::as_str).collect();
        assert_eq!(names, ["png", "jpg", "jpeg", "gif", "webp"]);
    }

    #[test]
    fn unknown_format_strings_round_trip_through_other() {
        let format = ImageFormat::from("heic");
        assert!(matches!(&format, ImageFormat::Other(other) if other.as_str() == "heic"));
        assert_eq!(ImageFormat::from("png".to_owned()), ImageFormat::Png);
        assert_eq!(VideoFormat::from("3gp").as_str(), "3gp");
        assert_eq!(AudioFormat::from("x-aac"), AudioFormat::XAac);
    }
}
