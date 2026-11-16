//! `AfterToolsEvent::end_turn` and `AfterInvocationEvent::resume`, ported from
//! strands-ts/src/agent/__tests__/agent.hook.test.ts (`AfterToolsEvent.endTurn`,
//! `AfterInvocationEvent resume`, the `AfterToolsEvent` result case) and the end-turn and resume
//! cases of agent.test.ts and agent.invocation-context.test.ts.

use std::num::NonZeroU32;

use super::*;
use crate::hooks::{AfterToolsEvent, AgentResultEvent, BeforeModelCallEvent, EndTurn};
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, StructuredOutputSchema};
use crate::types::agent::InvokeLimits;

const DEFAULT_END: &str = "Turn ended early by hook after tool execution";

fn my_tool() -> FunctionTool {
    FunctionTool::new("myTool", "a tool", |_input: JsonValue, _ctx| async move {
        Ok::<_, ToolError>("result")
    })
}

fn single_tool_setup() -> (Agent, Arc<MockMessageModel>) {
    let model = mock(vec![
        vec![tool_use("myTool", "tool-1", json!({}))],
        vec![text("Should not reach this")],
    ]);
    let agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([my_tool()])
        .build()
        .unwrap();
    (agent, model)
}

fn end_turn_with(agent: &mut Agent, end_turn: EndTurn) {
    agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
        event.set_end_turn(end_turn.clone());
    });
}

mod end_turn {
    use super::*;

    #[tokio::test]
    async fn halts_the_loop_when_end_turn_is_true_with_default_message() {
        let (mut agent, model) = single_tool_setup();
        end_turn_with(&mut agent, EndTurn::Requested);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().role(), Role::Assistant);
        assert_eq!(texts(result.last_message()), [DEFAULT_END]);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn halts_the_loop_with_custom_assistant_message_when_end_turn_is_a_string() {
        let (mut agent, model) = single_tool_setup();
        end_turn_with(&mut agent, "Stopping here".into());
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(texts(result.last_message()), ["Stopping here"]);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn does_not_halt_by_default_or_for_an_empty_string_or_list() {
        for end_turn in [
            None,
            Some(EndTurn::from("")),
            Some(EndTurn::from(Vec::<ContentBlock>::new())),
        ] {
            let (mut agent, model) = single_tool_setup();
            if let Some(end_turn) = end_turn {
                end_turn_with(&mut agent, end_turn);
            }
            let result = agent.invoke("Test").await.unwrap();
            assert_eq!(texts(result.last_message()), ["Should not reach this"]);
            assert_eq!(model.call_count(), 2);
        }
    }

    #[tokio::test]
    async fn halts_with_content_blocks_when_end_turn_is_a_content_block_array() {
        let (mut agent, _model) = single_tool_setup();
        end_turn_with(&mut agent, vec![text("Block one"), text("Block two")].into());
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(texts(result.last_message()), ["Block one", "Block two"]);
    }

    #[tokio::test]
    async fn appends_tool_results_and_default_end_turn_message_to_conversation_history() {
        let (mut agent, _model) = single_tool_setup();
        end_turn_with(&mut agent, EndTurn::Requested);
        agent.invoke("Test").await.unwrap();
        let roles: Vec<Role> = agent.messages().iter().map(Message::role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
        assert!(matches!(
            agent.messages()[1].content()[0],
            ContentBlock::ToolUse(_)
        ));
        assert!(matches!(
            agent.messages()[2].content()[0],
            ContentBlock::ToolResult(_)
        ));
        assert_eq!(texts(&agent.messages()[3]), [DEFAULT_END]);
    }

    #[tokio::test]
    async fn halts_the_loop_with_concurrent_tool_execution() {
        let model = mock(vec![
            vec![
                tool_use("add", "tool-1", json!({})),
                tool_use("myTool", "tool-2", json!({})),
            ],
            vec![text("Should not reach this")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools(crate::tool_list![add_tool(), my_tool()])
            .build()
            .unwrap();
        end_turn_with(&mut agent, EndTurn::Requested);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn emits_after_tools_event_with_end_turn_via_stream() {
        let (mut agent, _model) = single_tool_setup();
        end_turn_with(&mut agent, EndTurn::Requested);
        let events = collect(agent.stream("Test")).await;
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentStreamEvent::AfterTools(_)))
        );
        let results: Vec<&AgentResult> = events
            .iter()
            .filter_map(|event| match event {
                AgentStreamEvent::AgentResult(result) => Some(result),
                _ => None,
            })
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn halts_even_when_set_on_a_cancelled_tools_after_tools_event() {
        let (mut agent, model) = single_tool_setup();
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(crate::hooks::Cancel::Requested));
        end_turn_with(&mut agent, EndTurn::Requested);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().role(), Role::Assistant);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn after_tools_sees_a_result_replaced_by_after_tool_call() {
        let model = mock(vec![
            vec![tool_use("myTool", "tool-1", json!({}))],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder().model(model).tools([my_tool()]).build().unwrap();
        agent.add_hook(|event: &mut AfterToolCallEvent| {
            event.set_result(ToolResultBlock::new(
                "tool-1",
                ToolResultStatus::Success,
                vec!["[REDACTED]".into()],
            ));
        });
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
            sink.lock().unwrap().push(event.message().clone());
        });
        agent.invoke("Test").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let ContentBlock::ToolResult(result) = &seen[0].content()[0] else {
            panic!("expected a tool result");
        };
        assert_eq!(result.content(), ["[REDACTED]".into()]);
    }

    #[tokio::test]
    async fn honors_end_turn_over_structured_output() {
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        #[expect(dead_code, reason = "schema-only test type")]
        struct Name {
            name: String,
        }
        let model = mock(vec![
            vec![tool_use(
                STRUCTURED_OUTPUT_TOOL_NAME,
                "tool-1",
                json!({"name": "x"}),
            )],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .structured_output_schema(StructuredOutputSchema::of::<Name>())
            .build()
            .unwrap();
        end_turn_with(&mut agent, EndTurn::Requested);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.structured_output_value(), None);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn counts_a_failed_structured_output_attempt_when_a_hook_ends_the_turn() {
        #[derive(serde::Deserialize, schemars::JsonSchema)]
        #[expect(dead_code, reason = "schema-only test type")]
        struct Plan {
            steps: Vec<String>,
        }
        let model = Arc::new(MockMessageModel::new());
        model
            .add_turn(vec![tool_use(STRUCTURED_OUTPUT_TOOL_NAME, "invalid", json!({}))])
            .add_error(|| Error::other("Unexpected model call"));
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .structured_output_schema(StructuredOutputSchema::of::<Plan>())
            .build()
            .unwrap();
        end_turn_with(&mut agent, "Paused for review".into());
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                structured_output_attempts: NonZeroU32::new(1),
                ..InvokeLimits::default()
            }),
            ..InvokeOptions::default()
        };
        let ended = agent.invoke_with("Plan a review", options).await.unwrap();
        let options = InvokeOptions {
            invocation_context: ended.invocation_context().cloned(),
            ..InvokeOptions::default()
        };
        let result = agent.invoke_with("Continue", options).await.unwrap();
        assert_eq!(
            [ended.stop_reason(), result.stop_reason()],
            [StopReason::EndTurn, StopReason::LimitStructuredOutputAttempts]
        );
        assert_eq!(model.call_count(), 1);
    }
}

mod resume {
    use super::*;

    fn resume_on(agent: &mut Agent, plan: Vec<(usize, &'static str)>) -> Arc<Mutex<usize>> {
        let count = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&count);
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            let mut count = counter.lock().unwrap();
            *count += 1;
            if let Some((_, args)) = plan.iter().find(|(at, _)| *at == *count) {
                event.set_resume(*args);
            }
        });
        count
    }

    fn user_texts(agent: &Agent) -> Vec<String> {
        agent
            .messages()
            .iter()
            .filter(|message| message.role() == Role::User)
            .flat_map(|message| texts(message).into_iter().map(str::to_owned).collect::<Vec<_>>())
            .collect()
    }

    #[tokio::test]
    async fn re_invokes_the_agent_with_the_resume_args() {
        let mut agent = test_agent(mock(vec![vec![text("first")], vec![text("second")]]));
        let count = resume_on(&mut agent, vec![(1, "follow-up")]);
        let result = agent.invoke("initial").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 2);
        assert_eq!(texts(result.last_message()), ["second"]);
        assert_eq!(user_texts(&agent), ["initial", "follow-up"]);
        assert_eq!(result.metrics().cycle_count(), 2);
    }

    #[tokio::test]
    async fn chains_multiple_resumes() {
        let mut agent = test_agent(mock(vec![vec![text("a")], vec![text("b")], vec![text("c")]]));
        let count = resume_on(&mut agent, vec![(1, "second"), (2, "third")]);
        let result = agent.invoke("first").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 3);
        assert_eq!(texts(result.last_message()), ["c"]);
    }

    #[tokio::test]
    async fn does_not_resume_when_resume_is_left_undefined() {
        let mut agent = test_agent(mock(vec![vec![text("only")]]));
        let count = resume_on(&mut agent, vec![]);
        agent.invoke("hi").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn does_not_resume_when_the_invocation_errors() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("boom").into());
        let mut agent = test_agent(model);
        let count = resume_on(&mut agent, vec![(1, "should-not-run")]);
        let passes = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&passes);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *counter.lock().unwrap() += 1);
        let error = agent.invoke("hi").await.unwrap_err();
        assert!(error.to_string().contains("boom"));
        assert_eq!((*count.lock().unwrap(), *passes.lock().unwrap()), (1, 1));
    }

    #[tokio::test]
    async fn first_registered_hook_wins_when_multiple_hooks_set_resume() {
        let mut agent = test_agent(mock(vec![vec![text("first")], vec![text("second")]]));
        let passes = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&passes);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *counter.lock().unwrap() += 1);
        for args in ["first-registered wins", "second-registered loses"] {
            let passes = Arc::clone(&passes);
            agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
                if *passes.lock().unwrap() == 1 {
                    event.set_resume(args);
                }
            });
        }
        agent.invoke("initial").await.unwrap();
        assert_eq!(user_texts(&agent), ["initial", "first-registered wins"]);
    }

    #[tokio::test]
    async fn emits_only_one_agent_result_event_for_a_resumed_chain() {
        let mut agent = test_agent(mock(vec![vec![text("first")], vec![text("second")]]));
        resume_on(&mut agent, vec![(1, "follow-up")]);
        let results = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&results);
        agent.add_hook(move |_: &mut AgentResultEvent<'_>| *counter.lock().unwrap() += 1);
        let events = collect(agent.stream("initial")).await;
        let count = |kind: &str| events.iter().filter(|event| type_of(event) == kind).count();
        assert_eq!((count("agentResultEvent"), count("afterInvocationEvent")), (1, 2));
        assert_eq!(*results.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn ends_on_a_limit_stop_even_when_a_hook_asks_to_resume() {
        let model = mock(vec![vec![tool_use("add", "tu-1", json!({}))]]);
        let mut agent = tool_agent(&model);
        let passes = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&passes);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *counter.lock().unwrap() += 1);
        let model_calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&model_calls);
        agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| *counter.lock().unwrap() += 1);
        let bound = Arc::clone(&passes);
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            if *bound.lock().unwrap() < 5 {
                event.set_resume("try again");
            }
        });
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                turns: NonZeroU32::new(1),
                ..InvokeLimits::default()
            }),
            ..InvokeOptions::default()
        };
        let result = agent.invoke_with("go", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!((*passes.lock().unwrap(), *model_calls.lock().unwrap()), (1, 1));
    }
}
