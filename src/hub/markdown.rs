//! Markdown をサーバ側で描画し、入力由来の HTML と危険な URL を通さない。

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd, html};
use sha2::{Digest, Sha256};

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const STYLE: &str = include_str!("assets/md.css");

fn style_text() -> String {
    // HTML の改行正規化後の文字列と一致させる（Windows の checkout にも対応）。
    STYLE.replace("\r\n", "\n").replace('\r', "\n")
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn allowed_url(url: &str) -> bool {
    if url.chars().any(char::is_control) {
        return false;
    }
    let prefix = url.trim_start().split(['/', '?', '#']).next().unwrap_or_default();
    match prefix.split_once(':') {
        Some((scheme, _)) => matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "mailto"),
        None => true,
    }
}

pub fn render(bytes: &[u8], theme: Option<&str>) -> String {
    let style = style_text();
    let text = String::from_utf8_lossy(bytes);
    let invalid_utf8 = std::str::from_utf8(bytes).is_err();
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES;
    let mut links = Vec::new();
    let mut images = Vec::new();
    let events = Parser::new_ext(text.trim_start_matches('\u{feff}'), options).filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Start(Tag::Link { link_type, dest_url, title, .. }) => {
            let dest_url = if link_type == LinkType::Email { format!("mailto:{dest_url}").into() } else { dest_url };
            let allowed = allowed_url(&dest_url);
            links.push(allowed);
            allowed.then(|| {
                let lower = dest_url.trim_start().to_ascii_lowercase();
                let external = lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("//");
                let attributes = if external { " target=\"_blank\" rel=\"noopener noreferrer\"" } else { "" };
                Event::InlineHtml(
                    format!("<a href=\"{}\" title=\"{}\"{attributes}>", escape(&dest_url), escape(&title)).into(),
                )
            })
        }
        Event::End(TagEnd::Link) => links.pop().unwrap_or(false).then(|| Event::InlineHtml("</a>".into())),
        Event::Start(Tag::Image { ref dest_url, .. }) => {
            let allowed = allowed_url(dest_url);
            images.push(allowed);
            allowed.then_some(event)
        }
        Event::End(TagEnd::Image) => images.pop().unwrap_or(false).then_some(event),
        other => Some(other),
    });
    let mut body = String::new();
    html::push_html(&mut body, events);
    let class = match theme {
        Some("dark") => " class=\"dark\"",
        Some("light") => " class=\"light\"",
        _ => "",
    };
    let warning = if invalid_utf8 {
        "<p class=\"notice\">UTF-8 として読めない部分を置換文字で表示しています。</p>"
    } else {
        ""
    };
    format!(
        "<!doctype html><html lang=\"ja\"{class}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>workroom Markdown</title><style>{style}</style></head><body>{warning}<main>{body}</main></body></html>"
    )
}

pub fn csp(port: u16) -> String {
    let hash = base64(&Sha256::digest(style_text().as_bytes()));
    format!(
        "sandbox allow-popups allow-popups-to-escape-sandbox; default-src 'none'; style-src 'sha256-{hash}'; img-src http://127.0.0.1:{port} http://localhost:{port} data:; frame-ancestors http://127.0.0.1:{port} http://localhost:{port}"
    )
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as usize;
        let b = chunk.get(1).copied().unwrap_or(0) as usize;
        let c = chunk.get(2).copied().unwrap_or(0) as usize;
        output.push(ALPHABET[a >> 2] as char);
        output.push(ALPHABET[((a & 3) << 4) | (b >> 4)] as char);
        output.push(if chunk.len() > 1 { ALPHABET[((b & 15) << 2) | (c >> 6)] as char } else { '=' });
        output.push(if chunk.len() > 2 { ALPHABET[c & 63] as char } else { '=' });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html_and_removes_unsafe_link_and_image_schemes() {
        let output = render(b"<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[x](javascript:alert) ![alt](data:text/html,evil) [y](JaVaScRiPt:evil) [z](javascript&colon;evil)", None);
        assert!(output.contains("&lt;script&gt;"));
        assert!(output.contains("&lt;img"));
        assert!(!output.contains("<script>"));
        assert!(!output.contains("<img src=\"data:"));
        assert!(!output.contains("href=\"javascript:"));
        assert!(!output.contains("href=\"JaVaScRiPt:"));
        assert!(output.contains("alt"));
        assert!(!allowed_url("java\nscript:alert(1)"));
    }

    #[test]
    fn preserves_relative_links_images_and_extensions_and_marks_external_links() {
        let output = render(b"[next](../docs/a.md) ![image](images/a.png) [web](https://example.com/?a=1&b=2)\n\n|a|b|\n|-|-|\n|1|2|\n\n- [x] done\n\n~~old~~ note[^1]\n\n[^1]: footnote", Some("dark"));
        assert!(output.contains("href=\"../docs/a.md\""));
        assert!(output.contains("src=\"images/a.png\""));
        assert!(output.contains("target=\"_blank\" rel=\"noopener noreferrer\""));
        assert!(output.contains("<table>"));
        assert!(output.contains("type=\"checkbox\""));
        assert!(output.contains("<del>old</del>"));
        assert!(output.contains("footnote"));
        assert!(output.contains("<html lang=\"ja\" class=\"dark\">"));
    }

    #[test]
    fn embedded_style_hash_matches_csp_and_invalid_utf8_is_visible() {
        let output = render(b"bad\xff", Some("\"><script>"));
        let style = output.split_once("<style>").unwrap().1.split_once("</style>").unwrap().0;
        let hash = base64(&Sha256::digest(style.as_bytes()));
        let policy = csp(49123);
        assert!(policy.contains(&format!("style-src 'sha256-{hash}'")));
        assert!(policy.contains("http://127.0.0.1:49123"));
        assert!(!policy.contains("allow-same-origin"));
        assert!(output.contains("UTF-8"));
        assert!(output.contains('\u{fffd}'));
        for (value, expected) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
            assert_eq!(base64(value.as_bytes()), expected);
        }
    }

    #[test]
    fn email_autolinks_preserve_mailto_like_the_default_renderer() {
        let input = "<reader@example.invalid> [明示](mailto:writer@example.invalid)";
        let mut default_html = String::new();
        html::push_html(&mut default_html, Parser::new(input));
        let output = render(input.as_bytes(), None);
        for href in ["href=\"mailto:reader@example.invalid\"", "href=\"mailto:writer@example.invalid\""] {
            assert!(default_html.contains(href));
            assert!(output.contains(href));
        }
        assert!(!output.contains("mailto:mailto:"));
    }
}
