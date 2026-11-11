//! String unions that TS keeps open at runtime: known values get variants, anything else is kept.

/// Defines a `#[non_exhaustive]` enum over known string values plus an `Other` fallback.
///
/// The fallback payload is a newtype with a private field, so `Other` only ever holds an unknown
/// value: `From<&str>` maps known strings to their variants, and serde goes through `From`.
macro_rules! open_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident / $unknown:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            #[doc = concat!("A value this SDK version doesn't know, kept as written. See [`", stringify!($unknown), "`].")]
            Other($unknown),
        }

        #[doc = concat!("A [`", stringify!($name), "`] value that matches no known variant.")]
        ///
        /// Built only through `From<&str>`/`From<String>` or deserialization, so it never holds a
        /// known value.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $unknown(String);

        impl $unknown {
            /// The value as written.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl $name {
            /// The wire string for this value.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $value, )+
                    Self::Other(other) => other.as_str(),
                }
            }

            fn known(value: &str) -> Option<Self> {
                match value {
                    $( $value => Some(Self::$variant), )+
                    _ => None,
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::known(value).unwrap_or_else(|| Self::Other($unknown(value.to_owned())))
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::known(&value).unwrap_or(Self::Other($unknown(value)))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                String::deserialize(deserializer).map(Self::from)
            }
        }
    };
}

pub(crate) use open_enum;
