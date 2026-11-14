//! Markdown をサーバ側で描画し、入力由来の HTML と危険な URL を通さない。

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd, TextMergeStream, html};
use sha2::{Digest, Sha256};
use std::ops::Range;
use std::path::Path;

pub const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub const STYLE: &str = include_str!("assets/md.css");

pub fn is_markdown(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_ascii_lowercase();
    name.ends_with(".md") || name.ends_with(".md.txt")
}

pub fn hub_url(port: u16, room: &str, file: &str) -> String {
    fn encode(value: &str) -> String {
        use std::fmt::Write;
        value.bytes().fold(String::new(), |mut encoded, byte| {
            if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
                encoded.push(byte as char);
            } else {
                let _ = write!(encoded, "%{byte:02X}");
            }
            encoded
        })
    }
    let tab = if file.eq_ignore_ascii_case("README.md") { "overview" } else { "files" };
    format!(
        "http://127.0.0.1:{port}/#page=rooms&room={}&tab={tab}&file={}&status=all&period=0&scope=filtered&q=&view=preview",
        encode(room),
        encode(file)
    )
}

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

/// ハブの JavaScript と同じ規則で、ASCII の http(s) URL だけを取り出す。
fn url_ranges(text: &str) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            start += 1;
            continue;
        }
        let prefix_len = if bytes[start..].get(..8).is_some_and(|s| s.eq_ignore_ascii_case(b"https://")) {
            8
        } else if bytes[start..].get(..7).is_some_and(|s| s.eq_ignore_ascii_case(b"http://")) {
            7
        } else {
            start += 1;
            continue;
        };
        let content_start = start + prefix_len;
        let mut end = content_start;
        while let Some(&byte) = bytes.get(end) {
            if !(b'!'..=b'~').contains(&byte) || b"<>\"`".contains(&byte) {
                break;
            }
            end += 1;
        }
        let next = end;
        let end = trim_url_end(bytes, start, end);
        if end > content_start {
            ranges.push(start..end);
        }
        start = next;
    }
    ranges
}

/// 句読点と、対応する開き括弧より多い閉じ括弧を URL の末尾から外す。
fn trim_url_end(bytes: &[u8], start: usize, mut end: usize) -> usize {
    let mut balances = [0isize; 3];
    for &byte in &bytes[start..end] {
        if let Some(index) = b"([{".iter().position(|&b| b == byte) {
            balances[index] += 1;
        } else if let Some(index) = b")]}".iter().position(|&b| b == byte) {
            balances[index] -= 1;
        }
    }
    while end > start {
        let last = bytes[end - 1];
        if b".,;:!?'\"".contains(&last) {
            end -= 1;
        } else if let Some(index) = b")]}".iter().position(|&b| b == last)
            && balances[index] < 0
        {
            balances[index] += 1;
            end -= 1;
        } else {
            break;
        }
    }
    end
}

struct EmphasisEnd {
    opening: usize,
    closing: usize,
    end: usize,
    kind: TagEnd,
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    bytes[..index].iter().rev().take_while(|&&b| b == b'\\').count() % 2 == 1
}

/// 周囲の文字の種類を変えず、URL 内の記号だけをエスケープする。
fn append_escaped_urls(
    source: &str,
    range: Range<usize>,
    emphasis: &[EmphasisEnd],
    text_ranges: &[Range<usize>],
    output: &mut String,
) {
    let mut offset = range.start;
    for url in url_ranges(&source[range.clone()]) {
        let start = range.start + url.start;
        let mut end = range.start + url.end;
        // 重複した参照定義など、最初の解析で本文にならなかった範囲も変更しない。
        let text = text_ranges.partition_point(|span| span.end <= start);
        if text_ranges.get(text).is_none_or(|span| !span.contains(&start)) {
            continue;
        }
        let previous = source.as_bytes()[..start].last().copied();
        if previous == Some(b'<') {
            continue;
        }
        let first = emphasis.partition_point(|span| span.closing < start);
        for span in &emphasis[first..] {
            if span.closing >= end {
                break;
            }
            // 元の解析で URL の外から始まる強調の閉じだけを外す。
            if span.opening < start {
                // URL 内の記号で強調が分割されても、末尾に同種の閉じがあれば外側の開閉を保つ。
                end = emphasis[first..]
                    .iter()
                    .take_while(|candidate| candidate.closing < end)
                    .find(|candidate| candidate.end == end && candidate.kind == span.kind)
                    .map_or(span.closing, |candidate| candidate.closing);
                break;
            }
        }
        let end = trim_url_end(source.as_bytes(), start, end);
        let url = &source[start..end];
        if url.eq_ignore_ascii_case("http://") || url.eq_ignore_ascii_case("https://") {
            continue;
        }
        output.push_str(&source[offset..start]);
        for (index, &byte) in source.as_bytes()[start..end].iter().enumerate() {
            if b"*_~".contains(&byte) && !is_escaped(source.as_bytes(), start + index) {
                output.push('\\');
            }
            output.push(byte as char);
        }
        offset = end;
    }
    output.push_str(&source[offset..range.end]);
}

fn escape_url_markers(source: &str, options: Options) -> String {
    let parser = Parser::new_ext(source, options);
    let mut excluded: Vec<_> = parser.reference_definitions().iter().map(|(_, def)| def.span.clone()).collect();
    let mut emphasis = Vec::new();
    let mut text_ranges = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Text(_) => text_ranges.push(range),
            // Start のオフセットは、対応する End までのソース全体を含む。
            // 脚注の参照は本文にならないので、URL の範囲の境界にする。
            Event::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. } | Tag::HtmlBlock)
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::HardBreak
            | Event::FootnoteReference(_) => excluded.push(range),
            Event::Start(tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough)) => {
                let marker = source.as_bytes()[range.end - 1];
                let mut closing = range.end;
                while closing > range.start
                    && source.as_bytes()[closing - 1] == marker
                    && !is_escaped(source.as_bytes(), closing - 1)
                {
                    closing -= 1;
                }
                emphasis.push(EmphasisEnd { opening: range.start, closing, end: range.end, kind: tag.to_end() });
            }
            Event::Start(Tag::Table(_)) => {
                // 表のセルをまたいで URL を検出しないよう、区切りを除外範囲にする。
                for (index, _) in source[range.clone()].match_indices('|') {
                    let start = range.start + index;
                    let escapes = source.as_bytes()[..start].iter().rev().take_while(|&&b| b == b'\\').count();
                    if escapes == 0 {
                        excluded.push(start..start + 1);
                    } else {
                        // pulldown-cmark は表の \| をバックスラッシュの偶奇によらず保護する。
                        // 表のエスケープは書き換えず、2回目の解析で文字に戻す。
                        excluded.push(start - escapes..start + 1);
                    }
                }
            }
            _ => {}
        }
    }
    excluded.sort_unstable_by_key(|range| (range.start, range.end));
    emphasis.sort_unstable_by_key(|span| span.closing);
    let mut output = String::with_capacity(source.len());
    let mut offset = 0;
    for range in excluded {
        if range.end <= offset {
            continue;
        }
        if range.start > offset {
            append_escaped_urls(source, offset..range.start, &emphasis, &text_ranges, &mut output);
            offset = range.start;
        }
        output.push_str(&source[offset..range.end]);
        offset = range.end;
    }
    append_escaped_urls(source, offset..source.len(), &emphasis, &text_ranges, &mut output);
    output
}

fn linked_text(text: &str) -> Option<String> {
    let ranges = url_ranges(text);
    if ranges.is_empty() {
        return None;
    }
    let mut output = String::new();
    let mut offset = 0;
    for range in ranges {
        output.push_str(&escape(&text[offset..range.start]));
        let url = escape(&text[range.clone()]);
        output.push_str(&format!("<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>"));
        offset = range.end;
    }
    output.push_str(&escape(&text[offset..]));
    Some(output)
}

fn markdown_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_FOOTNOTES
}

pub fn render(bytes: &[u8], theme: Option<&str>, font: Option<&str>, hub_url: Option<&str>) -> String {
    let style = style_text();
    let text = String::from_utf8_lossy(bytes);
    let invalid_utf8 = std::str::from_utf8(bytes).is_err();
    let options = markdown_options();
    let source = escape_url_markers(text.trim_start_matches('\u{feff}'), options);
    let mut links = Vec::new();
    let mut images = Vec::new();
    let mut code_blocks = 0;
    // HTML を Text に置き換える前につなぎ、HTML 由来の文字をリンク化に渡さない。
    let parser = TextMergeStream::new(Parser::new_ext(&source, options));
    let events = parser.filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Text(value) if links.is_empty() && images.is_empty() && code_blocks == 0 => {
            Some(linked_text(&value).map_or_else(|| Event::Text(value), |html| Event::InlineHtml(html.into())))
        }
        Event::Start(Tag::CodeBlock(_)) => {
            code_blocks += 1;
            Some(event)
        }
        Event::End(TagEnd::CodeBlock) => {
            code_blocks -= 1;
            Some(event)
        }
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
    let theme_class = match theme {
        Some("dark") => "dark",
        Some("light") => "light",
        _ => "",
    };
    let classes = [theme_class, if font == Some("ud") { "font-ud" } else { "" }]
        .into_iter()
        .filter(|class| !class.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let class = if classes.is_empty() { String::new() } else { format!(" class=\"{classes}\"") };
    let warning = if invalid_utf8 {
        "<p class=\"notice\">UTF-8 として読めない部分を置換文字で表示しています。</p>"
    } else {
        ""
    };
    let hub_link = hub_url.map(|url| format!("<nav class=\"hub-link\"><a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">ハブでこのファイルを開く</a></nav>", escape(url))).unwrap_or_default();
    format!(
        "<!doctype html><html lang=\"ja\"{class}><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>workroom Markdown</title><style>{style}</style></head><body>{hub_link}{warning}<main>{body}</main></body></html>"
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
    fn plain_urls_match_ba1e77e_html_snapshots() {
        // git show ba1e77e:src/hub/markdown.rs を target/ 下の一時プロジェクトで実行して生成。
        // render() の出力全体を比較し、共通の HTML と CSS は1回だけ保存している。
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/markdown_plain_urls.json")).unwrap();
        let prefix = snapshot["prefix"].as_str().unwrap();
        let suffix = snapshot["suffix"].as_str().unwrap();
        let cases = snapshot["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 87);
        for case in cases {
            let input = case["input"].as_str().unwrap();
            let expected = format!("{prefix}{}{suffix}", case["body"].as_str().unwrap());
            assert_eq!(render(input.as_bytes(), None, None, None), expected, "入力: {input:?}");
        }
    }

    #[test]
    fn bare_url_ranges_and_rendering_match_the_hub_cases() {
        let cases: &[(&str, &[&str])] = &[
            ("参考：https://example.com/a?b=1&c=2。", &["https://example.com/a?b=1&c=2"]),
            ("https://example.com/を参照", &["https://example.com/"]),
            ("（https://example.com/x）", &["https://example.com/x"]),
            ("(see https://example.com/x).", &["https://example.com/x"]),
            ("https://ja.wikipedia.org/wiki/Foo_(bar)", &["https://ja.wikipedia.org/wiki/Foo_(bar)"]),
            ("URL: https://example.com/path, next", &["https://example.com/path"]),
            ("\"https://example.com/q\"", &["https://example.com/q"]),
            ("HTTPS://EXAMPLE.COM/", &["HTTPS://EXAMPLE.COM/"]),
            ("xhttps://example.com/", &[]),
            ("https://", &[]),
            ("ftp://example.com/", &[]),
            ("javascript:alert(1)", &[]),
            (r"C:\work\a.md", &[]),
            ("a https://a.example/ b http://b.example/c", &["https://a.example/", "http://b.example/c"]),
            ("https://example.com/a[b]{c}(d))]}.;,!?:'", &["https://example.com/a[b]{c}(d)"]),
            ("https://example.com/%E6%97%A5%E6%9C%AC", &["https://example.com/%E6%97%A5%E6%9C%AC"]),
            ("https://example.com/a\t次\nhttp://example.com/b", &["https://example.com/a", "http://example.com/b"]),
            (
                "https://example.com/a< https://example.com/b> https://example.com/c`",
                &["https://example.com/a", "https://example.com/b", "https://example.com/c"],
            ),
            ("1HTTPS://example.com/", &[]),
            ("https://.,;!?'", &[]),
        ];
        for &(input, expected) in cases {
            let urls: Vec<_> = url_ranges(input).into_iter().map(|range| &input[range]).collect();
            assert_eq!(urls, expected, "{input}");
            let output = render(input.as_bytes(), None, None, None);
            assert_eq!(output.matches("<a href=").count(), expected.len(), "{input}: {output}");
            for url in expected {
                let url = escape(url);
                assert!(
                    output.contains(&format!(
                        "<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>"
                    )),
                    "{input}"
                );
            }
        }
    }

    #[test]
    fn bare_urls_escape_both_href_and_label() {
        let input = "参考 https://example.com/a-b?x=1&y=2&title='yes' 次";
        let output = render(input.as_bytes(), None, None, None);
        let url = "https://example.com/a-b?x=1&amp;y=2&amp;title=&#39;yes";
        assert!(
            output.contains(&format!(
                "<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>&#39; 次"
            ))
        );
        assert_eq!(output.matches("<a href=").count(), 1);
    }

    #[test]
    fn bare_urls_keep_markdown_symbols_and_balanced_parentheses_as_one_link() {
        for url in [
            "https://example.com/_a_",
            "https://example.com/foo_bar_baz",
            "https://example.com/a*b*c",
            "https://example.com/~user/a~b~c",
            "https://example.com/a__b__c",
            "https://ja.wikipedia.org/wiki/Foo_(bar)",
            "https://example.com/trailing**",
        ] {
            let output = render(url.as_bytes(), None, None, None);
            assert!(
                output.contains(&format!("<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>")),
                "{url}: {output}"
            );
            assert_eq!(output.matches("<a href=").count(), 1, "{url}");
            for tag in ["<em>", "<strong>", "<del>"] {
                assert!(!output.contains(tag), "{url}: {tag}");
            }
        }
    }

    #[test]
    fn symbol_urls_preserve_original_emphasis_structure_outside_the_url() {
        fn outside_emphasis(source: &str, url: &Range<usize>, positions: &[usize]) -> Vec<(TagEnd, bool, usize)> {
            Parser::new_ext(source, markdown_options())
                .into_offset_iter()
                .filter_map(|(event, range)| {
                    let (kind, opening, position) = match event {
                        Event::Start(tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough)) => {
                            (tag.to_end(), true, range.start)
                        }
                        Event::End(kind @ (TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough)) => {
                            let length = if kind == TagEnd::Emphasis { 1 } else { 2 };
                            (kind, false, range.end - length)
                        }
                        _ => return None,
                    };
                    let position = positions[position];
                    (!url.contains(&position)).then_some((kind, opening, position))
                })
                .collect()
        }
        for (input, url) in [
            ("**https://example.com/a_b**を参照", "https://example.com/a_b"),
            ("文**https://example.com/a*b*c**文", "https://example.com/a*b*c"),
            ("文~~https://example.com/~user/a~b~c~~文", "https://example.com/~user/a~b~c"),
            ("*https://example.com/foo_bar_baz*。", "https://example.com/foo_bar_baz"),
            ("**参考 https://example.com/a__b__c**を参照", "https://example.com/a__b__c"),
            ("*https://example.com/a*b*c*。", "https://example.com/a*b*c"),
            ("*see https://example.com/a*b*c*", "https://example.com/a*b*c"),
        ] {
            let source = escape_url_markers(input, markdown_options());
            // 追加したバックスラッシュを除いた元ソース上の位置で、開き・閉じを比較する。
            let mut positions = vec![0];
            let mut original = 0;
            for &byte in source.as_bytes() {
                if input.as_bytes().get(original) == Some(&byte) {
                    original += 1;
                } else {
                    assert_eq!(byte, b'\\', "{input}");
                }
                positions.push(original);
            }
            assert_eq!(original, input.len());
            let start = input.find(url).unwrap();
            let url_range = start..start + url.len();
            let expected = outside_emphasis(input, &url_range, &(0..=input.len()).collect::<Vec<_>>());
            assert!(!expected.is_empty(), "元から強調になる入力であること: {input}");
            assert_eq!(outside_emphasis(&source, &url_range, &positions), expected, "{input}");
            let output = render(input.as_bytes(), None, None, None);
            assert!(
                output.contains(&format!("<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>")),
                "{input}: {output}"
            );
            assert_eq!(output.matches("<a href=").count(), 1);
        }
    }

    #[test]
    fn punctuation_before_closing_markers_keeps_the_original_commonmark_behavior() {
        for (input, url) in [
            ("**https://example.com/_a_**を参照", "https://example.com/_a_**"),
            ("**https://example.com/x)**を参照", "https://example.com/x)**"),
        ] {
            assert!(!Parser::new(input).any(|event| matches!(event, Event::Start(Tag::Strong))), "{input}");
            let output = render(input.as_bytes(), None, None, None);
            assert!(!output.contains("<strong>"), "{input}");
            assert!(
                output.contains(&format!(
                    "**<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>を参照"
                )),
                "{input}: {output}"
            );
            assert_eq!(url_ranges(input).into_iter().map(|range| &input[range]).collect::<Vec<_>>(), [url]);
        }
        let output = render("**x_**を参照".as_bytes(), None, None, None);
        assert!(output.contains("<p>**x_**を参照</p>"));
    }

    fn main_html(input: &str) -> String {
        let output = render(input.as_bytes(), None, None, None);
        let body = output.split_once("<main>").unwrap().1.rsplit_once("</main>").unwrap().0;
        body.replace(" target=\"_blank\" rel=\"noopener noreferrer\"", "")
    }

    #[test]
    fn footnote_references_right_after_urls_match_ba1e77e_html() {
        // 期待値は ba1e77e の render() の <main> の中身（素の URL のリンクの target と rel は省く）。
        for (input, expected) in [
            (
                "https://example.com/x[^a_b]\n\n[^a_b]: 注",
                "<p><a href=\"https://example.com/x\">https://example.com/x</a><sup class=\"footnote-reference\"><a href=\"#a_b\">1</a></sup></p>\n<div class=\"footnote-definition\" id=\"a_b\"><sup class=\"footnote-definition-label\">1</sup>\n<p>注</p>\n</div>\n",
            ),
            (
                "本文https://example.com/[^note_1]\n\n[^note_1]: 注",
                "<p>本文<a href=\"https://example.com/\">https://example.com/</a><sup class=\"footnote-reference\"><a href=\"#note_1\">1</a></sup></p>\n<div class=\"footnote-definition\" id=\"note_1\"><sup class=\"footnote-definition-label\">1</sup>\n<p>注</p>\n</div>\n",
            ),
            (
                "https://example.com/x[^*n*]\n\n[^*n*]: 注",
                "<p><a href=\"https://example.com/x\">https://example.com/x</a><sup class=\"footnote-reference\"><a href=\"#*n*\">1</a></sup></p>\n<div class=\"footnote-definition\" id=\"*n*\"><sup class=\"footnote-definition-label\">1</sup>\n<p>注</p>\n</div>\n",
            ),
            (
                "https://example.com/[^1]_x_\n\n[^1]: n",
                "<p><a href=\"https://example.com/\">https://example.com/</a><sup class=\"footnote-reference\"><a href=\"#1\">1</a></sup><em>x</em></p>\n<div class=\"footnote-definition\" id=\"1\"><sup class=\"footnote-definition-label\">1</sup>\n<p>n</p>\n</div>\n",
            ),
        ] {
            assert_eq!(main_html(input), expected, "入力: {input:?}");
        }
    }

    #[test]
    fn emphasis_opened_inside_a_url_and_closed_outside_stays_text() {
        // URL の範囲を優先し、URL の中の開きと外の閉じは強調にしない（docs/design.md の制限の節）。
        for (input, expected) in [
            (
                "https://example.com/**重要**",
                "<p><a href=\"https://example.com/**\">https://example.com/**</a>重要**</p>\n",
            ),
            (
                "詳細はhttps://example.com/**必ず確認**",
                "<p>詳細は<a href=\"https://example.com/**\">https://example.com/**</a>必ず確認**</p>\n",
            ),
            ("https://example.com/*強調*", "<p><a href=\"https://example.com/*\">https://example.com/*</a>強調*</p>\n"),
            (
                "https://example.com/~~旧~~",
                "<p><a href=\"https://example.com/~~\">https://example.com/~~</a>旧~~</p>\n",
            ),
            // 避け方：URL の後に空白か読点を入れる、または <URL> と書く。
            (
                "https://example.com/ **重要**",
                "<p><a href=\"https://example.com/\">https://example.com/</a> <strong>重要</strong></p>\n",
            ),
            (
                "https://example.com/、**重要**",
                "<p><a href=\"https://example.com/\">https://example.com/</a>、<strong>重要</strong></p>\n",
            ),
            (
                "<https://example.com/>**重要**",
                "<p><a href=\"https://example.com/\" title=\"\">https://example.com/</a><strong>重要</strong></p>\n",
            ),
        ] {
            assert_eq!(main_html(input), expected, "入力: {input:?}");
        }
    }

    #[test]
    fn source_preparation_changes_only_symbol_urls_in_visible_text() {
        let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
        for (input, expected) in [
            ("**https://example.com/_a_**を参照", r"**https://example.com/\_a\_\*\*を参照"),
            ("**https://example.com/a_b**を参照", r"**https://example.com/a\_b**を参照"),
            (r"https://example.com/a\_b\_c", r"https://example.com/a\_b\_c"),
            (r"https://example.com/a\\_b", r"https://example.com/a\\\_b"),
            (r"*see https://a.example/x\**", r"*see https://a.example/x\**"),
            ("**https://example.com/x.**", "**https://example.com/x.**"),
        ] {
            assert_eq!(escape_url_markers(input, options), expected, "{input}");
        }
        for input in [
            "```\nhttps://example.com/_a_\n```",
            "`https://example.com/_a_`",
            "[https://example.com/_label_](https://example.com/_destination_)",
            "![https://example.com/_alt_](image.svg)",
            "<div>https://example.com/_html_</div>",
            "文 <span title=\"https://example.com/_attribute_\">文字</span>",
            "[x]: https://example.com/_a_ \"https://example.com/a*b*c\"\n\n[x]",
            "[x]: https://example.com/_a_\n[x]: https://example.com/_b_.\n\n[x]",
            "参照[^https://example.com/_n_]\n\n[^https://example.com/_n_]: 脚注",
        ] {
            assert_eq!(
                escape_url_markers(input, options | Options::ENABLE_FOOTNOTES),
                input,
                "除外範囲を変えないこと: {input}"
            );
        }
    }

    #[test]
    fn escaped_emphasis_markers_and_duplicate_reference_definitions_preserve_links() {
        let output = render(br"*see https://a.example/x\**", None, None, None);
        assert!(output.contains("<em>see <a href=\"https://a.example/x*\" target=\"_blank\" rel=\"noopener noreferrer\">https://a.example/x*</a></em>"));
        let input = "[x]: https://a.example/_a_\n[x]: https://b.example/_b_.\n\n[x]";
        let output = render(input.as_bytes(), None, None, None);
        assert!(output.contains("href=\"https://a.example/_a_\""));
        assert!(!output.contains("https://b.example/"));
        assert_eq!(output.matches("<a href=").count(), 1);
    }

    #[test]
    fn surrounding_emphasis_and_strikethrough_stay_outside_the_link() {
        for (input, url, tag) in [
            ("**https://example.com/x** の強調の中", "https://example.com/x", "strong"),
            ("_https://example.com/y_", "https://example.com/y", "em"),
            ("~~https://example.com/z~~", "https://example.com/z", "del"),
            ("*https://example.com/a*", "https://example.com/a", "em"),
            ("__https://example.com/b__", "https://example.com/b", "strong"),
        ] {
            let output = render(input.as_bytes(), None, None, None);
            assert!(
                output.contains(&format!(
                    "<{tag}><a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a></{tag}>"
                )),
                "{input}: {output}"
            );
            assert_eq!(output.matches("<a href=").count(), 1, "{input}");
        }
        for input in ["**https://**", "_http://_"] {
            assert!(!render(input.as_bytes(), None, None, None).contains("<a href="), "{input}");
        }
    }

    #[test]
    fn emphasis_opened_before_other_text_keeps_its_closing_markers_outside_urls() {
        let url = "https://example.com/x";
        for (input, before, after) in [
            ("**参考：https://example.com/x**", "<strong>参考：", "</strong>"),
            ("*see https://example.com/x*", "<em>see ", "</em>"),
            ("~~旧 https://example.com/x~~", "<del>旧 ", "</del>"),
            ("**(https://example.com/x)**", "<strong>(", ")</strong>"),
        ] {
            let output = render(input.as_bytes(), None, None, None);
            assert!(
                output.contains(&format!(
                    "{before}<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>{after}"
                )),
                "{input}: {output}"
            );
            assert_eq!(output.matches("<a href=").count(), 1, "{input}");
        }
    }

    #[test]
    fn trims_punctuation_and_unbalanced_parentheses_again_after_removing_emphasis() {
        for (input, url, suffix) in [
            ("**https://example.com/x.**", "https://example.com/x", ".</strong>"),
            ("**(https://example.com/x)**", "https://example.com/x", ")</strong>"),
            ("**参考：https://example.com/x.;!?**", "https://example.com/x", ".;!?</strong>"),
            ("**(https://ja.wikipedia.org/wiki/Foo_(bar))**", "https://ja.wikipedia.org/wiki/Foo_(bar)", ")</strong>"),
        ] {
            let output = render(input.as_bytes(), None, None, None);
            assert!(
                output.contains(&format!(
                    "href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>{suffix}"
                )),
                "{input}: {output}"
            );
            assert!(output.contains("<strong>"));
        }
    }

    #[test]
    fn reference_definitions_preserve_destinations_labels_and_titles_containing_urls() {
        for (input, destination, label, title) in [
            ("[x]: https://example.com/a.\n\n[x]", "https://example.com/a.", "x", ""),
            (
                "[https://example.com/label]: https://example.com/a \"参考 https://example.com/title\"\n\n[https://example.com/label]",
                "https://example.com/a",
                "https://example.com/label",
                "参考 https://example.com/title",
            ),
            (
                "[x]: https://example.com/a.\n  \"https://example.com/title\"\n\n[x]",
                "https://example.com/a.",
                "x",
                "https://example.com/title",
            ),
        ] {
            let output = render(input.as_bytes(), None, None, None);
            assert!(output.contains(&format!("<a href=\"{destination}\" title=\"{title}\" target=\"_blank\" rel=\"noopener noreferrer\">{label}</a>")), "{input}: {output}");
            assert_eq!(output.matches("<a href=").count(), 1, "{input}");
        }
    }

    #[test]
    fn escaped_table_pipes_link_decoded_text_and_preserve_both_cells() {
        let url = "https://example.com/a";
        for escapes in 1..=3 {
            let input = format!("|a|b|\n|-|-|\n| {url}{}|b | c |", "\\".repeat(escapes));
            let output = render(input.as_bytes(), None, None, None);
            let body = output.split_once("<main>").unwrap().1.split_once("</main>").unwrap().0;
            assert_eq!(body.matches("<td>").count(), 2, "{body}");
            assert!(body.contains("<td>c</td>"), "{body}");
            let mut original = String::new();
            html::push_html(&mut original, Parser::new_ext(&input, Options::ENABLE_TABLES));
            let label = original.split_once("<td>").unwrap().1.split_once("</td>").unwrap().0;
            let link = format!("<a href=\"{label}\" target=\"_blank\" rel=\"noopener noreferrer\">{label}</a>");
            assert!(body.contains(&link), "{body}");
            assert_eq!(body.replace(&link, label), original, "リンク化しても表の列と本文を保つこと");
        }
    }

    #[test]
    fn markdown_hard_breaks_after_urls_keep_the_backslash_outside_the_link() {
        for input in
            ["https://example.com/a\\\n続き", "https://example.com/a\\\r\n続き", "https://example.com/a  \n続き"]
        {
            let output = render(input.as_bytes(), None, None, None);
            assert!(output.contains("<a href=\"https://example.com/a\" target=\"_blank\" rel=\"noopener noreferrer\">https://example.com/a</a><br />\n続き"), "{input}: {output}");
        }
    }

    #[test]
    fn escaped_angles_intraword_underscores_and_url_footnote_labels_keep_original_text() {
        let output = render(br"\<https://example.com/a>", None, None, None);
        assert!(output.contains("&lt;<a href=\"https://example.com/a\""));
        assert!(output.contains("</a>&gt;"));
        let output = render(b"snake_https://example.com/a_", None, None, None);
        assert!(output.contains("snake_<a href=\"https://example.com/a_\""));
        assert!(!output.contains("<em>"));
        let output =
            render("参照[^https://example.com/n]\n\n[^https://example.com/n]: 脚注".as_bytes(), None, None, None);
        assert!(output.contains("href=\"#https://example.com/n\""), "{output}");
        assert!(output.contains("id=\"https://example.com/n\""), "{output}");
    }

    #[test]
    fn table_urls_stop_at_cell_separators_and_do_not_hide_urls_in_later_cells() {
        let input = "|a|b|\n|-|-|\n|a|https://example.com/t|\n|https://example.com/u|https://example.com/v|\n\nhttps://example.com/a|b";
        let output = render(input.as_bytes(), None, None, None);
        assert!(output.contains("<table>"));
        assert_eq!(output.matches("<td>").count(), 4);
        assert_eq!(output.matches("<a href=").count(), 4);
        for url in ["https://example.com/t", "https://example.com/u", "https://example.com/v"] {
            assert!(
                output.contains(&format!(
                    "<td><a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a></td>"
                )),
                "{url}: {output}"
            );
        }
        assert!(output.contains("href=\"https://example.com/a|b\""));
    }

    #[test]
    fn source_escapes_and_character_references_are_decoded_before_linking() {
        let input = r"https://example.com/a\_b?x=1&amp;y=2";
        let output = render(input.as_bytes(), None, None, None);
        let escaped = escape("https://example.com/a_b?x=1&y=2");
        assert!(
            output.contains(&format!(
                "<a href=\"{escaped}\" target=\"_blank\" rel=\"noopener noreferrer\">{escaped}</a>"
            )),
            "{output}"
        );
        assert_eq!(output.matches("<a href=").count(), 1);
    }

    #[test]
    fn symbol_urls_decode_character_references_and_preserve_existing_link_attributes() {
        let input = "https://example.com/_a_?x=1&amp;y=2";
        let escaped = escape("https://example.com/_a_?x=1&y=2");
        let output = render(input.as_bytes(), None, None, None);
        // 記号を含む URL も、通常の Text と同じく文字参照を解釈してからリンクにする。
        assert!(
            output.contains(&format!(
                "<a href=\"{escaped}\" target=\"_blank\" rel=\"noopener noreferrer\">{escaped}</a>"
            ))
        );
        let url = "https://example.com/_a_";
        let output = render(format!("{url} <{url}>").as_bytes(), None, None, None);
        assert!(output.contains(&format!("<a href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>")));
        assert!(output.contains(&format!(
            "<a href=\"{url}\" title=\"\" target=\"_blank\" rel=\"noopener noreferrer\">{url}</a>"
        )));
    }

    #[test]
    fn source_offsets_preserve_excluded_spans_and_use_the_bom_stripped_input() {
        let input = "\u{feff}日本語 https://example.com/_a_\n\n[https://example.com/link](https://example.com/destination) ![https://example.com/alt](image.svg) `https://example.com/code`\n\n<div>https://example.com/html</div>\n\nhttps://example.com/after";
        let output = render(input.as_bytes(), None, None, None);
        assert_eq!(output.matches("<a href=").count(), 3);
        assert!(output.contains("<p>日本語 <a href=\"https://example.com/_a_\""));
        assert!(output.contains("alt=\"https://example.com/alt\""));
        assert!(output.contains("<code>https://example.com/code</code>"));
        assert!(output.contains("&lt;div&gt;https://example.com/html&lt;/div&gt;"));
        assert!(output.contains("href=\"https://example.com/after\""));
        assert!(render(b"<https://example.com/unclosed", None, None, None).contains("&lt;<a href="));
    }

    #[test]
    fn urls_between_inline_html_tags_are_linked_and_the_tags_stay_escaped_text() {
        let output = render(
            b"\xe6\x96\x87 <b>https://example.com/</b> <script>https://example.com/s</script>",
            None,
            None,
            None,
        );
        assert!(output.contains(
            "&lt;b&gt;<a href=\"https://example.com/\" target=\"_blank\" rel=\"noopener noreferrer\">https://example.com/</a>&lt;/b&gt;"
        ));
        assert!(output.contains("&lt;script&gt;"));
        assert!(!output.contains("<b>"));
        assert!(!output.contains("<script>"));
    }

    #[test]
    fn skips_code_link_labels_image_alt_and_escaped_raw_html() {
        for input in [
            "```text\nhttps://example.com/code\n```",
            "    https://example.com/indented\n",
            "`https://example.com/inline`",
            "[https://example.com/label](javascript:evil)",
            "![https://example.com/alt](image.svg)",
            "![https://example.com/alt](javascript:evil)",
            "<div>https://example.com/html</div>\n",
            "<script>https://example.com/script</script>\n",
            "文 <span title=\"https://example.com/attribute\">文字</span>",
        ] {
            let output = render(input.as_bytes(), None, None, None);
            assert!(!output.contains("<a href="), "{input}: {output}");
            assert!(output.contains("https://example.com/"), "{input}");
            assert!(!output.contains("<script>"));
        }
        for input in ["[https://example.com/label](https://example.com/destination)", "<https://example.com/autolink>"]
        {
            assert_eq!(render(input.as_bytes(), None, None, None).matches("<a href=").count(), 1, "{input}");
        }
        let output = render(b"![https://example.com/alt](image.svg)", None, None, None);
        assert!(output.contains("alt=\"https://example.com/alt\""));
        assert!(!output.contains("&lt;a"));
        // 除外した場所を抜けた後の本文では、再びリンクにする。
        let output = render(b"<div>https://example.com/html</div>\n\n`code` [x](javascript:evil) ![alt](image.svg)\n\n```\ncode\n```\n\nhttps://example.com/after", None, None, None);
        assert_eq!(output.matches("<a href=").count(), 1);
        assert!(output.contains("href=\"https://example.com/after\""));
    }

    #[test]
    fn escapes_html_and_removes_unsafe_link_and_image_schemes() {
        let output = render(b"<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[x](javascript:alert) ![alt](data:text/html,evil) [y](JaVaScRiPt:evil) [z](javascript&colon;evil)", None, None, None);
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
        let output = render(b"[next](../docs/a.md) ![image](images/a.png) [web](https://example.com/?a=1&b=2)\n\n|a|b|\n|-|-|\n|1|2|\n\n- [x] done\n\n~~old~~ note[^1]\n\n[^1]: footnote", Some("dark"), None, None);
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
    fn font_accepts_only_ud_and_combines_with_the_theme() {
        for (theme, font, expected) in [
            (None, None, "<html lang=\"ja\">"),
            (None, Some("ud"), "<html lang=\"ja\" class=\"font-ud\">"),
            (Some("dark"), Some("ud"), "<html lang=\"ja\" class=\"dark font-ud\">"),
            (Some("light"), Some("ud"), "<html lang=\"ja\" class=\"light font-ud\">"),
            (Some("dark"), Some("UD"), "<html lang=\"ja\" class=\"dark\">"),
            (None, Some(""), "<html lang=\"ja\">"),
            (Some("light"), Some("system"), "<html lang=\"ja\" class=\"light\">"),
            (Some("invalid"), Some("ud"), "<html lang=\"ja\" class=\"font-ud\">"),
            (None, Some("\"><script>"), "<html lang=\"ja\">"),
        ] {
            let output = render(b"# title", theme, font, None);
            assert!(output.starts_with(&format!("<!doctype html>{expected}")), "{theme:?} {font:?}");
            assert!(!output.contains("<script>"));
            let style = output.split_once("<style>").unwrap().1.split_once("</style>").unwrap().0;
            let hash = base64(&Sha256::digest(style.as_bytes()));
            assert!(csp(49123).contains(&format!("style-src 'sha256-{hash}'")));
        }
    }

    #[test]
    fn embedded_style_hash_matches_csp_and_invalid_utf8_is_visible() {
        let output = render(b"bad\xff", Some("\"><script>"), None, None);
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
        let output = render(input.as_bytes(), None, None, None);
        for href in ["href=\"mailto:reader@example.invalid\"", "href=\"mailto:writer@example.invalid\""] {
            assert!(default_html.contains(href));
            assert!(output.contains(href));
        }
        assert!(!output.contains("mailto:mailto:"));
    }

    #[test]
    fn recognizes_markdown_and_double_extensions_without_case_sensitivity() {
        for name in ["a.md", "a.MD", "a.md.txt", "a.Md.TxT", "a.MD.TXT"] {
            assert!(is_markdown(Path::new(name)), "{name}");
        }
        for name in ["a.txt", "a.md.txt.bak", "a.md.html", "md.txt"] {
            assert!(!is_markdown(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn hub_link_encodes_room_and_file_and_preserves_the_sandbox_policy() {
        let url = hub_url(49123, "EXAMPLE/2026-11-03-調査", "docs/報告 & a+1.md.txt");
        assert_eq!(
            url,
            "http://127.0.0.1:49123/#page=rooms&room=EXAMPLE%2F2026-11-03-%E8%AA%BF%E6%9F%BB&tab=files&file=docs%2F%E5%A0%B1%E5%91%8A%20%26%20a%2B1.md.txt&status=all&period=0&scope=filtered&q=&view=preview"
        );
        let output = render(b"# title", None, None, Some(&url));
        assert!(output.contains(&format!("href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\"", escape(&url))));
        assert!(output.find("ハブでこのファイルを開く").unwrap() < output.find("<main>").unwrap());
        assert!(csp(49123).starts_with("sandbox allow-popups allow-popups-to-escape-sandbox; default-src 'none';"));
        assert!(!csp(49123).contains("allow-same-origin"));
    }
}
