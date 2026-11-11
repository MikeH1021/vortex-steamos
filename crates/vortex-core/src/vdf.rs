//! Parser for the text KeyValues files Steam writes.
//!
//! Escape sequences stay off. Steam stores Windows paths with literal
//! backslashes, and a backslash must not swallow the next character.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    String(String),
    Object(Vec<(String, Value)>),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(value) => Some(value),
            Value::Object(_) => None,
        }
    }

    pub fn get_ignore_ascii_case(&self, key: &str) -> Option<&Value> {
        let Value::Object(entries) = self else {
            return None;
        };
        entries
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    }

    pub fn dig<'a>(&'a self, path: &[&str]) -> Option<&'a Value> {
        let mut current = self;
        for (index, key) in path.iter().enumerate() {
            let next = current.get_ignore_ascii_case(key)?;
            if index + 1 == path.len() {
                return Some(next);
            }
            current = next;
        }
        Some(current)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub root_key: String,
    pub root: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at line {}, column {}",
            self.message, self.line, self.column
        )
    }
}

impl std::error::Error for ParseError {}

struct Parser<'a> {
    text: &'a str,
    index: usize,
    line: usize,
    column: usize,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        let mut parser = Self {
            text,
            index: 0,
            line: 1,
            column: 1,
        };
        if text.starts_with('\u{feff}') {
            parser.index = '\u{feff}'.len_utf8();
        }
        parser
    }

    fn error(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            message: message.into(),
            line: self.line,
            column: self.column,
        }
    }

    fn peek(&self) -> Option<char> {
        self.text[self.index..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(ch) if ch.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.text[self.index..].starts_with("//") => {
                    while let Some(ch) = self.peek() {
                        self.bump();
                        if ch == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, ParseError> {
        self.skip_trivia();
        match self.peek() {
            Some('"') => self.parse_quoted(),
            Some(ch) if is_bare_start(ch) => self.parse_bare(),
            Some(ch) => Err(self.error(format!("expected a string, found '{ch}'"))),
            None => Err(self.error("expected a string, found the end of the file")),
        }
    }

    fn parse_quoted(&mut self) -> Result<String, ParseError> {
        self.bump();
        let mut out = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(out),
                Some(ch) => out.push(ch),
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn parse_bare(&mut self) -> Result<String, ParseError> {
        let mut out = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_whitespace() || matches!(ch, '"' | '{' | '}') {
                break;
            }
            out.push(ch);
            self.bump();
        }
        if out.is_empty() {
            return Err(self.error("expected a string"));
        }
        Ok(out)
    }

    fn parse_value(&mut self) -> Result<Value, ParseError> {
        self.skip_trivia();
        if self.peek() == Some('{') {
            self.bump();
            let mut entries = Vec::new();
            loop {
                self.skip_trivia();
                match self.peek() {
                    Some('}') => {
                        self.bump();
                        return Ok(Value::Object(entries));
                    }
                    None => return Err(self.error("unterminated object")),
                    _ => {
                        let key = self.parse_string()?;
                        let value = self.parse_value()?;
                        entries.push((key, value));
                    }
                }
            }
        } else {
            Ok(Value::String(self.parse_string()?))
        }
    }
}

fn is_bare_start(ch: char) -> bool {
    !ch.is_whitespace() && !matches!(ch, '"' | '{' | '}')
}

/// Parse one KeyValues document. The returned root is the value of the first key.
pub fn parse(text: &str) -> Result<Document, ParseError> {
    let mut parser = Parser::new(text);
    parser.skip_trivia();
    if parser.peek().is_none() {
        return Err(parser.error("empty KeyValues document"));
    }
    let root_key = parser.parse_string()?;
    let root = parser.parse_value()?;
    parser.skip_trivia();
    if parser.peek().is_some() {
        return Err(parser.error("unexpected text after the root object"));
    }
    Ok(Document { root_key, root })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_windows_backslashes() {
        let doc = parse("\"libraryfolders\"\n{\n\"path\" \"D:\\\\SteamLibrary\"\n}\n").unwrap();
        // Escapes are off, so each backslash in the source is a real character.
        // The Rust string above has two backslashes in the file text.
        let path = doc.root.get_ignore_ascii_case("path").unwrap().as_str();
        assert_eq!(path, Some("D:\\\\SteamLibrary"));
    }

    #[test]
    fn parses_nested_objects_and_comments() {
        let text = r#"
            // libraries
            "libraryfolders"
            {
                "0"
                {
                    "path" "/home/deck/.local/share/Steam"
                    "apps"
                    {
                        "489830" "120000"
                    }
                }
            }
        "#;
        let doc = parse(text).unwrap();
        assert_eq!(doc.root_key, "libraryfolders");
        let app = doc
            .root
            .dig(&["0", "apps", "489830"])
            .and_then(Value::as_str);
        assert_eq!(app, Some("120000"));
    }

    #[test]
    fn reports_the_line_of_an_unclosed_object() {
        let err = parse("\"root\"\n{\n\"key\" \"value\"\n").unwrap_err();
        assert!(err.line >= 2);
        assert!(err.message.contains("unterminated"));
    }
}
