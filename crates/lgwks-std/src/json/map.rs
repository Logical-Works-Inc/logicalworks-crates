//! [`Map`]: a JSON object's members in one vector kept sorted by key, so a
//! parsed object of up to eight members costs one allocation, a lookup is a
//! binary search, and keys serialize in the order the `BTreeMap`-backed map
//! this replaces wrote them (#831).

use std::fmt;
use std::ops::Index;

use serde::ser::{Serialize, SerializeMap, Serializer};

use super::{Text, Value};

/// A JSON object: unique [`Text`] keys in ascending byte order, each with its
/// [`Value`]. Inserting a key that is present replaces its value, as reading a
/// document with a repeated key keeps the last.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Map {
    /// The members, sorted by key with no key twice.
    entries: Vec<(Text, Value)>,
}

impl Map {
    /// An empty object; allocates nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Builds an object from members in any order, the last of a repeated key
    /// winning; sorts once rather than inserting one at a time.
    pub(crate) fn from_members(mut entries: Vec<(Text, Value)>) -> Self {
        normalize(&mut entries);
        Self { entries }
    }

    /// Where `key` is, or where it would go.
    fn position(&self, key: &str) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|entry| entry.0.as_bytes().cmp(key.as_bytes()))
    }

    /// How many members the object has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the object has no member.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        let index = self.position(key).ok()?;
        self.entries.get(index).map(|entry| &entry.1)
    }

    /// The value under `key`, mutably.
    #[must_use]
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        let index = self.position(key).ok()?;
        self.entries.get_mut(index).map(|entry| &mut entry.1)
    }

    /// Whether `key` is a member.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_ok()
    }

    /// Sets `key` to `value` and returns the value it replaced.
    pub fn insert(&mut self, key: impl Into<Text>, value: Value) -> Option<Value> {
        let key = key.into();
        match self.position(&key) {
            Ok(index) => self
                .entries
                .get_mut(index)
                .map(|entry| std::mem::replace(&mut entry.1, value)),
            Err(index) => {
                self.entries.insert(index, (key, value));
                None
            }
        }
    }

    /// Removes `key` and returns its value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.position(key).ok()?;
        self.entries
            .drain(index..=index)
            .next()
            .map(|entry| entry.1)
    }

    /// The members in key order.
    #[must_use]
    pub fn iter(&self) -> Iter<'_> {
        Iter {
            inner: self.entries.iter(),
        }
    }

    /// Every member name, ascending by byte, each once.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|entry| entry.0.as_str())
    }

    /// The values in key order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|entry| &entry.1)
    }
}

/// Sorts `entries` by key and keeps the last value of a repeated key, so a
/// run of members in any order becomes a map's sorted, unique vector.
fn normalize(entries: &mut Vec<(Text, Value)>) {
    let sorted = entries
        .windows(2)
        .all(|pair| matches!(*pair, [(ref left, _), (ref right, _)] if left < right));
    if !sorted {
        // Stable, so repeated keys stay in arrival order; `dedup_by` hands
        // each later duplicate with the kept earlier one, and the later
        // value moves into the kept slot so the last one wins.
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries.dedup_by(|later, kept| {
            let repeated = later.0 == kept.0;
            if repeated {
                std::mem::swap(&mut later.1, &mut kept.1);
            }
            repeated
        });
    }
}

/// The members of a [`Map`] in key order, borrowed.
#[derive(Clone)]
pub struct Iter<'map> {
    /// The sorted members.
    inner: std::slice::Iter<'map, (Text, Value)>,
}

impl<'map> Iterator for Iter<'map> {
    type Item = (&'map str, &'map Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|entry| (entry.0.as_str(), &entry.1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl DoubleEndedIterator for Iter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner
            .next_back()
            .map(|entry| (entry.0.as_str(), &entry.1))
    }
}

impl ExactSizeIterator for Iter<'_> {}

impl<'map> IntoIterator for &'map Map {
    type Item = (&'map str, &'map Value);
    type IntoIter = Iter<'map>;

    fn into_iter(self) -> Iter<'map> {
        self.iter()
    }
}

/// The members of a [`Map`] in key order, owned, each key as a `String`.
pub struct IntoIter {
    /// The sorted members.
    inner: std::vec::IntoIter<(Text, Value)>,
}

impl Iterator for IntoIter {
    type Item = (String, Value);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner
            .next()
            .map(|(key, value)| (String::from(key), value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = IntoIter;

    fn into_iter(self) -> IntoIter {
        IntoIter {
            inner: self.entries.into_iter(),
        }
    }
}

impl<K: Into<Text>, V: Into<Value>> FromIterator<(K, V)> for Map {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(members: I) -> Self {
        Self::from_members(
            members
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }
}

impl<K: Into<Text>, V: Into<Value>> Extend<(K, V)> for Map {
    /// Appends every member after the present ones and sorts once, so a
    /// later member of a repeated key wins as it would through
    /// [`Map::insert`], without shifting the vector once per member. The
    /// stable sort finds the sorted run already present and merges the
    /// appended run into it.
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, members: I) {
        self.entries.extend(
            members
                .into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        normalize(&mut self.entries);
    }
}

impl Index<&str> for Map {
    type Output = Value;

    /// The value under `key`, or `Value::Null` when it is absent.
    fn index(&self, key: &str) -> &Value {
        match self.get(key) {
            Some(value) => value,
            None => &super::value::NULL,
        }
    }
}

impl fmt::Debug for Map {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_map().entries(self.iter()).finish()
    }
}

impl Serialize for Map {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut members = serializer.serialize_map(Some(self.entries.len()))?;
        for entry in &self.entries {
            members.serialize_entry(entry.0.as_str(), &entry.1)?;
        }
        members.end()
    }
}
