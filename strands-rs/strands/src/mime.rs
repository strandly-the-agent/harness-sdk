//! Media formats and their MIME types (TS `mime.ts`).

use crate::types::open_enum::open_enum;

open_enum! {
    /// Image formats (TS `ImageFormat`). Unknown formats load as [`ImageFormat::Other`].
    pub enum ImageFormat / UnknownImageFormat {
        /// `png`
        Png = "png",
        /// `jpg`
        Jpg = "jpg",
        /// `jpeg`
        Jpeg = "jpeg",
        /// `gif`
        Gif = "gif",
        /// `webp`
        Webp = "webp",
    }
}

open_enum! {
    /// Audio formats (TS `AudioFormat`). Unknown formats load as [`AudioFormat::Other`].
    pub enum AudioFormat / UnknownAudioFormat {
        /// `mp3`
        Mp3 = "mp3",
        /// `opus`
        Opus = "opus",
        /// `wav`
        Wav = "wav",
        /// `aac`
        Aac = "aac",
        /// `flac`
        Flac = "flac",
        /// `mp4`
        Mp4 = "mp4",
        /// `ogg`
        Ogg = "ogg",
        /// `mkv`
        Mkv = "mkv",
        /// `mka`
        Mka = "mka",
        /// `x-aac`
        XAac = "x-aac",
        /// `m4a`
        M4a = "m4a",
        /// `mpeg`
        Mpeg = "mpeg",
        /// `mpga`
        Mpga = "mpga",
        /// `pcm`
        Pcm = "pcm",
        /// `webm`
        Webm = "webm",
    }
}

open_enum! {
    /// Video formats (TS `VideoFormat`). Unknown formats load as [`VideoFormat::Other`].
    pub enum VideoFormat / UnknownVideoFormat {
        /// `mkv`
        Mkv = "mkv",
        /// `mov`
        Mov = "mov",
        /// `mp4`
        Mp4 = "mp4",
        /// `webm`
        Webm = "webm",
        /// `flv`
        Flv = "flv",
        /// `mpeg`
        Mpeg = "mpeg",
        /// `mpg`
        Mpg = "mpg",
        /// `wmv`
        Wmv = "wmv",
        /// `3gp`
        ThreeGp = "3gp",
    }
}

open_enum! {
    /// Document formats (TS `DocumentFormat`). Unknown formats load as [`DocumentFormat::Other`].
    pub enum DocumentFormat / UnknownDocumentFormat {
        /// `pdf`
        Pdf = "pdf",
        /// `csv`
        Csv = "csv",
        /// `doc`
        Doc = "doc",
        /// `docx`
        Docx = "docx",
        /// `xls`
        Xls = "xls",
        /// `xlsx`
        Xlsx = "xlsx",
        /// `html`
        Html = "html",
        /// `txt`
        Txt = "txt",
        /// `md`
        Md = "md",
        /// `json`
        Json = "json",
        /// `xml`
        Xml = "xml",
    }
}

const MIME_TYPES: [(&str, &str); 25] = [
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

/// The MIME type for an image, video or document format, case-insensitively (TS `toMimeType`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn to_mime_type(format: &str) -> Option<&'static str> {
    let format = format.to_lowercase();
    MIME_TYPES
        .iter()
        .find(|(f, _)| *f == format)
        .map(|(_, mime)| *mime)
}

/// The media format for a MIME type, case-insensitively (TS `toMediaFormat`).
///
/// Where two formats share a MIME type, the canonical one wins (`image/jpeg` is `jpeg`,
/// `video/mpeg` is `mpeg`).
#[cfg_attr(not(test), expect(dead_code, reason = "used by model providers"))]
pub(crate) fn to_media_format(mime_type: &str) -> Option<&'static str> {
    let mime_type = mime_type.to_lowercase();
    MIME_TYPES
        .iter()
        .filter(|(format, _)| !matches!(*format, "jpg" | "mpg"))
        .find(|(_, mime)| *mime == mime_type)
        .map(|(format, _)| *format)
}

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/__tests__/mime.test.ts
    use super::*;

    const FORMATS: [(&str, &str); 25] = [
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

    #[test]
    fn converts_format_to_mime_type() {
        for (format, mime) in FORMATS {
            assert_eq!(to_mime_type(format), Some(mime), "{format}");
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
    }

    #[test]
    fn converts_mime_type_to_format() {
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
            assert_eq!(to_media_format(mime), Some(format), "{mime}");
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
}
