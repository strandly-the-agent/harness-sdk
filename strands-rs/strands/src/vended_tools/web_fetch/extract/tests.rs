//! Ported from strands-ts/src/vended-tools/web-fetch/__tests__/extract.test.ts

use super::html_to_markdown;

#[test]
fn strips_script_and_style_content() {
    let html = "
      <html><head><title>Hi</title>
      <style>body{color:red}</style>
      </head><body>
      <p>Hello world.</p>
      <script>alert('xss')</script>
      <p>After script.</p>
      </body></html>
    ";
    let result = html_to_markdown(html);
    assert!(result.contains("# Hi"), "{result}");
    assert!(!result.contains("alert"));
    assert!(!result.contains("color:red"));
    assert!(result.contains("Hello world."));
    assert!(result.contains("After script."));
}

#[test]
fn drops_element_entirely() {
    for tag in ["nav", "form", "iframe", "svg", "canvas", "video", "audio"] {
        let result = html_to_markdown(&format!("<p>before</p><{tag}>secret content</{tag}><p>after</p>"));
        assert!(!result.contains("secret content"), "{tag}: {result}");
        assert!(result.contains("before"), "{tag}");
        assert!(result.contains("after"), "{tag}");
    }
}

#[test]
fn strips_data_uri_image_blobs() {
    let blob = "A".repeat(10_000);
    let result = html_to_markdown(&format!(
        r#"<p>text</p><img src="data:image/png;base64,{blob}" alt="alt text">"#
    ));
    assert!(!result.contains(&blob));
    assert!(!result.contains("data:"));
    assert!(result.contains("alt text"));
}

#[test]
fn preserves_regular_images_and_relative_hrefs() {
    assert!(
        html_to_markdown(r#"<img src="https://example.com/pic.png" alt="pic">"#)
            .contains("![pic](https://example.com/pic.png)")
    );
    assert!(html_to_markdown(r#"<a href="/about">About</a>"#).contains("[About](/about)"));
}

#[test]
fn drops_javascript_hrefs_but_keeps_link_text() {
    let result = html_to_markdown(r#"<a href="javascript:alert(1)">click</a>"#);
    assert!(!result.contains("javascript:"));
    assert!(result.contains("click"));
}

#[test]
fn drops_javascript_href_with_invisible_prefix() {
    for prefix in ["\u{200b}", "\u{00ad}", "\u{feff}", " "] {
        let result = html_to_markdown(&format!(r#"<a href="{prefix}javascript:alert(1)">click</a>"#));
        assert!(!result.contains("javascript:"), "{prefix:?}: {result}");
        assert!(result.contains("click"), "{prefix:?}");
    }
}

#[test]
fn drops_hrefs_but_keeps_link_text() {
    for href in ["vbscript:msgbox(1)", "data:text/html,<script>alert(1)</script>"] {
        assert_eq!(
            html_to_markdown(&format!(r#"<a href="{href}">click</a>"#)).trim(),
            "click",
            "{href}"
        );
    }
}

#[test]
fn drops_javascript_img_srcs() {
    assert!(!html_to_markdown(r#"<img src="javascript:alert(1)" alt="x">"#).contains("javascript:"));
}

#[test]
fn drops_vbscript_img_srcs() {
    assert_eq!(html_to_markdown(r#"<img src="vbscript:msgbox(1)" alt="x">"#), "");
}

#[test]
fn returns_alt_text_for_image_with_empty_src() {
    assert!(html_to_markdown(r#"<img src="" alt="description">"#).contains("description"));
}

#[test]
fn drops_javascript_img_src_with_invisible_prefix() {
    for prefix in ["\u{200b}", "\u{00ad}", "\u{feff}"] {
        let result = html_to_markdown(&format!(r#"<img src="{prefix}javascript:alert(1)" alt="x">"#));
        assert!(!result.contains("javascript:"), "{prefix:?}: {result}");
    }
}

#[test]
fn strips_data_uri_image_blob_with_invisible_prefix() {
    let blob = "A".repeat(100);
    let result = html_to_markdown(&format!(
        "<img src=\"\u{200b}data:image/png;base64,{blob}\" alt=\"alt\">"
    ));
    assert!(!result.contains(&blob));
    assert!(result.contains("alt"));
}

#[test]
fn embeds_the_page_title_as_a_top_level_heading() {
    let result = html_to_markdown("<html><head><title>My Page</title></head><body><p>body</p></body></html>");
    assert!(result.starts_with("# My Page\n"), "{result}");
    assert!(result.contains("body"));
}

#[test]
fn produces_no_heading_when_there_is_no_title() {
    assert!(!html_to_markdown("<p>no title here</p>").starts_with('#'));
}

#[test]
fn ends_with_one_newline_and_is_empty_without_content() {
    assert_eq!(html_to_markdown("<h1>Hi</h1>"), "# Hi\n");
    assert_eq!(html_to_markdown("<script>x()</script>"), "");
}
