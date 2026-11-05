//! The direct reader behind `str::parse::<Value>()`: one pass over the bytes
//! into the tree, with no serde visitor between the text and the nodes (#831).
//!
//! It reads the plain shapes a document is made of (literals, unescaped
//! strings, integers that fit 64 bits, arrays and objects) itself, and hands
//! each token it does not read plainly (an escaped string, a fraction, an
//! exponent, an integer past 64 bits) to `serde_json`, so every value it
//! returns is the value `serde_json` would. Any text it cannot read is read
//! again by `serde_json`, whose error is the one returned, so a refusal keeps
//! `serde_json`'s message, line and column.

use super::{Map, Number, Text, Value};

/// The deepest nesting read directly: `serde_json` reads 127 nested containers
/// and refuses the 128th, so a deeper document falls through and is refused
/// there with its error.
const DEPTH_LIMIT: usize = 127;

/// The members an object reserves before its first, as the serde reader does.
const FIRST_MEMBERS: usize = 8;

/// Reads `text` as one JSON document, or `None` when the direct reader leaves
/// it to `serde_json` (malformed, or deeper than [`DEPTH_LIMIT`]).
pub(super) fn document(text: &str) -> Option<Value> {
    let mut reader = Reader { text, at: 0 };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    (reader.at == text.len()).then_some(value)
}

/// A cursor over the document.
struct Reader<'text> {
    /// The whole document.
    text: &'text str,
    /// The byte offset of the next unread byte.
    at: usize,
}

impl Reader<'_> {
    /// The next unread byte.
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    /// Steps past one byte.
    const fn bump(&mut self) {
        self.at = self.at.saturating_add(1);
    }

    /// Steps past the four whitespace bytes RFC 8259 §2 allows.
    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.bump();
        }
    }

    /// Steps past `byte` when it is next, and says whether it was.
    fn eat(&mut self, byte: u8) -> bool {
        let next = self.peek() == Some(byte);
        if next {
            self.bump();
        }
        next
    }

    /// The value at the cursor, `depth` containers deep.
    fn value(&mut self, depth: usize) -> Option<Value> {
        match self.peek()? {
            b'n' => self.literal("null", Value::Null),
            b't' => self.literal("true", Value::Bool(true)),
            b'f' => self.literal("false", Value::Bool(false)),
            b'"' => self.string().map(Value::String),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number().map(Value::Number),
            _ => None,
        }
    }

    /// `word` at the cursor, read as `value`.
    fn literal(&mut self, word: &str, value: Value) -> Option<Value> {
        let end = self.at.checked_add(word.len())?;
        (self.text.get(self.at..end)? == word).then(|| {
            self.at = end;
            value
        })
    }

    /// The string at the cursor, its opening quote next.
    fn string(&mut self) -> Option<Text> {
        let open = self.at;
        self.bump();
        let start = self.at;
        loop {
            match self.peek()? {
                b'"' => {
                    // Both quotes are ASCII, so the run between them is cut on
                    // character boundaries and is valid UTF-8 as `str`.
                    let run = self.text.get(start..self.at)?;
                    self.bump();
                    return Some(Text::from(run));
                }
                b'\\' => return self.escaped_string(open),
                // RFC 8259 §7: a raw control must be escaped.
                0x00..=0x1f => return None,
                _ => self.bump(),
            }
        }
    }

    /// The string opened at `open` that holds an escape, decoded by
    /// `serde_json` so its escapes and surrogates are read exactly as there.
    fn escaped_string(&mut self, open: usize) -> Option<Text> {
        loop {
            match self.peek()? {
                b'"' => {
                    self.bump();
                    let token = self.text.get(open..self.at)?;
                    return serde_json::from_str::<String>(token).ok().map(Text::from);
                }
                b'\\' => {
                    self.bump();
                    self.bump();
                }
                _ => self.bump(),
            }
        }
    }

    /// The number at the cursor: a 64-bit integer read here, any other number
    /// read by `serde_json` so its fractions and overflow match.
    fn number(&mut self) -> Option<Number> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.bump();
        }
        let token = self.text.get(start..self.at)?;
        let (negative, digits) = match token.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, token),
        };
        let plain = !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (digits.len() == 1 || !digits.starts_with('0'))
            && !(negative && digits == "0");
        if plain {
            let integer = if negative {
                token.parse::<i64>().ok().map(Number::from)
            } else {
                token.parse::<u64>().ok().map(Number::from)
            };
            if integer.is_some() {
                return integer;
            }
        }
        serde_json::from_str::<Number>(token).ok()
    }

    /// The array at the cursor, `depth` containers deep.
    fn array(&mut self, depth: usize) -> Option<Value> {
        let inner = depth.checked_add(1).filter(|level| *level <= DEPTH_LIMIT)?;
        self.bump();
        self.skip_space();
        let mut items = Vec::new();
        if self.eat(b']') {
            return Some(Value::Array(items));
        }
        loop {
            items.push(self.value(inner)?);
            self.skip_space();
            if self.eat(b']') {
                return Some(Value::Array(items));
            }
            if !self.eat(b',') {
                return None;
            }
            self.skip_space();
        }
    }

    /// The object at the cursor, `depth` containers deep.
    fn object(&mut self, depth: usize) -> Option<Value> {
        let inner = depth.checked_add(1).filter(|level| *level <= DEPTH_LIMIT)?;
        self.bump();
        self.skip_space();
        let mut entries = Vec::with_capacity(FIRST_MEMBERS);
        if self.eat(b'}') {
            return Some(Value::Object(Map::from_members(entries)));
        }
        loop {
            if self.peek()? != b'"' {
                return None;
            }
            let key = self.string()?;
            self.skip_space();
            if !self.eat(b':') {
                return None;
            }
            self.skip_space();
            entries.push((key, self.value(inner)?));
            self.skip_space();
            if self.eat(b'}') {
                return Some(Value::Object(Map::from_members(entries)));
            }
            if !self.eat(b',') {
                return None;
            }
            self.skip_space();
        }
    }
}
