//! `Debug` for the builders whose members can't derive it.

/// Implements `Debug` for a `bon` builder as its type name, without its members: they hold
/// callbacks, trait objects or keys.
macro_rules! opaque_builder_debug {
    ($builder:ident, $state:ident) => {
        impl<S: $state::State> ::std::fmt::Debug for $builder<S> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.debug_struct(stringify!($builder)).finish_non_exhaustive()
            }
        }
    };
}
