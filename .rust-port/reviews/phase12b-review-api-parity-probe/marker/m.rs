use std::{future::Future, pin::Pin, sync::Arc};
type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub struct Spec(pub u8);
pub struct Agent;
pub type AgentBuilder = Arc<dyn Fn(Spec) -> BoxFuture<Result<Agent, ()>> + Send + Sync>;
pub trait IntoAgentBuilder<M> {
    fn into_agent_builder(self) -> AgentBuilder;
}
impl<F, Fut> IntoAgentBuilder<(Fut,)> for F
where
    F: Fn(Spec) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Agent, ()>> + Send + 'static,
{
    fn into_agent_builder(self) -> AgentBuilder {
        Arc::new(move |spec| Box::pin(self(spec)))
    }
}
impl IntoAgentBuilder<()> for AgentBuilder {
    fn into_agent_builder(self) -> AgentBuilder {
        self
    }
}
pub fn builder<M>(b: impl IntoAgentBuilder<M>) -> AgentBuilder {
    b.into_agent_builder()
}
fn main() {
    let a = builder(|_s: Spec| async { Ok(Agent) });
    let _b = builder(a.clone());
    #[cfg(unannotated)]
    let _c = builder(|s| async move {
        let _ = s.0;
        Ok(Agent)
    });
}
