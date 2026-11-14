//! Ported from strands-ts/src/agent/__tests__/agent.test.ts
//!
//! Gaps (reasons in PARITY.md): hook errors (Rust hooks can't fail), concurrency guards
//! (`&mut self` makes concurrent invocation a compile error), stream drop and async disposal
//! (Phase 4), MCP, session manager, checkpoints and history offloading (later phases), and
//! the provider-specific model cases (Phase 5).

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::json;

use crate::agent::{Agent, AgentConfig, AgentStreamEvent, InvokeArgs, InvokeLimits, InvokeOptions};
use crate::error::{Error, MaxTokensError};
use crate::hooks::{AfterToolsEvent, BeforeModelCallEvent, EndTurn};
use crate::models::{Model, Usage};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::{STRUCTURED_OUTPUT_TOOL_NAME, ToolChoice, ToolSource};
use crate::types::{
    ContentBlock, Message, ReasoningBlock, Role, StopReason, SystemPrompt, ToolResultContent, ToolUseBlock,
};

fn tool_use(name: &str, id: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))
}

fn agent_with(model: &Arc<MockModel>, tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn mock() -> Arc<MockModel> {
    Arc::new(MockModel::new())
}

fn roles(agent: &Agent) -> Vec<Role> {
    agent.messages().iter().map(|message| message.role).collect()
}

mod stream_and_invoke {
    use super::*;

    #[tokio::test]
    async fn yields_agent_stream_event_objects() {
        let model = mock();
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let mut types = Vec::new();
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            types.push(event.unwrap().event_type());
        }
        assert_eq!(types.first(), Some(&"beforeInvocationEvent"));
        assert_eq!(types.last(), Some(&"agentResultEvent"));
        assert!(types.contains(&"modelStreamUpdateEvent"));
    }

    #[tokio::test]
    async fn returns_agent_result_as_generator_return_value() {
        let model = mock();
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let mut last = None;
        let mut stream = agent.stream("Hi");
        while let Some(event) = stream.next().await {
            last = Some(event.unwrap());
        }
        let Some(AgentStreamEvent::AgentResult(result)) = last else {
            panic!("the last event is the result");
        };
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn returns_correct_stop_reason_and_last_message() {
        let model = mock();
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content, vec![ContentBlock::from("Hello")]);
        assert_eq!(result.to_string(), "Hello");
    }

    #[tokio::test]
    async fn executes_tools_and_returns_final_result() {
        let model = mock();
        model.add_turn(vec![tool_use("calc", "t1", json!({}))]);
        model.add_turn(vec!["The answer is 4".into()]);
        let mut agent = agent_with(
            &model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        );
        let result = agent.invoke("2+2").await.unwrap();
        assert_eq!(result.to_string(), "The answer is 4");
        assert_eq!(
            roles(&agent),
            [Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn stores_cycle_id_in_trace_metadata() {
        let model = mock();
        model.add_turn(vec![tool_use("calc", "t1", json!({}))]);
        model.add_turn(vec!["done".into()]);
        let mut agent = agent_with(
            &model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        );
        let result = agent.invoke("go").await.unwrap();
        let traces = result.traces();
        assert_eq!(traces.len(), 2);
        assert_eq!(traces[0].name, "Cycle 1");
        assert_eq!(traces[0].metadata["cycleId"], "cycle-1");
        let children: Vec<&str> = traces[0]
            .children
            .iter()
            .map(|child| child.name.as_str())
            .collect();
        assert_eq!(children, ["stream_messages", "Tool: calc"]);
        let tool = &traces[0].children[1];
        assert_eq!(tool.metadata["toolUseId"], "t1");
        assert_eq!(tool.metadata["toolName"], "calc");
        assert_eq!(tool.parent_id.as_deref(), Some(traces[0].id.as_str()));
    }

    #[tokio::test]
    async fn throws_max_tokens_error_when_model_hits_token_limit() {
        let model = mock();
        model.add_turn_with(vec!["partial".into()], Some(StopReason::MaxTokens), None);
        let mut agent = agent_with(&model, Vec::new());
        let error = agent.invoke("Hi").await.unwrap_err();
        assert!(matches!(error, Error::MaxTokens(_)), "{error}");
        let _: Option<&MaxTokensError> = None;
    }

    #[tokio::test]
    async fn invoke_and_stream_produce_same_final_result() {
        let first = mock();
        first.add_turn(vec!["Same".into()]);
        let mut agent = agent_with(&first, Vec::new());
        let invoked = agent.invoke("Hi").await.unwrap();
        let mut streamed = None;
        {
            let mut stream = agent.stream("Hi");
            while let Some(event) = stream.next().await {
                if let AgentStreamEvent::AgentResult(result) = event.unwrap() {
                    streamed = Some(result);
                }
            }
        }
        let streamed = streamed.unwrap();
        assert_eq!(invoked.stop_reason(), streamed.stop_reason());
        assert_eq!(invoked.to_string(), streamed.to_string());
    }

    #[tokio::test]
    async fn allows_sequential_invocations_after_lock_is_released() {
        let model = mock();
        model.add_error(|| crate::error::ModelError::new("boom").into());
        model.add_turn(vec!["ok".into()]);
        let mut agent = Agent::new(AgentConfig {
            model: Some((Arc::clone(&model) as Arc<dyn Model>).into()),
            retry_strategy: Some(Vec::new()),
            printer: Some(false),
            ..Default::default()
        })
        .unwrap();
        assert!(agent.invoke("first").await.is_err());
        assert_eq!(agent.invoke("second").await.unwrap().to_string(), "ok");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_loop_runs_tools_on_a_current_thread_runtime() {
        let model = mock();
        model.add_turn(vec![tool_use("calc", "t1", json!({}))]);
        model.add_turn(vec!["done".into()]);
        let mut agent = agent_with(
            &model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        );
        assert_eq!(agent.invoke("go").await.unwrap().to_string(), "done");
    }

    #[test]
    fn the_agent_its_stream_and_its_futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        fn assert_send_sync<T: Send + Sync>() {}
        let mut agent = agent_with(&mock(), Vec::new());
        assert_send(&agent);
        assert_send_sync::<crate::AgentResult>();
        assert_send_sync::<crate::agent::ToolAgent>();
        assert_send_sync::<crate::ToolContext>();
        assert_send_sync::<Error>();
        assert_send_sync::<AgentStreamEvent>();
        assert_send(&agent.invoke("x"));
        assert_send(&agent.stream("x"));
    }
}

mod metrics {
    use super::*;

    #[tokio::test]
    async fn exposes_accumulated_metrics_on_the_agent_instance() {
        let model = mock();
        model.add_turn_with(vec!["Hello".into()], None, Some(Usage::new(10, 5, 15)));
        let mut agent = agent_with(&model, Vec::new());
        agent.invoke("Hi").await.unwrap();
        let metrics = agent.metrics();
        assert_eq!(metrics.cycle_count, 1);
        assert_eq!(metrics.accumulated_usage, Usage::new(10, 5, 15));
        assert_eq!(metrics.latest_context_size, Some(10));
        assert_eq!(metrics.projected_context_size, Some(15));
        assert_eq!(metrics.agent_invocations.len(), 1);
    }

    #[tokio::test]
    async fn tracks_tool_metrics_by_name() {
        let model = mock();
        model.add_turn(vec![tool_use("calc", "t1", json!({}))]);
        model.add_turn(vec!["done".into()]);
        let mut agent = agent_with(
            &model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        );
        let result = agent.invoke("go").await.unwrap();
        let calc = result.metrics().tool_metrics["calc"];
        assert_eq!((calc.call_count, calc.success_count, calc.error_count), (1, 1, 0));
        assert_eq!(result.metrics().cycle_count, 2);
    }

    #[tokio::test]
    async fn tracks_cycle_count_when_max_tokens_error_occurs() {
        let model = mock();
        model.add_turn_with(vec!["partial".into()], Some(StopReason::MaxTokens), None);
        let mut agent = agent_with(&model, Vec::new());
        assert!(agent.invoke("Hi").await.is_err());
        assert_eq!(agent.metrics().cycle_count, 1);
    }
}

mod configuration {
    use super::*;

    #[tokio::test]
    async fn reflects_conversation_history_after_invoke() {
        let model = mock();
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent_with(&model, Vec::new());
        assert!(agent.messages().is_empty());
        agent.invoke("Hi").await.unwrap();
        assert_eq!(roles(&agent), [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn builds_without_a_model_and_fails_on_first_use() {
        let mut agent = Agent::builder().printer(false).build().unwrap();
        let error = agent.invoke("Hi").await.unwrap_err();
        assert!(error.is_model_error(), "{error:?}");
        assert!(error.to_string().contains("no model configured"), "{error}");
    }

    #[test]
    fn defaults_to_printer_true_when_not_specified() {
        let agent = Agent::new(AgentConfig::default()).unwrap();
        assert!(agent.printer.is_some());
    }

    #[test]
    fn does_not_create_printer_when_printer_is_false() {
        let agent = agent_with(&mock(), Vec::new());
        assert!(agent.printer.is_none());
    }

    #[tokio::test]
    async fn validates_output_when_printer_is_enabled() {
        let model = mock();
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let output = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&output);
        agent.printer = Some(Arc::new(Mutex::new(crate::agent::printer::AgentPrinter::new(
            move |text| {
                sink.lock().unwrap().push_str(text);
            },
        ))));
        agent.invoke("Hi").await.unwrap();
        assert_eq!(*output.lock().unwrap(), "Hello\n");
    }

    #[test]
    fn flattens_nested_arrays_at_any_depth() {
        let tools: Vec<ToolSource> = crate::tool_list![
            mock_tool("a", |ctx| text_result(ctx, "a")),
            vec![
                ToolSource::from(mock_tool("b", |ctx| text_result(ctx, "b"))),
                ToolSource::from(vec![mock_tool("c", |ctx| text_result(ctx, "c"))])
            ],
        ];
        let agent = agent_with(&mock(), tools);
        let names: Vec<&str> = agent.tool_registry().names().collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn catches_duplicate_tool_names_across_nested_arrays() {
        let tools: Vec<ToolSource> = crate::tool_list![
            mock_tool("a", |ctx| text_result(ctx, "a")),
            vec![mock_tool("a", |ctx| text_result(ctx, "a"))],
        ];
        let error = Agent::new(AgentConfig {
            tools,
            ..Default::default()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "Tool with name 'a' already registered");
    }

    #[test]
    fn builder_tools_extend_rather_than_replace() {
        let agent = Agent::builder()
            .printer(false)
            .tools([mock_tool("a", |ctx| text_result(ctx, "a"))])
            .tools([mock_tool("b", |ctx| text_result(ctx, "b"))])
            .build()
            .unwrap();
        assert_eq!(agent.tools().len(), 2);
    }

    #[tokio::test]
    async fn accepts_and_stores_string_system_prompt() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model) as Arc<dyn Model>)
            .system_prompt("Be brief.")
            .printer(false)
            .build()
            .unwrap();
        assert_eq!(agent.system_prompt(), Some(&SystemPrompt::from("Be brief.")));
        agent.invoke("Hi").await.unwrap();
        assert_eq!(
            model.requests()[0].system_prompt,
            Some(SystemPrompt::from("Be brief."))
        );
    }

    #[tokio::test]
    async fn allows_system_prompt_to_be_changed_between_turns() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        agent.invoke("Hi").await.unwrap();
        agent.set_system_prompt(Some("New".into()));
        agent.invoke("Hi").await.unwrap();
        let requests = model.requests();
        assert_eq!(requests[0].system_prompt, None);
        assert_eq!(requests[1].system_prompt, Some(SystemPrompt::from("New")));
    }

    #[tokio::test]
    async fn allows_model_change_to_persist_across_invocations() {
        let first = mock();
        first.add_turn(vec!["first".into()]);
        let second = mock();
        second.add_turn(vec!["second".into()]);
        let mut agent = agent_with(&first, Vec::new());
        agent.set_model(Arc::clone(&second) as Arc<dyn Model>);
        assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "second");
        assert_eq!(agent.invoke("Hi").await.unwrap().to_string(), "second");
        assert_eq!(first.call_count(), 0);
    }

    #[tokio::test]
    async fn creates_default_bedrock_model_and_fails_on_first_use_when_unavailable() {
        let mut agent = Agent::builder().printer(false).build().unwrap();
        let error = agent.invoke("Hi").await.unwrap_err();
        // Model streams wrap errors outside the ModelError family, as TS `streamAggregated` does.
        assert!(error.is_model_error());
        assert!(error.to_string().starts_with("no model configured"), "{error}");
    }

    #[test]
    fn a_string_model_is_a_bedrock_model_id() {
        let agent = Agent::builder()
            .model("global.anthropic.claude-sonnet-4-6")
            .build()
            .unwrap();
        assert_eq!(
            agent.model().model_id().as_deref(),
            Some("global.anthropic.claude-sonnet-4-6")
        );
    }

    #[test]
    fn aux_model_defaults_to_model_and_uses_the_configured_one() {
        let main = mock();
        let agent = agent_with(&main, Vec::new());
        assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
        let aux: Arc<dyn Model> = Arc::new(MockModel::new());
        let agent = Agent::new(AgentConfig {
            aux_model: Some(Arc::clone(&aux).into()),
            ..Default::default()
        })
        .unwrap();
        assert!(Arc::ptr_eq(agent.aux_model(), &aux));
    }
}

mod input {
    use super::*;

    #[tokio::test]
    async fn assigns_a_durable_tracking_id_to_every_recorded_message() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        agent.invoke("Hi").await.unwrap();
        assert!(
            agent
                .messages()
                .iter()
                .all(|message| !message.tracking_id().is_empty())
        );
    }

    #[tokio::test]
    async fn preserves_a_caller_supplied_id_on_input_messages() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let message = Message::new(Role::User, vec!["Hi".into()]).with_tracking_id("caller-id");
        agent.invoke(message).await.unwrap();
        assert_eq!(agent.messages()[0].tracking_id(), "caller-id");
    }

    #[tokio::test]
    async fn creates_single_user_message_with_multiple_blocks() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        agent
            .invoke(vec![ContentBlock::from("a"), ContentBlock::from("b")])
            .await
            .unwrap();
        assert_eq!(
            agent.messages()[0].content,
            vec![ContentBlock::from("a"), ContentBlock::from("b")]
        );
    }

    #[tokio::test]
    async fn handles_empty_content_block_array() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        agent.invoke(Vec::<ContentBlock>::new()).await.unwrap();
        assert_eq!(roles(&agent), [Role::Assistant]);
    }

    #[tokio::test]
    async fn appends_multiple_messages_in_order() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let messages = vec![
            Message::new(Role::User, vec!["one".into()]),
            Message::new(Role::Assistant, vec!["two".into()]),
            Message::new(Role::User, vec!["three".into()]),
        ];
        agent.invoke(messages).await.unwrap();
        assert_eq!(
            roles(&agent),
            [Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
    }

    #[tokio::test]
    async fn rejects_a_blank_prompt_with_the_ts_text() {
        let mut agent = agent_with(&mock(), Vec::new());
        let error = agent.invoke("   ").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Input prompt is blank. Provide non-blank text or at least one non-text content block."
        );
    }

    #[test]
    fn invoke_args_convert_from_every_input_shape() {
        assert_eq!(InvokeArgs::from("x"), InvokeArgs::Text("x".into()));
        assert!(matches!(
            InvokeArgs::from(vec![ContentBlock::from("x")]),
            InvokeArgs::Content(_)
        ));
        assert!(matches!(
            InvokeArgs::from(Vec::<Message>::new()),
            InvokeArgs::Messages(_)
        ));
        let response = crate::InterruptResponse::new("id", json!(true));
        assert!(matches!(
            InvokeArgs::from(vec![response]),
            InvokeArgs::InterruptResponses(_)
        ));
    }
}

mod structured_output {
    use super::*;

    fn schema() -> serde_json::Value {
        json!({ "type": "object", "properties": { "value": { "type": "number" } }, "required": ["value"] })
    }

    fn so_agent(model: &Arc<MockModel>) -> Agent {
        Agent::new(AgentConfig {
            model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
            structured_output_schema: Some(schema()),
            printer: Some(false),
            ..Default::default()
        })
        .unwrap()
    }

    #[tokio::test]
    async fn returns_structured_output_when_schema_provided_and_tool_used() {
        let model = mock();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": 42 }),
        )]);
        let mut agent = so_agent(&model);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.structured_output_value(), Some(&json!({ "value": 42 })));
        assert_eq!(result.stop_reason(), StopReason::ToolUse);
        assert_eq!(result.to_string(), "{\"value\":42}");
        #[derive(serde::Deserialize)]
        struct Out {
            value: u32,
        }
        assert_eq!(result.structured_output::<Out>().unwrap().value, 42);
    }

    #[tokio::test]
    async fn forces_structured_output_tool_when_model_does_not_use_it() {
        let model = mock();
        model.add_turn(vec!["Plain text, no tool call".into()]);
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": 42 }),
        )]);
        let mut agent = so_agent(&model);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.structured_output_value(), Some(&json!({ "value": 42 })));
        let requests = model.requests();
        assert_eq!(
            requests[1].tool_choice,
            Some(ToolChoice::tool(STRUCTURED_OUTPUT_TOOL_NAME))
        );
        assert_eq!(requests[1].structured_output_schema, Some(schema()));
        assert_eq!(requests[0].structured_output_schema, None);
        assert_eq!(
            requests[1].messages.last().map(|message| message.role),
            Some(Role::User)
        );
    }

    #[tokio::test]
    async fn throws_structured_output_error_when_model_refuses_to_use_tool_after_forcing() {
        let model = mock();
        model.add_turn(vec!["Response".into()]);
        let mut agent = so_agent(&model);
        let error = agent.invoke("Test").await.unwrap_err();
        assert_eq!(error.to_string(), crate::tools::NO_STRUCTURED_OUTPUT_MESSAGE);
    }

    #[tokio::test]
    async fn passes_the_schema_on_the_retry_and_accepts_a_json_text_response() {
        let model = mock();
        model.add_turn(vec!["Plain text, no tool call".into()]);
        model.add_turn(vec!["{\"value\": 42}".into()]);
        let mut agent = so_agent(&model);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.structured_output_value(), Some(&json!({ "value": 42 })));
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(roles(&agent), [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn throws_structured_output_error_when_the_json_text_response_fails_validation() {
        let model = mock();
        model.add_turn(vec!["Plain".into()]);
        model.add_turn(vec!["{\"value\": \"x\"}".into()]);
        let mut agent = so_agent(&model);
        let error = agent.invoke("Test").await.unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Structured output failed validation: "),
            "{error}"
        );
    }

    #[tokio::test]
    async fn retries_with_validation_feedback_when_structured_output_tool_returns_error() {
        let model = mock();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": "bad" }),
        )]);
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t2",
            json!({ "value": 1 }),
        )]);
        let mut agent = so_agent(&model);
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.structured_output_value(), Some(&json!({ "value": 1 })));
        let ContentBlock::ToolResult(feedback) = &agent.messages()[2].content[0] else {
            panic!("tool result expected");
        };
        assert_eq!(feedback.status, crate::tools::ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn cleans_up_structured_output_tool_after_invocation() {
        let model = mock();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": 1 }),
        )]);
        let mut agent = so_agent(&model);
        agent.invoke("Test").await.unwrap();
        assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_none());
    }

    #[tokio::test]
    async fn cleans_up_structured_output_tool_even_when_error_occurs() {
        let model = mock();
        model.add_turn(vec!["Response".into()]);
        let mut agent = so_agent(&model);
        assert!(agent.invoke("Test").await.is_err());
        assert!(agent.tool_registry().get(STRUCTURED_OUTPUT_TOOL_NAME).is_none());
    }

    #[tokio::test]
    async fn uses_per_invocation_override_schema_and_restores_constructor_schema_on_next_call() {
        let model = mock();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": 1 }),
        )]);
        let mut agent = so_agent(&model);
        let other = json!({ "type": "object", "properties": { "value": { "type": "number" } } });
        let options = InvokeOptions {
            structured_output_schema: Some(other.clone()),
            ..Default::default()
        };
        agent.invoke_with("Test", options).await.unwrap();
        agent.invoke("Test").await.unwrap();
        let schemas: Vec<_> = model
            .requests()
            .iter()
            .map(|request| {
                request
                    .tool_specs
                    .iter()
                    .find(|spec| spec.name == STRUCTURED_OUTPUT_TOOL_NAME)
                    .unwrap()
                    .input_schema
                    .clone()
            })
            .collect();
        assert_eq!(schemas, [Some(other), Some(schema())]);
    }

    #[tokio::test]
    async fn honors_after_tools_event_end_turn_over_structured_output() {
        let model = mock();
        model.add_turn(vec![tool_use(
            STRUCTURED_OUTPUT_TOOL_NAME,
            "t1",
            json!({ "value": 1 }),
        )]);
        let mut agent = so_agent(&model);
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(EndTurn::from("stop here")));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "stop here");
    }
}

mod history_rewrites {
    use super::*;

    #[tokio::test]
    async fn replaces_invalid_tool_use_names_with_invalid_tool_name_before_calling_model() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let history = vec![
            Message::new(Role::User, vec!["Hi".into()]),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(
                    ToolUseBlock::new("bad name!", "t1", json!({})).with_reasoning_signature("sig"),
                )],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult(crate::tools::tool::error_result(
                    "t1", "x",
                ))],
            ),
        ];
        let mut agent = Agent::new(AgentConfig {
            model: Some((Arc::clone(&model) as Arc<dyn Model>).into()),
            messages: history,
            printer: Some(false),
            ..Default::default()
        })
        .unwrap();
        agent.invoke(InvokeArgs::empty()).await.unwrap();
        let sent = &model.requests()[0].messages[1].content[0];
        let ContentBlock::ToolUse(sent) = sent else {
            panic!()
        };
        assert_eq!(sent.name, "INVALID_TOOL_NAME");
        assert_eq!(sent.reasoning_signature.as_deref(), Some("sig"));
        let ContentBlock::ToolUse(stored) = &agent.messages()[1].content[0] else {
            panic!()
        };
        assert_eq!(stored.name, "bad name!");
    }

    #[tokio::test]
    async fn sends_it_to_the_model_as_a_blank_text_placeholder_without_changing_stored_history() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let history = vec![
            Message::new(Role::User, vec!["Hi".into()]),
            Message::new(Role::Assistant, Vec::new()),
        ];
        let mut agent = Agent::new(AgentConfig {
            model: Some((Arc::clone(&model) as Arc<dyn Model>).into()),
            messages: history,
            printer: Some(false),
            ..Default::default()
        })
        .unwrap();
        agent.invoke("again").await.unwrap();
        assert_eq!(
            model.requests()[0].messages[1].content,
            vec![ContentBlock::from("[blank text]")]
        );
        assert!(agent.messages()[1].content.is_empty());
    }

    #[tokio::test]
    async fn the_model_sees_reasoning_and_text_of_the_previous_turn() {
        let model = mock();
        model.add_turn(vec![
            ContentBlock::Reasoning(ReasoningBlock::new().with_text("think")),
            "answer".into(),
        ]);
        let mut agent = agent_with(&model, Vec::new());
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(result.to_string(), "💭 Reasoning:\n   think\nanswer");
    }
}

mod estimate_input_tokens {
    use super::*;

    async fn projected(agent: &mut Agent) -> Vec<Option<u64>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            sink.lock().unwrap().push(event.projected_input_tokens());
        });
        agent.invoke("Hello there").await.unwrap();
        seen.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn uses_full_estimation_on_cold_start_no_prior_usage_metadata() {
        let model = mock();
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let estimate = projected(&mut agent).await;
        assert!(estimate[0].is_some_and(|tokens| tokens > 0));
    }

    #[tokio::test]
    async fn uses_baseline_from_prior_invocation_on_second_invoke() {
        let model = mock();
        model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(100, 20, 120)));
        let mut agent = agent_with(&model, Vec::new());
        agent.invoke("first").await.unwrap();
        let estimate = projected(&mut agent).await[0].unwrap();
        assert!(estimate > 120, "{estimate}");
    }
}

mod limits {
    use super::*;

    fn limited(turns: Option<u64>, output: Option<u64>, total: Option<u64>) -> InvokeOptions {
        InvokeOptions {
            limits: Some(InvokeLimits {
                turns,
                output_tokens: output,
                total_tokens: total,
                structured_output_attempts: None,
            }),
            ..Default::default()
        }
    }

    fn looping() -> (Arc<MockModel>, Agent) {
        let model = mock();
        model.add_turn_with(
            vec![tool_use("calc", "t1", json!({}))],
            None,
            Some(Usage::new(10, 10, 20)),
        );
        let agent = agent_with(
            &model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        );
        (model, agent)
    }

    #[tokio::test]
    async fn runs_the_cycle_to_completion_and_bails_at_top_of_next_iteration() {
        let (model, mut agent) = looping();
        let result = agent
            .invoke_with("go", limited(Some(1), None, None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(model.requests().len(), 1);
        assert_eq!(roles(&agent), [Role::User, Role::Assistant, Role::User]);
        assert_eq!(result.last_message().role, Role::User);
    }

    #[tokio::test]
    async fn returns_limit_output_tokens_once_cumulative_output_tokens_hits_the_limit() {
        let (_, mut agent) = looping();
        let result = agent
            .invoke_with("go", limited(None, Some(20), None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
    }

    #[tokio::test]
    async fn returns_limit_total_tokens_once_cumulative_total_tokens_hits_the_limit() {
        let (_, mut agent) = looping();
        let result = agent
            .invoke_with("go", limited(None, None, Some(40)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTotalTokens);
    }

    #[tokio::test]
    async fn prefers_turns_when_all_three_trip() {
        let (_, mut agent) = looping();
        let result = agent
            .invoke_with("go", limited(Some(1), Some(1), Some(1)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    }

    #[tokio::test]
    async fn prefers_total_tokens_over_output_tokens() {
        let (_, mut agent) = looping();
        let result = agent
            .invoke_with("go", limited(None, Some(1), Some(1)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTotalTokens);
    }

    #[tokio::test]
    async fn returns_end_turn_the_model_answer_wins() {
        let model = mock();
        model.add_turn_with(vec!["done".into()], None, Some(Usage::new(10, 10, 20)));
        let mut agent = agent_with(&model, Vec::new());
        let result = agent
            .invoke_with("go", limited(Some(1), None, None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn scopes_the_limit_to_the_current_invocation_not_lifetime() {
        let model = mock();
        model.add_turn(vec!["done".into()]);
        let mut agent = agent_with(&model, Vec::new());
        for _ in 0..3 {
            let result = agent
                .invoke_with("go", limited(Some(1), None, None))
                .await
                .unwrap();
            assert_eq!(result.stop_reason(), StopReason::EndTurn);
        }
    }

    #[tokio::test]
    async fn rejects_an_invalid_limit_with_the_ts_text() {
        let mut agent = agent_with(&mock(), Vec::new());
        let error = agent
            .invoke_with("go", limited(Some(0), None, None))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "limits.turns must be a positive finite number, got 0"
        );
    }

    fn heavy(output_tokens: u64) -> (Arc<MockModel>, Agent) {
        let model = mock();
        for id in ["tool-1", "tool-2"] {
            model.add_turn_with(
                vec![tool_use("loop", id, json!({}))],
                None,
                Some(Usage::new(10, output_tokens, output_tokens + 10)),
            );
        }
        let agent = agent_with(
            &model,
            crate::tool_list![mock_tool("loop", |ctx| text_result(ctx, "ok"))],
        );
        (model, agent)
    }

    #[tokio::test]
    async fn does_not_trip_and_the_model_ends_naturally() {
        let model = mock();
        model.add_turn_with(vec!["done".into()], None, Some(Usage::new(5, 5, 10)));
        let mut agent = agent_with(&model, Vec::new());
        let result = agent
            .invoke_with("go", limited(Some(5), Some(1000), Some(1000)))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.to_string(), "done");
        assert_eq!(result.metrics().cycle_count, 1);
        assert_eq!(result.metrics().accumulated_usage, Usage::new(5, 5, 10));
    }

    #[tokio::test]
    async fn returns_limit_output_tokens_after_the_second_cycle_crosses_the_limit() {
        let (_, mut agent) = heavy(60);
        let result = agent
            .invoke_with("go", limited(None, Some(100), None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
        assert_eq!(result.last_message().role, Role::User);
        assert!(matches!(
            result.last_message().content[0],
            ContentBlock::ToolResult(_)
        ));
        assert_eq!(result.metrics().cycle_count, 2);
        assert_eq!(result.metrics().accumulated_usage, Usage::new(20, 120, 140));
        assert!(result.metrics().tool_metrics.contains_key("loop"));
    }

    #[tokio::test]
    async fn uses_at_most_semantics_stops_when_count_exactly_equals_the_limit() {
        let (_, mut agent) = heavy(100);
        let result = agent
            .invoke_with("go", limited(None, Some(100), None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
        assert_eq!(result.metrics().cycle_count, 1);
        assert_eq!(result.metrics().accumulated_usage, Usage::new(10, 100, 110));
    }

    #[tokio::test]
    async fn falls_back_to_output_tokens_when_no_higher_priority_limit_is_set() {
        let (_, mut agent) = heavy(100);
        let result = agent
            .invoke_with("go", limited(None, Some(1), None))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitOutputTokens);
        assert_eq!(result.metrics().cycle_count, 1);
    }

    #[tokio::test]
    async fn scopes_the_output_token_limit_to_the_current_invocation() {
        let model = mock();
        model.add_turn_with(vec!["first".into()], None, Some(Usage::new(10, 50, 60)));
        model.add_turn_with(vec!["second".into()], None, Some(Usage::new(10, 50, 60)));
        let mut agent = agent_with(&model, Vec::new());
        for _ in 0..2 {
            let result = agent
                .invoke_with("go", limited(None, Some(75), None))
                .await
                .unwrap();
            assert_eq!(result.stop_reason(), StopReason::EndTurn);
            assert_eq!(
                result.metrics().latest_agent_invocation().unwrap().cycles.len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn rejects_zero_structured_output_attempts_with_type_error() {
        let model = mock();
        model.add_turn(vec!["never reached".into()]);
        let mut agent = agent_with(&model, Vec::new());
        let options = InvokeOptions {
            limits: Some(InvokeLimits {
                structured_output_attempts: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let error = agent.invoke_with("go", options).await.unwrap_err();
        assert!(matches!(error, Error::Type(_)), "{error:?}");
        assert_eq!(model.call_count(), 0);
    }

    #[tokio::test]
    async fn returns_limit_turns_as_the_generator_return_value() {
        let (_, mut agent) = looping();
        let mut last = None;
        let mut stream = agent.stream_with("go", limited(Some(1), None, None));
        while let Some(event) = stream.next().await {
            last = Some(event.unwrap());
        }
        let Some(AgentStreamEvent::AgentResult(result)) = last else {
            panic!()
        };
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    }
}

#[tokio::test]
async fn redacts_last_user_message_and_keeps_the_tracking_id() {
    use crate::models::{
        ModelMessageStartEvent, ModelMessageStopEvent, ModelRedactionEvent, RedactInputContent,
    };
    let events = vec![
        ModelMessageStartEvent::new(Role::Assistant).into(),
        ModelRedactionEvent {
            input_redaction: Some(RedactInputContent::new("[redacted]")),
            ..ModelRedactionEvent::new()
        }
        .into(),
        ModelMessageStopEvent::new(StopReason::GuardrailIntervened).into(),
    ];
    let model = crate::testing::TestModelProvider::new(events);
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let message = Message::new(Role::User, vec!["secret".into()]).with_tracking_id("keep-me");
    agent.invoke(message).await.unwrap();
    let first = &agent.messages()[0];
    assert_eq!(first.content, vec![ContentBlock::from("[redacted]")]);
    assert_eq!(first.tracking_id(), "keep-me");
    let _ = ToolResultContent::from("unused");
}
