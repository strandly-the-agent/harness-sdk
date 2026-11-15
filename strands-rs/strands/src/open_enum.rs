//! A string enum that keeps unknown wire values, for TS unions that accept any string at runtime.

/// Declares a `#[non_exhaustive]` string enum with an `Other` fallback.
///
/// The fallback payload is a private newtype built only through `From<&str>`/`From<String>`, which map
/// known strings to their variants, so `Other` never holds a known value.
macro_rules! open_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident / $unknown:ident {
            $( $(#[$vmeta:meta])* $variant:ident => $wire:literal, )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum $name {
            $( $(#[$vmeta])* $variant, )+
            /// A value this SDK version doesn't know, kept verbatim so it round-trips.
            Other($unknown),
        }

        #[doc = concat!("A [`", stringify!($name), "`] value this SDK version doesn't know.")]
        ///
        /// Built only through `From<&str>`/`From<String>` on the enum, so it never holds a known value.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $unknown(String);

        impl $unknown {
            /// The raw wire value.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl $name {
            /// The wire string.
            pub fn as_str(&self) -> &str {
                match self {
                    $( Self::$variant => $wire, )+
                    Self::Other(other) => other.as_str(),
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $( $wire => Self::$variant, )+
                    other => Self::Other($unknown(other.to_owned())),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match Self::from(value.as_str()) {
                    Self::Other(_) => Self::Other($unknown(value)),
                    known => known,
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
