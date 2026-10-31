//! Small read-only Valve KeyValues parser. Keep duplicate keys (e.g. depots).
#[derive(Debug, Clone)]
pub enum Value {
    Text(String),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.entries()?
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn entries(&self) -> Option<&[(String, Value)]> {
        match self {
            Self::Object(v) => Some(v),
            _ => None,
        }
    }
    pub fn field(&self, name: &str) -> Option<&str> {
        self.get(name)?.text()
    }
}

pub fn parse(input: &str) -> Result<Value, String> {
    let mut p = Parser {
        chars: input.trim_start_matches('\u{feff}').chars().peekable(),
    };
    p.object(false, 0)
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}
impl Parser<'_> {
    fn skip(&mut self) {
        loop {
            while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
                self.chars.next();
            }
            let mut peek = self.chars.clone();
            if peek.next() == Some('/') && peek.next() == Some('/') {
                for c in self.chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }
    fn token(&mut self) -> Result<String, String> {
        self.skip();
        if self.chars.peek() == Some(&'"') {
            self.chars.next();
            let mut out = String::new();
            while let Some(c) = self.chars.next() {
                match c {
                    '"' => return Ok(out),
                    '\\' => match self.chars.next() {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some(c) => {
                            out.push('\\');
                            out.push(c);
                        }
                        None => return Err("Unterminated escape".into()),
                    },
                    c => out.push(c),
                }
            }
            Err("Unterminated quoted string".into())
        } else {
            let mut out = String::new();
            while self
                .chars
                .peek()
                .is_some_and(|c| !c.is_whitespace() && !['{', '}'].contains(c))
            {
                out.push(self.chars.next().unwrap());
            }
            if out.is_empty() {
                Err("Expected a key or value".into())
            } else {
                Ok(out)
            }
        }
    }
    fn object(&mut self, nested: bool, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("KeyValues nesting limit exceeded".into());
        }
        let mut entries = Vec::new();
        loop {
            self.skip();
            match self.chars.peek() {
                None if nested => return Err("Unclosed object".into()),
                None => return Ok(Value::Object(entries)),
                Some('}') if nested => {
                    self.chars.next();
                    return Ok(Value::Object(entries));
                }
                Some('}') => return Err("Unexpected closing brace".into()),
                _ => {}
            }
            let key = self.token()?;
            self.skip();
            let value = if self.chars.peek() == Some(&'{') {
                self.chars.next();
                self.object(true, depth + 1)?
            } else {
                Value::Text(self.token()?)
            };
            entries.push((key, value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn comments_unicode_backslashes_and_case() {
        let v = parse(
            "\u{feff}// comment\n libraryfolders { 0 { path \"D:\\\\Games\" } 1 \"/mnt/é\" }",
        )
        .unwrap();
        let root = v.get("LibraryFolders").unwrap();
        assert_eq!(root.get("0").unwrap().field("path"), Some("D:\\Games"));
        assert_eq!(root.field("1"), Some("/mnt/é"));
        assert!(parse("a { b \"unfinished").is_err());
        assert!(parse("}").is_err());
    }
}
