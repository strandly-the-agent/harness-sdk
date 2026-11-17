//! `describe('subagent depth guard')`, `describe('subagent cancellation and failure')` and
//! `describe('subagent interrupt propagation')`.

use super::*;

fn generalist_over(model: &Arc<MockMessageModel>) -> (MakeSubagentOptions, Arc<Built>) {
    let (builder, built) = builder_over(model);
    let options = MakeSubagentOptions::builder()
        .builder(calling(builder))
        .presets(generalist())
        .build();
    (options, built)
}

/// Drives a tool whose child answers `reply` with `stop_reason`; the result and the tool.
async fn stopped(reply: &str, stop_reason: StopReason) -> (ToolResultBlock, SubagentTool) {
    let (options, _) = generalist_over(&replying(reply, stop_reason));
    let tool = SubagentTool::new(options).unwrap();
    let ToolOutcome::Result(result) = drive_in(&tool, context(json!({"task": "x"}))).await.1 else {
        panic!("expected a result");
    };
    (result, tool)
}

async fn drive_at_depth(max_depth: u32, stored: Option<JsonValue>) -> (ToolOutcome, Arc<Built>) {
    let (mut options, built) = generalist_over(&replying("done", StopReason::EndTurn));
    options.max_depth = Some(max_depth);
    let tool = make_subagent(options).unwrap();
    let context = context(json!({"task": "x"}));
    if let Some(depth) = stored {
        context.agent().app_state().set(DEPTH_STATE_KEY, depth).unwrap();
    }
    (drive_in(tool.as_ref(), context).await.1, built)
}

fn child_depth(built: &Built) -> Option<JsonValue> {
    built.states.lock().unwrap()[0].get_value(DEPTH_STATE_KEY)
}

#[tokio::test]
async fn refuses_once_the_budget_is_exhausted_without_building_a_child() {
    let (outcome, built) = drive_at_depth(2, Some(json!(0))).await;
    let ToolOutcome::Result(result) = outcome else {
        panic!("expected a result");
    };
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert_eq!(
        result_text(&result),
        "Delegation depth limit reached (2 levels); you cannot delegate further. Complete this task yourself instead of calling subagent again."
    );
    // No child is constructed once the budget is exhausted.
    assert!(built.specs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn fails_closed_on_a_stored_depth_that_is_not_a_number() {
    for stored in [json!("deep"), json!({"depth": 2}), json!([2, 2])] {
        let (outcome, built) = drive_at_depth(2, Some(stored)).await;
        let ToolOutcome::Result(result) = outcome else {
            panic!("expected a result");
        };
        assert_eq!(result.status(), &ToolResultStatus::Error);
        assert!(built.specs.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn decrements_the_budget_onto_the_childs_own_state() {
    let (_, built) = drive_at_depth(3, Some(json!(2))).await;
    assert_eq!(child_depth(&built), Some(json!(1)));
}

#[tokio::test]
async fn starts_a_parent_that_never_delegated_at_max_depth() {
    let (_, built) = drive_at_depth(3, None).await;
    assert_eq!(child_depth(&built), Some(json!(2)));
}

#[tokio::test]
async fn turns_a_cancelled_child_into_an_error_result() {
    // A cancelled child is an error, not a bogus success, and is dropped from the pending map.
    let (result, tool) = stopped("partial", StopReason::Cancelled).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert_eq!(
        result_text(&result),
        "Subagent stopped before finishing (stopReason=cancelled).\n\nPartial output:\npartial"
    );
    assert!(tool.shared.pending().is_empty());
}

#[tokio::test]
async fn turns_a_thrown_child_error_into_an_error_result() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| ModelError::new("kaboom").into());
    let (options, _) = generalist_over(&model);
    let result = drive(options, json!({"task": "x"})).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert!(result_text(&result).starts_with("Subagent error: "));
    assert!(result_text(&result).contains("kaboom"));
}

#[tokio::test]
async fn turns_a_s_child_into_an_error_result_naming_the_stop_reason() {
    for stop_reason in [StopReason::ContentFiltered, StopReason::Refusal] {
        let (result, tool) = stopped("I'll retrieve that now.\n", stop_reason.clone()).await;
        assert_eq!(result.status(), &ToolResultStatus::Error);
        assert_eq!(result.tool_use_id(), "t1");
        assert_eq!(
            result_text(&result),
            format!(
                "Subagent stopped before finishing (stopReason={stop_reason}).\n\nPartial output:\nI'll retrieve that now."
            )
        );
        assert!(tool.shared.pending().is_empty());
    }
}

#[tokio::test]
async fn reports_only_the_stop_reason_when_a_blocked_child_produced_no_output() {
    let (result, _) = stopped("", StopReason::ContentFiltered).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert_eq!(
        result_text(&result),
        "Subagent stopped before finishing (stopReason=contentFiltered)."
    );
}

#[tokio::test]
async fn returns_a_s_childs_text_as_a_success() {
    for stop_reason in ["guardrailIntervened", "limitTurns", "stopSequence"] {
        let (result, _) = stopped("the text\n", StopReason::from(stop_reason)).await;
        assert_eq!(result.status(), &ToolResultStatus::Success, "{stop_reason}");
        assert_eq!(result_text(&result), "the text\n");
    }
}

#[tokio::test]
async fn forwards_child_progress_as_full_serialized_child_events() {
    let (options, _) = generalist_over(&replying("done", StopReason::EndTurn));
    let tool = make_subagent(options).unwrap();
    let (events, _) = drive_in(tool.as_ref(), context(json!({"task": "x"}))).await;
    let types: Vec<&str> = events
        .iter()
        .map(|event| event.data().unwrap()["type"].as_str().unwrap())
        .collect();
    assert!(types.contains(&"agentResultEvent"), "{types:?}");
    assert!(types.len() > 1, "{types:?}");
}

#[tokio::test]
async fn propagates_a_child_interrupt_through_tool_context_interrupt_and_resumes() {
    // The child's tool asks for approval; the parent's `subagent` call must stop on that interrupt
    // and resume the same child once the parent's caller answers.
    let approvals = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&approvals);
    let gate = FunctionTool::new(
        "gate",
        "asks for approval",
        move |_input: JsonValue, ctx: ToolContext| {
            let record = Arc::clone(&record);
            async move {
                let response = ctx.interrupt(InterruptParams {
                    name: "gate".to_owned(),
                    reason: Some(json!("approve?")),
                    response: None,
                })?;
                record.lock().unwrap().push(response);
                Ok::<_, ToolError>("approved")
            }
        },
    );
    let child_model = Arc::new(MockMessageModel::new());
    child_model.add_turn(vec![tool_use("gate", "c1", json!({}))]);
    child_model.add_turn(vec![text("resumed answer")]);
    let child = Agent::builder()
        .model(Arc::clone(&child_model))
        .tools([gate])
        .build()
        .unwrap();
    let options = MakeSubagentOptions::builder()
        .builder(calling(builder_of(child)))
        .presets(generalist())
        .build();
    let tool = make_subagent(options).unwrap();

    let parent_model = Arc::new(MockMessageModel::new());
    parent_model.add_turn(vec![tool_use("subagent", "t1", json!({"task": "gated work"}))]);
    parent_model.add_turn(vec![text("parent done")]);
    let mut parent = Agent::builder()
        .model(Arc::clone(&parent_model))
        .tools([tool])
        .build()
        .unwrap();

    // First call: the child interrupts and the parent stops on the re-raised interrupt.
    let interrupted = parent.invoke("go").await.unwrap();
    assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
    let [interrupt] = interrupted.interrupts() else {
        panic!("expected one interrupt, got {:?}", interrupted.interrupts());
    };
    assert_eq!(interrupt.id(), "tool:t1:tool:c1:gate");
    assert_eq!(interrupt.reason(), Some(&json!("approve?")));
    assert!(approvals.lock().unwrap().is_empty());

    // The caller approves; the tool call re-runs and resumes the retained child.
    let answer = vec![InterruptResponse::new(
        interrupt.id(),
        json!({"decision": "approve"}),
    )];
    let result = parent.invoke(answer).await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    // The child got the response, not the original task again.
    assert_eq!(*approvals.lock().unwrap(), [json!({"decision": "approve"})]);
    assert_eq!(child_model.call_count(), 2);
    let tool_results: Vec<&ToolResultBlock> = parent
        .messages()
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    let [tool_result] = tool_results.as_slice() else {
        panic!("expected one tool result, got {tool_results:?}");
    };
    assert_eq!(tool_result.status(), &ToolResultStatus::Success);
    assert_eq!(result_text(tool_result), "resumed answer");
}

#[tokio::test]
async fn the_child_joins_the_parents_abort() {
    // The child's tool cancels the parent; the child's own signal follows.
    let parent_handle = Arc::new(Mutex::new(None::<strands::CancelHandle>));
    let seen = Arc::new(Mutex::new(None));
    let (handle, record) = (Arc::clone(&parent_handle), Arc::clone(&seen));
    let stopper = FunctionTool::new(
        "stopper",
        "cancels the parent",
        move |_input: JsonValue, ctx: ToolContext| {
            let (handle, record) = (Arc::clone(&handle), Arc::clone(&record));
            async move {
                if let Some(handle) = handle.lock().unwrap().as_ref() {
                    handle.cancel(strands::CancelOptions::default());
                }
                *record.lock().unwrap() = Some(ctx.cancel_signal().is_cancelled());
                Ok::<_, ToolError>("ok")
            }
        },
    );
    let child_model = Arc::new(MockMessageModel::new());
    child_model.add_turn(vec![tool_use("stopper", "c1", json!({}))]);
    child_model.add_turn(vec![text("never said")]);
    let child = Agent::builder()
        .model(Arc::clone(&child_model))
        .tools([stopper])
        .build()
        .unwrap();
    let options = MakeSubagentOptions::builder()
        .builder(calling(builder_of(child)))
        .build();
    let parent_model = Arc::new(MockMessageModel::new());
    parent_model.add_turn(vec![tool_use("subagent", "t1", json!({"task": "x"}))]);
    parent_model.add_turn(vec![text("parent done")]);
    let mut parent = Agent::builder()
        .model(parent_model)
        .tools([make_subagent(options).unwrap()])
        .build()
        .unwrap();
    *parent_handle.lock().unwrap() = Some(parent.cancel_handle());

    let result = parent.invoke("go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(*seen.lock().unwrap(), Some(true));
    assert_eq!(child_model.call_count(), 1);
}

#[tokio::test]
async fn the_child_model_sees_the_parents_history_under_context_all() {
    // End to end through a real parent: the delegating call is still in flight, so the child's
    // history is the parent's earlier turns followed by the framed task.
    let child_model = replying("child done", StopReason::EndTurn);
    let (builder, _) = builder_over(&child_model);
    let options = MakeSubagentOptions::builder()
        .builder(calling(builder))
        .context(none_or("all"))
        .build();
    let parent_model = Arc::new(MockMessageModel::new());
    parent_model.add_turn(vec![text("hello")]);
    parent_model.add_turn(vec![tool_use(
        "subagent",
        "t1",
        json!({"task": "do X", "context": "all"}),
    )]);
    parent_model.add_turn(vec![text("parent done")]);
    let mut parent = Agent::builder()
        .model(parent_model)
        .tools([make_subagent(options).unwrap()])
        .build()
        .unwrap();
    parent.invoke("hi").await.unwrap();
    parent.invoke("delegate").await.unwrap();

    let requests = child_model.requests();
    let [request] = requests.as_slice() else {
        panic!("expected one child model call, got {}", requests.len());
    };
    assert_eq!(
        shape_of(&request.messages),
        shape_of(&[
            user(vec![text("hi")]),
            assistant(vec![text("hello")]),
            user(vec![text("delegate"), framed("do X")]),
        ])
    );
}
