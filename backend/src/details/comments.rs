use super::rich;
use linkify::{LinkFinder, LinkKind};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommentLink {
    pub range: Range<usize>,
    pub url: String,
}

pub(crate) fn markdown_text(source: &str, base: &str) -> (String, Vec<CommentLink>) {
    use pulldown_cmark::{Event, Parser, Tag};

    if !Parser::new(source).any(|event| matches!(event, Event::Start(Tag::Link { .. }))) {
        return formatted_text(source, base);
    }
    let mut html = String::new();
    pulldown_cmark::html::push_html(
        &mut html,
        Parser::new(source).map(|event| match event {
            Event::SoftBreak => Event::HardBreak,
            event => event,
        }),
    );
    formatted_text(&html, base)
}

pub(crate) fn formatted_text(source: &str, base: &str) -> (String, Vec<CommentLink>) {
    if !source.contains('<') && source.contains('&') {
        let text = scraper::Html::parse_fragment(source)
            .root_element()
            .text()
            .collect::<String>();
        return (text.trim().into(), Vec::new());
    }
    let blocks = if source.contains('<') {
        rich::parse_html(source, base, true)
    } else {
        rich::parse(source, base)
    };
    let mut text = String::new();
    let mut links: Vec<CommentLink> = Vec::new();
    for block in blocks {
        let rich::Block::Text(block) = block else {
            continue;
        };
        if !text.is_empty() {
            text.push('\n');
        }
        for span in block.spans {
            let start = text.len();
            text.push_str(&span.text);
            if let Some(url) = span.style.link {
                // An anchor can contain multiple spans, for instance one bold word.
                if let Some(last) = links
                    .last_mut()
                    .filter(|link| link.range.end == start && link.url == url)
                {
                    last.range.end = text.len();
                } else {
                    links.push(CommentLink {
                        range: start..text.len(),
                        url,
                    });
                }
            }
        }
    }
    (text, links)
}

pub fn links(text: &str, formatted: &[CommentLink]) -> Vec<CommentLink> {
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]).url_must_have_scheme(false);
    let mut links = Vec::new();
    let mut start = 0;
    let add_pasted_links = |range: Range<usize>, links: &mut Vec<_>| {
        let offset = range.start;
        links.extend(finder.links(&text[range]).filter_map(|link| {
            let value = link.as_str();
            let lower = value.to_ascii_lowercase();
            let destination = if lower.starts_with("www.") {
                format!("https://{value}")
            } else if lower.starts_with("http://") || lower.starts_with("https://") {
                value.to_owned()
            } else {
                return None;
            };
            let url = web_url(&destination, None)?;
            Some(CommentLink {
                range: offset + link.start()..offset + link.end(),
                url,
            })
        }));
    };
    for link in formatted {
        if link.range.start < start
            || link.range.is_empty()
            || text.get(link.range.clone()).is_none()
        {
            continue;
        }
        if let Some(url) = web_url(&link.url, None) {
            add_pasted_links(start..link.range.start, &mut links);
            links.push(CommentLink {
                range: link.range.clone(),
                url,
            });
            start = link.range.end;
        }
    }
    add_pasted_links(start..text.len(), &mut links);
    links
}

pub(super) fn web_url(value: &str, base: Option<&str>) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    let url = if let Some(base) = base {
        Url::parse(base).ok()?.join(value).ok()?
    } else {
        Url::parse(value).ok()?
    };
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels<'a>(text: &'a str, links: &'a [CommentLink]) -> Vec<(&'a str, &'a str)> {
        links
            .iter()
            .map(|link| (&text[link.range.clone()], link.url.as_str()))
            .collect()
    }

    #[test]
    fn detects_pasted_web_links_without_swallowing_punctuation() {
        let text = "📎 (https://en.example/wiki/Test_(example)), HTTP://example.org/a, www.example.org/é. file.txt ftp://example.org/ file:///tmp/a";
        assert_eq!(
            labels(text, &links(text, &[])),
            [
                (
                    "https://en.example/wiki/Test_(example)",
                    "https://en.example/wiki/Test_(example)"
                ),
                ("HTTP://example.org/a", "http://example.org/a"),
                ("www.example.org/é", "https://www.example.org/%C3%A9"),
            ],
        );
    }

    #[test]
    fn preserves_formatted_comment_links_and_resolves_relative_targets() {
        let source = "[A **wiki**](/wiki) & <a href='http://example.org/faq?a=1&amp;b=2'>FAQ</a>\n<a href='steam://install/42'>Install</a> via https://example.org/download.";
        let (text, formatted) = markdown_text(source, "https://mods.example/items/42");
        assert_eq!(
            text,
            "A wiki & FAQ\nInstall via https://example.org/download."
        );
        assert_eq!(
            labels(&text, &links(&text, &formatted)),
            [
                ("A wiki", "https://mods.example/wiki"),
                ("FAQ", "http://example.org/faq?a=1&b=2"),
                (
                    "https://example.org/download",
                    "https://example.org/download"
                ),
            ],
        );
    }
}
