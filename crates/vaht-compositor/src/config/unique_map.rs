//! Reading a TOML table into a map whose keys are parsed values, refusing
//! two keys that parse to the same value.
//!
//! Binding keys are written in several equivalent ways (`Super+T`,
//! `Mod4+t`). A plain map would keep whichever came last and silently drop
//! the other; [`deserialize`] reports the duplicate instead.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fmt;
use std::hash::Hash;
use std::marker::PhantomData;

use serde::de::{self, Deserializer, MapAccess, Visitor};

/// Reads a table into a map, refusing two keys that mean the same (such as
/// `Super+T` and `Mod4+t`).
pub(crate) fn deserialize<'de, D, Key, Value>(
    deserializer: D,
) -> Result<HashMap<Key, Value>, D::Error>
where
    D: Deserializer<'de>,
    Key: serde::Deserialize<'de> + Eq + Hash + fmt::Display,
    Value: serde::Deserialize<'de>,
{
    /// Reads the map.
    struct UniqueMap<Key, Value>(PhantomData<(Key, Value)>);

    impl<'de, Key, Value> Visitor<'de> for UniqueMap<Key, Value>
    where
        Key: serde::Deserialize<'de> + Eq + Hash + fmt::Display,
        Value: serde::Deserialize<'de>,
    {
        type Value = HashMap<Key, Value>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a table of bindings")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut bindings = HashMap::new();
            while let Some((key, value)) = map.next_entry::<Key, Value>()? {
                match bindings.entry(key) {
                    Entry::Occupied(occupied) => {
                        return Err(de::Error::custom(format!(
                            "`{}` is bound twice",
                            occupied.key()
                        )));
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(value);
                    }
                }
            }
            Ok(bindings)
        }
    }

    deserializer.deserialize_map(UniqueMap(PhantomData))
}
