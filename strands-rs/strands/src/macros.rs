//! Crate-internal declarative macros.

/// Defines an open string union: the known TS literals as unit variants plus `Other`, whose payload
/// is a private newtype so a known value can never end up in `Other`.
macro_rules! open_string_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $unknown:ident {
            $( $variant:ident => $value:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $(
                #[doc = concat!("`\"", $value, "\"`")]
                $variant,
            )+
            /// A value this version doesn't know, kept as written so it round-trips.
            Other($unknown),
        }

        #[doc = concat!("A [`", stringify!($name), "`] string this version doesn't know.")]
        ///
        /// It is only built by `From<&str>`/`From<String>` and deserialization, which map known
        /// strings to their variants first, so it never holds a known value.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $unknown(String);

        impl $unknown {
            /// Returns the value as written.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl $name {
            /// Returns the wire string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $value, )+
                    Self::Other(other) => other.as_str(),
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $( $value => Self::$variant, )+
                    other => Self::Other($unknown(other.to_owned())),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $( $value => Self::$variant, )+
                    _ => Self::Other($unknown(value)),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                <String as serde::Deserialize>::deserialize(deserializer).map(Self::from)
            }
        }
    };
}

/// Implements `Serialize`/`Deserialize` for a block as the TS single-key wrapper
/// (`{"<key>": <inner>}`), delegating the inner shape to a `#[serde(remote = "...")]` mirror.
macro_rules! wrapped_serde {
    ($ty:ty, $def:ident, $key:literal) => {
        wrapped_serialize!($ty, $def, $key);
        wrapped_deserialize!($ty, $def, $key);
    };
}

/// The `Serialize` half of [`wrapped_serde!`].
macro_rules! wrapped_serialize {
    ($ty:ty, $def:ident, $key:literal) => {
        impl serde::Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                struct Inner<'a>(&'a $ty);
                impl serde::Serialize for Inner<'_> {
                    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                        $def::serialize(self.0, serializer)
                    }
                }
                let mut map = serde::Serializer::serialize_map(serializer, Some(1))?;
                serde::ser::SerializeMap::serialize_entry(&mut map, $key, &Inner(self))?;
                serde::ser::SerializeMap::end(map)
            }
        }
    };
}

/// The `Deserialize` half of [`wrapped_serde!`].
macro_rules! wrapped_deserialize {
    ($ty:ty, $def:ident, $key:literal) => {
        impl<'de> serde::Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                struct Inner($ty);
                impl<'de> serde::Deserialize<'de> for Inner {
                    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                        $def::deserialize(deserializer).map(Inner)
                    }
                }
                #[derive(serde::Deserialize)]
                struct Wrapper {
                    #[serde(rename = $key)]
                    inner: Inner,
                }
                <Wrapper as serde::Deserialize>::deserialize(deserializer).map(|wrapper| wrapper.inner.0)
            }
        }
    };
}
