//! The request an invocation belongs to (TS `agent/invocation-context.ts`).

use std::fmt;
use std::sync::Arc;

/// The request a call belongs to (TS `InvocationContext`).
///
/// Tools read it from [`ToolContext::invocation_context`](crate::ToolContext::invocation_context)
/// to run nested calls in the same request. Cloning is cheap and every clone is the same request.
#[derive(Clone, Default)]
pub struct InvocationContext {
    inner: Arc<InvocationContextInner>,
}

#[derive(Default)]
struct InvocationContextInner {}

impl InvocationContext {
    #[cfg_attr(not(test), expect(dead_code, reason = "created by the agent loop"))]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Whether both handles belong to the same request.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the agent loop"))]
    pub(crate) fn same_request(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl fmt::Debug for InvocationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvocationContext").finish_non_exhaustive()
    }
}
