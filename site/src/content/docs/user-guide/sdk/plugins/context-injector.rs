//! Rust snippets for `context-injector.mdx`.

// =====================
// Getting Started
// =====================

fn getting_started() -> Result<(), strands_agents::Error> {
    // --8<-- [start:getting_started]
    use std::time::{SystemTime, UNIX_EPOCH};

    use strands_agents::plugins::ContextInjector;
    use strands_agents::testing::MockMessageModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(MockMessageModel::shared(vec![]))
        .plugin(ContextInjector::new(|_context| {
            Box::pin(async {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default();
                Ok(Some(format!("<now>{}</now>", now.as_secs())))
            })
        }))
        .build()?;
    // --8<-- [end:getting_started]

    let _ = agent;
    Ok(())
}

// =====================
// Inject before every turn
// =====================

fn every_turn() {
    // --8<-- [start:every_turn]
    use std::time::{SystemTime, UNIX_EPOCH};

    use strands_agents::injection::InjectionTrigger;
    use strands_agents::plugins::ContextInjector;

    let clock = ContextInjector::new(|_context| {
        Box::pin(async {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            Ok(Some(format!("<now>{}</now>", now.as_secs())))
        })
    })
    .with_name("clock")
    // inject before every model call, not just fresh user asks
    .with_trigger(InjectionTrigger::EveryTurn);
    // --8<-- [end:every_turn]

    let _ = clock;
}

// =====================
// Predicate trigger reading app state
// =====================

fn predicate() {
    // --8<-- [start:predicate]
    use strands_agents::plugins::ContextInjector;
    use strands_agents::types::JsonValue;

    let injector = ContextInjector::new(|context| {
        let turns = context.messages.len();
        Box::pin(async move {
            Ok(Some(format!("<context>{turns} turns so far</context>")))
        })
    })
    // Inject only when a tool stashed a flag in app state last turn.
    .with_trigger_fn(|context| {
        context.agent.state().get("recallEnabled") == Some(JsonValue::Bool(true))
    });
    // --8<-- [end:predicate]

    let _ = injector;
}
