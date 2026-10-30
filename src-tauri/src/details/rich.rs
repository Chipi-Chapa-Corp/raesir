use scraper::{ElementRef, Html, Node};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Debug, Default)]
pub struct TextBlock {
    pub spans: Vec<Span>,
    pub heading: u8,
    pub marker: Option<String>,
    pub indent: usize,
    pub quote: bool,
}

#[derive(Clone, Debug)]
pub enum Block {
    Text(TextBlock),
    Image {
        url: String,
        alt: String,
        link: Option<String>,
    },
}

pub fn parse(source: &str, base: &str) -> Vec<Block> {
    if !source.contains('<') {
        return source
            .replace("\r\n", "\n")
            .split("\n\n")
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                Block::Text(TextBlock {
                    spans: vec![Span {
                        text: text.trim().into(),
                        style: Style::default(),
                    }],
                    ..Default::default()
                })
            })
            .collect();
    }
    let document = Html::parse_fragment(source);
    let mut parser = Parser {
        base,
        blocks: Vec::new(),
        current: TextBlock::default(),
        lists: Vec::new(),
    };
    parser.walk(document.root_element(), Style::default());
    parser.flush();
    parser.blocks
}

struct Parser<'a> {
    base: &'a str,
    blocks: Vec<Block>,
    current: TextBlock,
    lists: Vec<Option<u32>>,
}

impl Parser<'_> {
    fn url(&self, value: &str) -> Option<String> {
        let url = reqwest::Url::parse(self.base)
            .ok()?
            .join(value.trim())
            .ok()?;
        super::safe_url(url.as_str()).then(|| url.into())
    }

    fn flush(&mut self) {
        if let Some(first) = self.current.spans.first_mut() {
            first.text = first.text.trim_start().into();
        }
        if let Some(last) = self.current.spans.last_mut() {
            last.text = last.text.trim_end().into();
        }
        self.current.spans.retain(|s| !s.text.is_empty());
        if !self.current.spans.is_empty() {
            self.blocks.push(Block::Text(TextBlock {
                spans: std::mem::take(&mut self.current.spans),
                marker: self.current.marker.take(),
                ..self.current.clone()
            }));
        }
    }

    fn text(&mut self, value: &str, style: &Style, line_break: bool) {
        let mut space = self
            .current
            .spans
            .last()
            .is_none_or(|s| s.text.ends_with(char::is_whitespace));
        let mut text = String::new();
        if line_break {
            text.push('\n');
        } else {
            for c in value.chars() {
                if c.is_whitespace() {
                    if !space {
                        text.push(' ');
                    }
                    space = true;
                } else {
                    text.push(c);
                    space = false;
                }
            }
        }
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.current.spans.last_mut().filter(|s| s.style == *style) {
            last.text.push_str(&text);
        } else {
            self.current.spans.push(Span {
                text,
                style: style.clone(),
            });
        }
    }

    fn walk(&mut self, element: ElementRef<'_>, mut style: Style) {
        let tag = element.value().name();
        if matches!(
            tag,
            "script" | "style" | "iframe" | "object" | "form" | "noscript"
        ) {
            return;
        }
        if tag == "br" {
            self.text("", &style, true);
            return;
        }
        if matches!(tag, "img" | "video") {
            self.flush();
            if let Some(url) = element
                .value()
                .attr(if tag == "video" { "poster" } else { "src" })
                .or_else(|| element.value().attr("data-src"))
                .and_then(|s| self.url(s))
            {
                self.blocks.push(Block::Image {
                    url,
                    alt: element.value().attr("alt").unwrap_or_default().into(),
                    link: style.link.clone(),
                });
            }
            return;
        }
        let block = matches!(
            tag,
            "p" | "div"
                | "section"
                | "article"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "li"
                | "ul"
                | "ol"
                | "blockquote"
                | "pre"
                | "hr"
        );
        let previous = if block {
            self.flush();
            let previous = self.current.clone();
            if tag.starts_with('h') && tag.len() == 2 {
                self.current.heading = tag[1..].parse().unwrap_or(0);
            }
            if tag == "blockquote" {
                self.current.quote = true;
            }
            if tag == "li" {
                self.current.marker = Some(match self.lists.last_mut() {
                    Some(Some(index)) => {
                        let marker = format!("{index}.");
                        *index += 1;
                        marker
                    }
                    _ => "•".into(),
                });
                self.current.indent = self.lists.len().saturating_sub(1);
            }
            Some(previous)
        } else {
            None
        };
        let before = self.blocks.len();
        if matches!(tag, "ul" | "ol") {
            self.lists.push((tag == "ol").then(|| {
                element
                    .value()
                    .attr("start")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1)
            }));
        }
        match tag {
            "b" | "strong" => style.bold = true,
            "i" | "em" => style.italic = true,
            "s" | "del" | "strike" => style.strike = true,
            "a" => style.link = element.value().attr("href").and_then(|s| self.url(s)),
            _ => {}
        }
        for child in element.children() {
            match child.value() {
                Node::Text(text) => self.text(text, &style, false),
                Node::Element(_) => {
                    if let Some(child) = ElementRef::wrap(child) {
                        self.walk(child, style.clone());
                    }
                }
                _ => {}
            }
        }
        if matches!(tag, "ul" | "ol") {
            self.lists.pop();
        }
        if let Some(mut previous) = previous {
            self.flush();
            if self.blocks.len() > before {
                previous.marker = None;
            }
            self.current = previous;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_headings_lists_inline_styles_and_word_boundaries() {
        let blocks = parse(
            "<h2>Features</h2><p>Try <strong>talking</strong>, <em>walking</em> &amp; toys.</p><ol start='3'><li>One<ul><li>Nested</li></ul></li><li>Two</li></ol>",
            "https://store.example/",
        );
        let texts = blocks
            .iter()
            .filter_map(|b| {
                if let Block::Text(t) = b {
                    Some(t)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(texts[0].heading, 2);
        assert_eq!(
            texts[1]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "Try talking, walking & toys."
        );
        assert!(
            texts[1]
                .spans
                .iter()
                .any(|s| s.style.bold && s.text == "talking")
        );
        assert!(
            texts[1]
                .spans
                .iter()
                .any(|s| s.style.italic && s.text == "walking")
        );
        assert_eq!(texts[2].marker.as_deref(), Some("3."));
        assert_eq!(texts[3].marker.as_deref(), Some("•"));
        assert_eq!(texts[3].indent, 1);
        assert_eq!(texts[4].marker.as_deref(), Some("4."));
    }
    #[test]
    fn resolves_links_and_images_without_importing_executable_content() {
        let blocks = parse(
            "<script>bad()</script><style>bad</style><p><a href='/game'>Game</a><a href='javascript:bad()'>no</a></p><img src='//cdn.example/art.jpg' alt='Art'><img src='file:///secret'><iframe src='https://example.com'></iframe>",
            "https://store.example/",
        );
        let Block::Text(text) = &blocks[0] else {
            panic!()
        };
        assert_eq!(
            text.spans[0].style.link.as_deref(),
            Some("https://store.example/game")
        );
        assert!(text.spans[1].style.link.is_none());
        assert!(
            matches!(&blocks[1], Block::Image { url, alt, .. } if url == "https://cdn.example/art.jpg" && alt == "Art")
        );
        assert_eq!(blocks.len(), 2);
    }
    #[test]
    fn plain_store_descriptions_keep_paragraphs_and_line_breaks() {
        let blocks = parse(
            "First\r\nline.\r\n\r\nSecond paragraph.",
            "https://store.example/",
        );
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], Block::Text(t) if t.spans[0].text == "First\nline."));
    }
}
