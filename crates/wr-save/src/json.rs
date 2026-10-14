//! A lossless JSON tree for save documents.
//!
//! Unlike `serde_json::Value` this keeps object key order, duplicate keys and every
//! number exactly as written (its lexeme), so nothing about a document is lost before the
//! typed decoder decides what each value means. It accepts what Newtonsoft's
//! `JsonTextReader` writes and reads: standard JSON, a leading UTF-8 BOM, and the bare
//! `NaN`, `Infinity` and `-Infinity` literals.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Json {
    Null,
    Bool(bool),
    /// A number as written, or `NaN` / `Infinity` / `-Infinity`.
    Num(Box<str>),
    Str(Box<str>),
    Arr(Vec<Json>),
    /// Properties in document order; duplicates are kept.
    Obj(Vec<(Box<str>, Json)>),
}

/// Deepest nesting accepted (Newtonsoft's default `MaxDepth` is unlimited for the
/// serializer; the deepest save document seen so far is far below this).
pub const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub what: &'static str,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.what, self.offset)
    }
}

impl std::error::Error for ParseError {}

/// A parsed document.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub root: Json,
    /// The text started with a UTF-8 byte order mark.
    pub bom: bool,
}

const BOM: &[u8] = b"\xef\xbb\xbf";

/// Whether the bytes are empty or only a BOM and whitespace (`tbm.json` outside
/// turn-based combat).
#[must_use]
pub fn is_blank(bytes: &[u8]) -> bool {
    bytes.strip_prefix(BOM).unwrap_or(bytes).iter().all(u8::is_ascii_whitespace)
}

/// Parses one JSON document.
///
/// # Errors
///
/// Invalid UTF-8, invalid JSON, trailing content or nesting deeper than [`MAX_DEPTH`].
pub fn parse(bytes: &[u8]) -> Result<Document, ParseError> {
    let (bom, body) = match bytes.strip_prefix(BOM) {
        Some(b) => (true, b),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|e| ParseError {
        offset: e.valid_up_to() + if bom { 3 } else { 0 },
        what: "invalid UTF-8",
    })?;
    let mut p = Parser { s: text.as_bytes(), text, i: 0, base: if bom { 3 } else { 0 } };
    p.ws();
    let root = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("trailing content"));
    }
    Ok(Document { root, bom })
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    base: usize,
}

impl Parser<'_> {
    fn err(&self, what: &'static str) -> ParseError {
        ParseError { offset: self.base + self.i, what }
    }

    fn ws(&mut self) {
        while let Some(&c) = self.s.get(self.i) {
            if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn lit(&mut self, word: &[u8]) -> bool {
        if self.s[self.i..].starts_with(word) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, ParseError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.lit(b"true") => Ok(Json::Bool(true)),
            Some(b'f') if self.lit(b"false") => Ok(Json::Bool(false)),
            Some(b'n') if self.lit(b"null") => Ok(Json::Null),
            Some(b'N') if self.lit(b"NaN") => Ok(Json::Num("NaN".into())),
            Some(b'I') if self.lit(b"Infinity") => Ok(Json::Num("Infinity".into())),
            Some(b'-') if self.s[self.i..].starts_with(b"-Infinity") => {
                self.i += 9;
                Ok(Json::Num("-Infinity".into()))
            }
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.i += 1;
        let mut props = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(Json::Obj(props));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(self.err("expected property name"));
            }
            let k = self.string()?;
            self.ws();
            if self.s.get(self.i) != Some(&b':') {
                return Err(self.err("expected ':'"));
            }
            self.i += 1;
            self.ws();
            let v = self.value(depth + 1)?;
            props.push((k, v));
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(props));
                }
                _ => return Err(self.err("expected ',' or '}'")),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, ParseError> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.s.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.s.get(self.i) {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("expected ',' or ']'")),
            }
        }
    }

    fn number(&mut self) -> Result<Json, ParseError> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        let digits = |p: &mut Self| {
            let s = p.i;
            while p.s.get(p.i).is_some_and(u8::is_ascii_digit) {
                p.i += 1;
            }
            p.i > s
        };
        if !digits(self) {
            return Err(self.err("bad number"));
        }
        if self.s.get(self.i) == Some(&b'.') {
            self.i += 1;
            if !digits(self) {
                return Err(self.err("bad number"));
            }
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(self.err("bad number"));
            }
        }
        Ok(Json::Num(self.text[start..self.i].into()))
    }

    fn string(&mut self) -> Result<Box<str>, ParseError> {
        self.i += 1;
        let start = self.i;
        // Fast path: no escapes.
        while let Some(&c) = self.s.get(self.i) {
            match c {
                b'"' => {
                    let s = &self.text[start..self.i];
                    self.i += 1;
                    return Ok(s.into());
                }
                b'\\' => break,
                0..=0x1f => return Err(self.err("control character in string")),
                _ => self.i += 1,
            }
        }
        let mut out = String::from(&self.text[start..self.i]);
        loop {
            let Some(&c) = self.s.get(self.i) else {
                return Err(self.err("unterminated string"));
            };
            match c {
                b'"' => {
                    self.i += 1;
                    return Ok(out.into_boxed_str());
                }
                b'\\' => {
                    self.i += 1;
                    let e = *self.s.get(self.i).ok_or_else(|| self.err("bad escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            let ch = if (0xd800..0xdc00).contains(&hi)
                                && self.s[self.i..].starts_with(b"\\u")
                            {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xdc00..0xe000).contains(&lo) {
                                    char::from_u32(0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00))
                                } else {
                                    self.i = save;
                                    None
                                }
                            } else {
                                char::from_u32(hi)
                            };
                            // Lone surrogates can't be held in a Rust string; .NET strings
                            // can. Not seen in saves; refuse rather than alter.
                            out.push(ch.ok_or_else(|| self.err("lone surrogate escape"))?);
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                0..=0x1f => return Err(self.err("control character in string")),
                _ => {
                    // Copy a run of ordinary characters.
                    let s = self.i;
                    while let Some(&c) = self.s.get(self.i) {
                        if c == b'"' || c == b'\\' || c < 0x20 {
                            break;
                        }
                        self.i += 1;
                    }
                    out.push_str(&self.text[s..self.i]);
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("bad \\u escape"))?;
        let h = std::str::from_utf8(h).map_err(|_| self.err("bad \\u escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        Ok(v)
    }
}

/// Writes a string literal with Newtonsoft's default escaping (control characters,
/// quote, backslash, U+0085, U+2028, U+2029).
pub fn write_str(s: &str, out: &mut String) {
    out.push('"');
    let mut last = 0;
    for (i, c) in s.char_indices() {
        let esc: Option<&str> = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            _ => None,
        };
        let needs_u = esc.is_none()
            && (u32::from(c) < 0x20 || matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}'));
        if esc.is_some() || needs_u {
            out.push_str(&s[last..i]);
            match esc {
                Some(e) => out.push_str(e),
                None => {
                    let _ = write!(out, "\\u{:04x}", u32::from(c));
                }
            }
            last = i + c.len_utf8();
        }
    }
    out.push_str(&s[last..]);
    out.push('"');
}

/// Writes compact JSON (Newtonsoft's `Formatting.None`).
pub fn write(j: &Json, out: &mut String) {
    match j {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(n),
        Json::Str(s) => write_str(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (k, it) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write(it, out);
            }
            out.push(']');
        }
        Json::Obj(props) => {
            out.push('{');
            for (k, (key, v)) in props.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                write_str(key, out);
                out.push(':');
                write(v, out);
            }
            out.push('}');
        }
    }
}

impl Json {
    /// Kind name, for reasons and reports.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Num(n) if is_int_lexeme(n) => "integer",
            Self::Num(_) => "float",
            Self::Str(_) => "string",
            Self::Arr(_) => "array",
            Self::Obj(_) => "object",
        }
    }

    /// Number of values in the tree (this one included).
    #[must_use]
    pub fn count(&self) -> u64 {
        match self {
            Self::Arr(items) => 1 + items.iter().map(Self::count).sum::<u64>(),
            Self::Obj(props) => 1 + props.iter().map(|(_, v)| v.count()).sum::<u64>(),
            _ => 1,
        }
    }

    /// `JObject[name]`: the first property with exactly this name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Self> {
        match self {
            Self::Obj(props) => props.iter().find(|(k, _)| &**k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Compact text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        write(self, &mut s);
        s
    }
}

/// An integer literal: optional `-`, digits, no fraction or exponent.
#[must_use]
pub fn is_int_lexeme(n: &str) -> bool {
    let d = n.strip_prefix('-').unwrap_or(n);
    !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_duplicates_and_lexemes() {
        let d = parse(b"\xef\xbb\xbf{\"b\":1.50,\"a\":-0,\"b\":1e-05,\"n\":NaN,\"i\":-Infinity}")
            .unwrap();
        assert!(d.bom);
        let Json::Obj(p) = &d.root else { panic!() };
        let keys: Vec<&str> = p.iter().map(|(k, _)| &**k).collect();
        assert_eq!(keys, ["b", "a", "b", "n", "i"]);
        assert_eq!(p[0].1, Json::Num("1.50".into()));
        assert_eq!(p[1].1, Json::Num("-0".into()));
        assert_eq!(p[2].1, Json::Num("1e-05".into()));
        assert_eq!(p[3].1, Json::Num("NaN".into()));
        assert_eq!(p[4].1, Json::Num("-Infinity".into()));
        assert_eq!(d.root.get("b"), Some(&Json::Num("1.50".into())));
    }

    #[test]
    fn strings_and_escapes() {
        let d =
            parse("[\"a\\\"b\\\\c\\/\\u00e9\\ud83d\\ude00\\n\", \"plain\"]".as_bytes()).unwrap();
        let Json::Arr(a) = &d.root else { panic!() };
        assert_eq!(a[0], Json::Str("a\"b\\c/\u{e9}\u{1f600}\n".into()));
        let mut s = String::new();
        write(&d.root, &mut s);
        assert_eq!(parse(s.as_bytes()).unwrap().root, d.root);
        let mut e = String::new();
        write_str("\u{1}\u{2028}x", &mut e);
        assert_eq!(e, "\"\\u0001\\u2028x\"");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse(b"{\"a\":}").is_err());
        assert!(parse(b"[1,]").is_err());
        assert!(parse(b"01x").is_err());
        assert!(parse(b"{} x").is_err());
        assert!(parse(br#""\ud800""#).is_err());
        assert!(parse(&b"[".repeat(MAX_DEPTH + 2)).is_err());
    }

    #[test]
    fn blank_documents() {
        assert!(is_blank(b"\xef\xbb\xbf"));
        assert!(is_blank(b""));
        assert!(!is_blank(b"\xef\xbb\xbf{}"));
        assert!(parse(b"\xef\xbb\xbf").is_err());
    }
}
