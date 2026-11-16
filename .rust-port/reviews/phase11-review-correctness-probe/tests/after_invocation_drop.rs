use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use strands::Agent;
use strands::hooks::AfterInvocationEvent;
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;

#[tokio::test(start_paused = true)]
async fn drop_inside_after_invocation_hooks_is_not_retried() {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new("Hello!").into()]);
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let (started, finished) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (s, f) = (Arc::clone(&started), Arc::clone(&finished));
    agent.add_async_hook::<AfterInvocationEvent<'static>>(move |_event| {
        let (s, f) = (Arc::clone(&s), Arc::clone(&f));
        Box::pin(async move {
            if s.fetch_add(1, Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            f.fetch_add(1, Ordering::SeqCst);
        })
    });
    assert!(tokio::time::timeout(Duration::from_secs(1), agent.invoke("one")).await.is_err());
    assert_eq!(started.load(Ordering::SeqCst), 1);
    agent.shutdown().await.unwrap();
    assert_eq!(finished.load(Ordering::SeqCst), 1, "shutdown() must finish the AfterInvocationEvent hooks the drop cut short");
}
