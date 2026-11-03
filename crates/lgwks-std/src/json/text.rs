//! [`Text`]: the string a JSON [`Value`](super::Value) and a [`Map`](super::Map)
//! key hold, stored inline up to 22 bytes and on the heap past
//! it, so the short keys and words that make up most documents cost no
//! allocation (#831).

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

use serde::de::{self, Deserialize, Deserializer, Visitor};
use serde::ser::{Serialize, Serializer};

/// The longest text kept inline, in bytes. Twenty-two bytes plus the length
/// byte and the discriminant fill the 24 bytes a heap pointer and its length
/// already take, so inline storage costs no width over `Box<str>`.
pub(crate) const INLINE_CAPACITY: usize = 22;

/// Where a [`Text`]'s bytes live.
#[derive(Clone)]
enum Storage {
    /// At most [`INLINE_CAPACITY`] bytes, copied whole from a `&str`.
    Inline {
        /// How many of `bytes` are the text.
        len: u8,
        /// The text's bytes, then zeros.
        bytes: [u8; INLINE_CAPACITY],
    },
    /// Longer text, one allocation of exactly its length.
    Heap(Box<str>),
}

/// A JSON string: inline up to 22 bytes, heap past it.
///
/// It reads as a `&str` through [`Text::as_str`], `Deref`, `AsRef` and
/// `Borrow`, compares and orders as that `&str` does, and converts from and
/// into `String`.
#[derive(Clone)]
pub struct Text {
    /// The bytes, inline or boxed.
    storage: Storage,
}

impl Text {
    /// The empty text, inline.
    const fn new() -> Self {
        Self {
            storage: Storage::Inline {
                len: 0,
                bytes: [0; INLINE_CAPACITY],
            },
        }
    }

    /// The text as a string slice.
    ///
    /// Inline bytes are only ever written whole from a `&str` cut at its own
    /// end, so they are one valid UTF-8 run: the first chunk of their UTF-8
    /// reading is the whole text, and only the empty text has no chunk.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self.storage {
            Storage::Inline { .. } => match self.as_bytes().utf8_chunks().next() {
                Some(chunk) => chunk.valid(),
                None => "",
            },
            Storage::Heap(ref text) => text,
        }
    }

    /// The text's UTF-8 bytes, read without validation; equality, order and
    /// hashing go through these, so they agree with the `&str` reading.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self.storage {
            Storage::Inline { len, ref bytes } => match bytes.get(..usize::from(len)) {
                Some(text) => text,
                None => &[],
            },
            Storage::Heap(ref text) => text.as_bytes(),
        }
    }

    /// Whether the text is stored inline, with no allocation.
    #[cfg(test)]
    const fn is_inline(&self) -> bool {
        matches!(self.storage, Storage::Inline { .. })
    }

    /// Copies `text` inline when it fits, or returns `None`.
    fn inline(text: &str) -> Option<Self> {
        if text.len() > INLINE_CAPACITY {
            return None;
        }
        let len = u8::try_from(text.len()).ok()?;
        let mut bytes = [0; INLINE_CAPACITY];
        for (slot, byte) in bytes.iter_mut().zip(text.bytes()) {
            *slot = byte;
        }
        Some(Self {
            storage: Storage::Inline { len, bytes },
        })
    }

    /// Boxes `text` on the heap.
    fn heap(text: Box<str>) -> Self {
        Self {
            storage: Storage::Heap(text),
        }
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl From<&str> for Text {
    fn from(text: &str) -> Self {
        match Self::inline(text) {
            Some(inline) => inline,
            None => Self::heap(Box::from(text)),
        }
    }
}

impl From<String> for Text {
    fn from(text: String) -> Self {
        match Self::inline(&text) {
            Some(inline) => inline,
            None => Self::heap(text.into_boxed_str()),
        }
    }
}

impl From<&String> for Text {
    fn from(text: &String) -> Self {
        Self::from(text.as_str())
    }
}

impl From<Box<str>> for Text {
    fn from(text: Box<str>) -> Self {
        match Self::inline(&text) {
            Some(inline) => inline,
            None => Self::heap(text),
        }
    }
}

impl From<char> for Text {
    fn from(character: char) -> Self {
        let mut buffer = [0; 4];
        Self::from(&*character.encode_utf8(&mut buffer))
    }
}

impl From<Text> for String {
    fn from(text: Text) -> Self {
        match text.storage {
            Storage::Heap(boxed) => boxed.into_string(),
            Storage::Inline { .. } => Self::from(text.as_str()),
        }
    }
}

impl From<&Text> for String {
    fn from(text: &Text) -> Self {
        Self::from(text.as_str())
    }
}

impl Deref for Text {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Text {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Text {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl PartialEq for Text {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for Text {}

impl PartialOrd for Text {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Text {
    /// Byte order, which for UTF-8 is the scalar order `str` sorts by.
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_bytes().cmp(other.as_bytes())
    }
}

impl Hash for Text {
    /// Hashes as the `str` it reads as (its bytes, then `0xff`), so a map
    /// keyed by `Text` can be searched by `&str` through `Borrow`.
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.as_bytes());
        state.write_u8(0xff);
    }
}

impl PartialEq<str> for Text {
    fn eq(&self, other: &str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<&str> for Text {
    fn eq(&self, other: &&str) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<String> for Text {
    fn eq(&self, other: &String) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<Text> for str {
    fn eq(&self, other: &Text) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<Text> for &str {
    fn eq(&self, other: &Text) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl PartialEq<Text> for String {
    fn eq(&self, other: &Text) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for Text {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for Text {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Reads a JSON string into a [`Text`], copying a borrowed or transient string
/// inline when it fits and keeping an owned one's allocation when it does not.
struct TextVisitor;

impl Visitor<'_> for TextVisitor {
    type Value = Text;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Text, E> {
        Ok(Text::from(text))
    }

    fn visit_string<E: de::Error>(self, text: String) -> Result<Text, E> {
        Ok(Text::from(text))
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(TextVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{INLINE_CAPACITY, Text};

    /// Requirement: text at the inline capacity costs no allocation and one byte
    /// past it moves to the heap, and both read back as the `&str` they came
    /// from (#831).
    #[test]
    fn the_inline_boundary_is_exact_and_both_sides_read_back() {
        let at = "a".repeat(INLINE_CAPACITY);
        let past = "a".repeat(INLINE_CAPACITY.saturating_add(1));
        let inline = Text::from(at.as_str());
        let heap = Text::from(past.as_str());
        assert!(inline.is_inline(), "22 bytes fit inline");
        assert!(!heap.is_inline(), "23 bytes do not");
        assert_eq!(inline.as_str(), at, "inline text reads back whole");
        assert_eq!(heap.as_str(), past, "heap text reads back whole");
        assert_eq!(String::from(heap), past, "heap text gives its String back");
    }

    /// Requirement: a multi-byte character is never split by the inline cut;
    /// the whole text decides where it is stored.
    #[test]
    fn multi_byte_text_is_stored_whole_on_either_side_of_the_boundary() {
        let seventeen = "ééééééé€";
        let text = Text::from(seventeen);
        assert_eq!(
            seventeen.len(),
            17,
            "seven two-byte and one three-byte scalar"
        );
        assert!(text.is_inline(), "17 bytes fit inline");
        assert_eq!(text.as_str(), seventeen, "multi-byte text reads back whole");
        let wide = "€€€€€€€€";
        assert_eq!(wide.len(), 24, "eight three-byte scalars");
        assert_eq!(
            Text::from(wide).as_str(),
            wide,
            "past the cut it is boxed whole"
        );
        assert_eq!(Text::new().as_str(), "", "the empty text reads as empty");
    }
}
