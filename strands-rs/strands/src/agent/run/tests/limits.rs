//! Limits and the invocation context, ported from strands-ts/src/agent/__tests__/
//! {agent.test.ts `limits`, agent.invocation-context.test.ts}.

use std::num::{NonZeroU32, NonZeroU64};

use super::*;
use crate::agent::InvocationContext;
use crate::hooks::BeforeModelCallEvent;
use crate::tools::ToolContext;
use crate::types::agent::InvokeLimits;

fn usage(input: u64, output: u64, total: u64) -> MockTurnOptions {
    MockTurnOptions {
        usage: Some(Usage::new(input, output, total)),
        ..MockTurnOptions::default()
    }
}

fn loop_tool() -> FunctionTool {
    FunctionTool::new("loop", "loops", |_input: JsonValue, _ctx| async move {
        Ok::<_, ToolError>("ok")
    })
}

/// A model whose turns each call `loop` with the given usage.
fn loop_model(usages: &[(u64, u64, u64)]) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for (index, &(input, output, total)) in usages.iter().enumerate() {
        model.add_turn_with(
            vec![tool_use("loop", &format!("tool-{}", index + 1), json!({}))],
            usage(input, output, total),
        );
    }
    model
}

fn loop_agent(model: &Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .tools([loop_tool()])
        .build()
        .unwrap()
}

fn limits(turns: Option<u32>, total: Option<u64>, output: Option<u64>) -> InvokeLimits {
    InvokeLimits {
        turns: turns.and_then(NonZeroU32::new),
        total_tokens: total.and_then(NonZeroU64::new),
        output_tokens: output.and_then(NonZeroU64::new),
        ..InvokeLimits::default()
    }
}

fn with_limits(limits: InvokeLimits) -> InvokeOptions {
    InvokeOptions {
        limits: Some(limits),
        ..InvokeOptions::default()
    }
}

fn in_context(context: &InvocationContext, limits: Option<InvokeLimits>) -> InvokeOptions {
    InvokeOptions {
        invocation_context: Some(context.clone()),
        limits,
        ..InvokeOptions::default()
    }
}

fn usage_of(context: &InvocationContext) -> (u32, u64, u64, u64) {
    let usage = context.usage();
    let tokens = usage.tokens();
    (
        usage.turns_taken(),
        tokens.input_tokens(),
        tokens.output_tokens(),
        tokens.total_tokens(),
    )
}

mod stop_reasons {
    use super::*;

    #[tokio::test]
    async fn runs_the_cycle_to_completion_and_bails_at_top_of_next_iteration() {
        let model = loop_model(&[(10, 5, 15), (20, 5, 25)]);
        let mut agent = loop_agent(&model);
        let result = agent
            .invoke_with("go", with_limits(limits(Some(1), None, None)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(result.last_message().role(), Role::User);
        assert!(matches!(
            result.last_message().content()[0],
            ContentBlock::ToolResult(_)
        ));
        assert_eq!(result.metrics().cycle_count(), 1);
        assert_eq!(result.metrics().accumulated_usage(), &Usage::new(10, 5, 15));
        assert!(result.metrics().tool_metric("loop").is_some());
    }

    #[tokio::test]
    async fn does_not_trip_and_the_model_ends_naturally() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("done")], usage(5, 5, 10));
        let mut agent = test_agent(model);
        let result = agent
            .invoke_with("go", with_limits(limits(Some(5), Some(1000), Some(1000))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "done");
        assert_eq!(result.metrics().cycle_count(), 1);
    }

    #[tokio::test]
    async fn returns_limit_output_tokens_once_cumulative_output_tokens_hits_the_limit() {
        let model = loop_model(&[(10, 60, 70), (10, 60, 70), (10, 60, 70)]);
        let mut agent = loop_agent(&model);
        let result = agent
            .invoke_with("go", with_limits(limits(None, None, Some(100))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
        assert_eq!(result.last_message().role(), Role::User);
        assert_eq!(result.metrics().cycle_count(), 2);
        assert_eq!(result.metrics().accumulated_usage(), &Usage::new(20, 120, 140));
    }

    #[tokio::test]
    async fn uses_at_most_semantics_stops_when_count_exactly_equals_the_limit() {
        let model = loop_model(&[(10, 100, 110), (10, 100, 110)]);
        let mut agent = loop_agent(&model);
        let result = agent
            .invoke_with("go", with_limits(limits(None, None, Some(100))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
        assert_eq!(result.metrics().cycle_count(), 1);
    }

    #[tokio::test]
    async fn returns_limit_total_tokens_once_cumulative_total_tokens_hits_the_limit() {
        let model = loop_model(&[(50, 10, 60), (50, 10, 60), (50, 10, 60)]);
        let mut agent = loop_agent(&model);
        let result = agent
            .invoke_with("go", with_limits(limits(None, Some(100), None)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTotalTokens);
        assert_eq!(result.metrics().cycle_count(), 2);
    }

    #[tokio::test]
    async fn returns_end_turn_the_model_answer_wins() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("final answer")], usage(300, 300, 600));
        let mut agent = test_agent(model);
        let result = agent
            .invoke_with("go", with_limits(limits(None, Some(500), None)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "final answer");
    }

    async fn simultaneous(limits: InvokeLimits) -> AgentResult {
        let model = loop_model(&[(100, 100, 200), (100, 100, 200)]);
        loop_agent(&model)
            .invoke_with("go", with_limits(limits))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn prefers_turns_when_all_three_trip() {
        let result = simultaneous(limits(Some(1), Some(1), Some(1))).await;
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(result.metrics().cycle_count(), 1);
    }

    #[tokio::test]
    async fn prefers_total_tokens_over_output_tokens() {
        let result = simultaneous(limits(None, Some(1), Some(1))).await;
        assert_eq!(result.stop_reason(), StopReason::LimitTotalTokens);
    }

    #[tokio::test]
    async fn falls_back_to_output_tokens_when_no_higher_priority_limit_is_set() {
        let result = simultaneous(limits(None, None, Some(1))).await;
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
    }

    #[tokio::test]
    async fn scopes_the_limit_to_the_current_invocation_not_lifetime() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("first")], usage(10, 50, 60));
        model.add_turn_with(vec![text("second")], usage(10, 50, 60));
        let mut agent = test_agent(model);
        for prompt in ["go", "go again"] {
            let result = agent
                .invoke_with(prompt, with_limits(limits(None, None, Some(75))))
                .await
                .unwrap();
            assert_eq!(result.stop_reason(), StopReason::EndTurn);
            assert_eq!(
                result.metrics().latest_agent_invocation().unwrap().cycles().len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn returns_limit_turns_as_the_generator_return_value() {
        let model = loop_model(&[(10, 5, 15), (10, 5, 15)]);
        let mut agent = loop_agent(&model);
        let events = collect(agent.stream_with("go", with_limits(limits(Some(1), None, None)))).await;
        let Some(AgentStreamEvent::AgentResult(result)) = events.last() else {
            panic!("the stream must end with the result");
        };
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    }
}

mod invocation_context {
    use super::*;

    /// The sub-agent as a tool, named like the relay the parent's model calls.
    fn as_relay(sub: Agent) -> crate::agent::AgentAsTool {
        sub.as_tool(crate::agent::AgentAsToolOptions {
            name: Some("relay".into()),
            ..Default::default()
        })
    }

    /// A hand-written tool that runs the sub-agent forwarding only the invocation state; `as_tool`
    /// always forwards the invocation context too.
    fn sub_agent_tool(sub: Agent) -> FunctionTool {
        let sub = Arc::new(tokio::sync::Mutex::new(sub));
        FunctionTool::new(
            "relay",
            "runs the sub-agent",
            move |_input: JsonValue, ctx: ToolContext| {
                let sub = Arc::clone(&sub);
                async move {
                    let options = InvokeOptions {
                        invocation_state: ctx.invocation_state().clone(),
                        ..InvokeOptions::default()
                    };
                    let result = sub.lock().await.invoke_with("go", options).await.unwrap();
                    Ok::<_, ToolError>(result.stop_reason().to_string())
                }
            },
        )
    }

    fn sub_agent() -> Agent {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("sub done")], usage(1, 1, 2));
        test_agent(model)
    }

    fn relay_model(count: usize) -> Arc<MockMessageModel> {
        let model = Arc::new(MockMessageModel::new());
        for index in 1..=count {
            model.add_turn_with(
                vec![tool_use("relay", &format!("tu-{index}"), json!({"input": "go"}))],
                usage(1, 1, 2),
            );
        }
        model
    }

    #[tokio::test]
    async fn leaves_the_parent_limit_intact_and_the_child_uncounted_when_a_hand_written_tool_forwards_invocation_state()
     {
        let parent_model = relay_model(3);
        let mut parent = Agent::builder()
            .model(Arc::clone(&parent_model))
            .tools([sub_agent_tool(sub_agent())])
            .build()
            .unwrap();
        let result = parent
            .invoke_with("start", with_limits(limits(Some(2), None, None)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(parent_model.requests().len(), 2);
    }

    #[tokio::test]
    async fn joins_the_request_when_a_hand_written_tool_forwards_context_invocation_context() {
        let parent_model = relay_model(2);
        let mut parent = Agent::builder()
            .model(Arc::clone(&parent_model))
            .tools([as_relay(sub_agent())])
            .build()
            .unwrap();
        let result = parent
            .invoke_with("start", with_limits(limits(Some(2), None, None)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(parent_model.requests().len(), 1);
        let context = result.invocation_context().unwrap();
        assert_eq!(usage_of(context), (2, 2, 2, 4));
    }

    #[tokio::test]
    async fn reports_the_whole_requests_usage_on_the_result_while_metrics_cover_this_agent_only() {
        let inner = {
            let model = Arc::new(MockMessageModel::new());
            model.add_turn_with(vec![text("inner done")], usage(5, 7, 12));
            test_agent(model)
        };
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![tool_use("relay", "tu-1", json!({"input": "go"}))],
            usage(1, 2, 3),
        );
        model.add_turn_with(vec![text("outer done")], usage(1, 1, 2));
        let mut outer = Agent::builder()
            .model(model)
            .tools([as_relay(inner)])
            .build()
            .unwrap();
        let result = outer.invoke("go").await.unwrap();
        assert_eq!(usage_of(result.invocation_context().unwrap()), (3, 7, 10, 17));
        assert_eq!(result.metrics().accumulated_usage(), &Usage::new(2, 3, 5));
    }

    #[tokio::test]
    async fn produces_a_well_formed_result_when_an_inherited_limit_is_already_exhausted_on_entry() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("should not run")], usage(1, 1, 2));
        let mut agent = Agent::builder()
            .model(model)
            .messages(vec![
                Message::new(Role::User, vec![text("earlier question")]),
                Message::new(Role::Assistant, vec![text("earlier answer")]),
            ])
            .build()
            .unwrap();
        let inherited = InvocationContext::new(Some(limits(Some(1), None, None)), None, false);
        inherited.set_turns_taken(5);
        let result = agent
            .invoke_with("go", in_context(&inherited, None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(result.last_message().role(), Role::Assistant);
        assert!(result.last_message().content().is_empty());
        assert_eq!(result.to_string(), "");
    }

    #[tokio::test]
    async fn continues_a_previous_calls_limits_and_usage_when_passed_its_invocation_context() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("first")], usage(1, 1, 2));
        model.add_turn_with(vec![text("second")], usage(1, 1, 2));
        model.add_turn(vec![text("never reached")]);
        let mut agent = test_agent(model.clone());
        let first = agent
            .invoke_with("one", with_limits(limits(Some(2), None, None)))
            .await
            .unwrap();
        let first_context = first.invocation_context().unwrap().clone();
        let second = agent
            .invoke_with("two", in_context(&first_context, None))
            .await
            .unwrap();
        let second_context = second.invocation_context().unwrap().clone();
        let third = agent
            .invoke_with("three", in_context(&second_context, None))
            .await
            .unwrap();
        assert_eq!(
            [first.stop_reason(), second.stop_reason(), third.stop_reason()],
            [StopReason::EndTurn, StopReason::EndTurn, StopReason::LimitTurns]
        );
        assert_eq!(second_context, first_context);
        assert_eq!(usage_of(third.invocation_context().unwrap()), (2, 2, 2, 4));
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn counts_only_a_nested_invokes_own_tokens_against_its_own_token_limits() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("done")], usage(10, 10, 20));
        let mut agent = test_agent(model);
        let enclosing = InvocationContext::new(None, None, false);
        enclosing.record_usage(&Usage::new(4000, 4000, 8000));
        let result = agent
            .invoke_with("go", in_context(&enclosing, Some(limits(None, Some(5000), None))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(usage_of(result.invocation_context().unwrap()), (1, 10, 10, 20));
        assert_eq!(usage_of(&enclosing), (1, 4010, 4010, 8020));
    }

    #[tokio::test]
    async fn stops_a_nested_invoke_on_the_requests_limit_even_when_its_own_limit_is_looser() {
        let model = loop_model(&[(0, 0, 0), (0, 0, 0)]);
        let mut agent = loop_agent(&model);
        let request = InvocationContext::new(Some(limits(Some(2), None, None)), None, false);
        request.set_turns_taken(1);
        let result = agent
            .invoke_with("go", in_context(&request, Some(limits(Some(5), None, None))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(model.call_count(), 1);
        assert_eq!(request.usage().turns_taken(), 2);
    }

    #[tokio::test]
    async fn stops_a_nested_invoke_on_its_own_tighter_limit_and_counts_its_turns_toward_the_request() {
        let model = loop_model(&[(1, 2, 3)]);
        model.add_turn(vec![text("never reached")]);
        let mut agent = loop_agent(&model);
        let enclosing = InvocationContext::new(Some(limits(Some(10), None, None)), None, false);
        let result = agent
            .invoke_with("go", in_context(&enclosing, Some(limits(Some(1), None, None))))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(model.call_count(), 1);
        assert_eq!(usage_of(&enclosing), (1, 1, 2, 3));
    }

    #[tokio::test]
    async fn folds_an_auxiliary_call_into_the_request_total_without_limiting_it() {
        let parent = InvocationContext::new(Some(limits(Some(1), None, None)), None, false);
        parent.record_usage(&Usage::new(1, 2, 3));
        parent.set_turns_taken(5);
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(vec![text("aux done")], usage(4, 5, 9));
        let mut aux = test_agent(model);
        let auxiliary = InvocationContext::auxiliary(Some(&parent)).unwrap();
        let result = aux.invoke_with("go", in_context(&auxiliary, None)).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(usage_of(&parent), (5, 5, 7, 12));
    }

    #[tokio::test]
    async fn attaches_one_request_handle_to_the_events_and_tool_context_it_hands_out() {
        let probed = Arc::new(Mutex::new(None));
        let probe = {
            let probed = Arc::clone(&probed);
            FunctionTool::new(
                "probe",
                "captures its tool context",
                move |_input: JsonValue, ctx: ToolContext| {
                    *probed.lock().unwrap() = ctx.invocation_context().cloned();
                    async { Ok::<_, ToolError>("ok") }
                },
            )
        };
        let model = mock(vec![
            vec![tool_use("probe", "tu-1", json!({}))],
            vec![text("done")],
        ]);
        let mut agent = Agent::builder().model(model).tools([probe]).build().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            sink.lock()
                .unwrap()
                .push(event.invocation_context().cloned().unwrap());
        });
        agent.invoke("go").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1], seen[0]);
        assert_eq!(probed.lock().unwrap().as_ref(), Some(&seen[0]));
    }

    #[tokio::test]
    async fn starts_fresh_request_state_on_each_root_invoke_of_a_reused_agent() {
        let model = mock(vec![vec![text("first")], vec![text("second")]]);
        let mut agent = test_agent(model);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            sink.lock()
                .unwrap()
                .push(event.invocation_context().cloned().unwrap());
        });
        agent.invoke("one").await.unwrap();
        agent.invoke("two").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_ne!(seen[1], seen[0]);
        assert_eq!(seen[1].usage().turns_taken(), 1);
    }
}
