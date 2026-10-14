//! Hooks: typed lifecycle events and the registry that dispatches them.

pub mod events;
mod registry;

pub use events::*;
pub use registry::{hook_order, HookCallback, HookCleanup, HookEvent, HookFn, HookRegistry};

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use crate::hooks::*;
    #[allow(unused_imports)]
    use crate::models::*;

    #[test]
    fn hooks_and_models_globs_do_not_collide() {
        let redaction = Redaction {
            user_message: "redacted".to_owned(),
        };
        assert_eq!(redaction.user_message, "redacted");
    }
}
