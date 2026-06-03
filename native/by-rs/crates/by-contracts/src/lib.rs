#![forbid(unsafe_code)]

use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum EdnValue {
    Nil,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Keyword(String),
    Symbol(String),
    Instant(String),
    Uuid(String),
    Vector(Vec<EdnValue>),
    List(Vec<EdnValue>),
    Set(Vec<EdnValue>),
    Map(EdnMap),
    MapEntries(Vec<(EdnValue, EdnValue)>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdnMap {
    entries: BTreeMap<String, EdnValue>,
}

impl EdnMap {
    pub fn new(entries: BTreeMap<String, EdnValue>) -> Self {
        Self { entries }
    }

    pub fn get(&self, key: &str) -> Option<&EdnValue> {
        self.entries.get(key)
    }

    pub fn string(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(EdnValue::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn instant(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(EdnValue::Instant(value)) | Some(EdnValue::String(value)) => Some(value),
            _ => None,
        }
    }

    pub fn i64(&self, key: &str) -> Option<i64> {
        match self.get(key) {
            Some(EdnValue::Integer(value)) => Some(*value),
            _ => None,
        }
    }

    pub fn is_nil(&self, key: &str) -> bool {
        matches!(self.get(key), Some(EdnValue::Nil))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &EdnValue)> {
        self.entries.iter()
    }
}

pub fn parse_value(input: &str) -> Result<EdnValue> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    parser.skip_ws_and_comments();
    if !parser.is_eof() {
        bail!(
            "unexpected trailing EDN input at byte {}",
            parser.position()
        );
    }
    Ok(value)
}

pub fn parse_map(input: &str) -> Result<EdnMap> {
    match parse_value(input)? {
        EdnValue::Map(map) => Ok(map),
        _ => bail!("expected EDN map root"),
    }
}

struct Parser<'a> {
    input: &'a str,
    chars: Vec<char>,
    index: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input,
            chars: input.chars().collect(),
            index: 0,
        }
    }

    fn parse_value(&mut self) -> Result<EdnValue> {
        self.skip_ws_and_comments();
        let Some(ch) = self.peek() else {
            bail!("unexpected end of EDN input");
        };

        match ch {
            '{' => self.parse_map_value().map(EdnValue::Map),
            '[' => self.parse_vector().map(EdnValue::Vector),
            '(' => self.parse_list().map(EdnValue::List),
            '"' => self.parse_string().map(EdnValue::String),
            ':' => self.parse_keyword().map(EdnValue::Keyword),
            '#' => self.parse_dispatch(),
            _ => self.parse_token_value(),
        }
    }

    fn parse_map_value(&mut self) -> Result<EdnMap> {
        self.expect('{')?;
        let mut entries = BTreeMap::new();
        loop {
            self.skip_ws_and_comments();
            if self.consume_if('}') {
                break;
            }
            let key = self.parse_map_key()?;
            let value = self.parse_value()?;
            entries.insert(key, value);
        }
        Ok(EdnMap::new(entries))
    }

    fn parse_map_key(&mut self) -> Result<String> {
        self.skip_ws_and_comments();
        match self.peek() {
            Some(':') => self.parse_keyword(),
            Some('"') => self.parse_string(),
            Some(_) => bail!("expected EDN map key at byte {}", self.position()),
            None => bail!("unexpected end while parsing EDN map key"),
        }
    }

    fn parse_vector(&mut self) -> Result<Vec<EdnValue>> {
        self.expect('[')?;
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.consume_if(']') {
                break;
            }
            values.push(self.parse_value()?);
        }
        Ok(values)
    }

    fn parse_list(&mut self) -> Result<Vec<EdnValue>> {
        self.expect('(')?;
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.consume_if(')') {
                break;
            }
            values.push(self.parse_value()?);
        }
        Ok(values)
    }

    fn parse_set(&mut self) -> Result<Vec<EdnValue>> {
        let mut values = Vec::new();
        loop {
            self.skip_ws_and_comments();
            if self.consume_if('}') {
                break;
            }
            values.push(self.parse_value()?);
        }
        Ok(values)
    }

    fn parse_dispatch(&mut self) -> Result<EdnValue> {
        if self.consume_str("#inst") {
            self.skip_ws_and_comments();
            return self.parse_string().map(EdnValue::Instant);
        }
        if self.consume_str("#uuid") {
            self.skip_ws_and_comments();
            return self.parse_string().map(EdnValue::Uuid);
        }
        if self.consume_str("#mulog/flake") {
            return self.parse_value();
        }
        if self.consume_str("#{") {
            return self.parse_set().map(EdnValue::Set);
        }
        if self.consume_str("#_") {
            let _discarded = self.parse_value()?;
            return self.parse_value();
        }
        bail!("unsupported EDN dispatch tag at byte {}", self.position())
    }

    fn parse_string(&mut self) -> Result<String> {
        self.expect('"')?;
        let mut out = String::new();
        while let Some(ch) = self.next() {
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escaped = self
                        .next()
                        .ok_or_else(|| anyhow!("unterminated EDN string escape"))?;
                    match escaped {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        bail!("unterminated EDN string")
    }

    fn parse_keyword(&mut self) -> Result<String> {
        self.expect(':')?;
        let token = self.read_token();
        if token.is_empty() {
            bail!("empty EDN keyword at byte {}", self.position());
        }
        Ok(token)
    }

    fn parse_token_value(&mut self) -> Result<EdnValue> {
        let token = self.read_token();
        if token.is_empty() {
            bail!("expected EDN token at byte {}", self.position());
        }
        match token.as_str() {
            "nil" => Ok(EdnValue::Nil),
            "true" => Ok(EdnValue::Bool(true)),
            "false" => Ok(EdnValue::Bool(false)),
            _ => match token.parse::<i64>() {
                Ok(value) => Ok(EdnValue::Integer(value)),
                Err(_) => match token.parse::<f64>() {
                    Ok(value)
                        if value.is_finite()
                            && (token.contains('.')
                                || token.contains('e')
                                || token.contains('E')) =>
                    {
                        Ok(EdnValue::Float(value))
                    }
                    _ => Ok(EdnValue::Symbol(token)),
                },
            },
        }
    }

    fn read_token(&mut self) -> String {
        let mut token = String::new();
        while let Some(ch) = self.peek() {
            if is_delimiter(ch) {
                break;
            }
            token.push(ch);
            self.index += 1;
        }
        token
    }

    fn skip_ws_and_comments(&mut self) {
        loop {
            while matches!(self.peek(), Some(ch) if ch.is_whitespace() || ch == ',') {
                self.index += 1;
            }
            if self.peek() == Some(';') {
                while let Some(ch) = self.next() {
                    if ch == '\n' {
                        break;
                    }
                }
                continue;
            }
            break;
        }
    }

    fn expect(&mut self, expected: char) -> Result<()> {
        match self.next() {
            Some(actual) if actual == expected => Ok(()),
            Some(actual) => bail!(
                "expected '{}' at byte {}, found '{}'",
                expected,
                self.position(),
                actual
            ),
            None => bail!("expected '{}' at end of EDN input", expected),
        }
    }

    fn consume_if(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn consume_str(&mut self, expected: &str) -> bool {
        let expected_chars: Vec<char> = expected.chars().collect();
        if self.chars[self.index..].starts_with(&expected_chars) {
            self.index += expected_chars.len();
            true
        } else {
            false
        }
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        Some(ch)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn is_eof(&self) -> bool {
        self.index >= self.chars.len()
    }

    fn position(&self) -> usize {
        self.input
            .chars()
            .take(self.index)
            .map(char::len_utf8)
            .sum()
    }
}

fn is_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, ',' | '{' | '}' | '[' | ']' | '(' | ')' | '"')
}
