//! Port of `harness-ts/test/tools/programmatic-tool-caller.test.ts`.
//!
//! Inner calls here go through `ungated`, a test-only inner caller that runs the registry tool
//! directly; the tools the crate hands out refuse inner calls until the SDK can gate them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use serde_json::json;
use strands::agent::{Agent, DirectToolCallOptions};
use strands::testing::MockMessageModel;
use strands::tools::{FunctionTool, ToolStreamItem};
use strands::types::messages::{ToolResultBlock, ToolResultContent};
use strands::types::tools::{ToolResultStatus, ToolUse};
use strands::{CancelOptions, CancelSignal, JsonValue, ToolContext, ToolError, ToolOutcome, ToolSource};

use super::output::Output;
use super::run::{HostCall, LIMITS, Limits, MAX_CONCURRENT_TOOL_CALLS, RunEnd, run_code};
use super::{
    DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, InnerCall, InnerCaller, MakeProgrammaticToolCallerOptions,
    make_programmatic_tool_caller_with, programmatic_tool_caller,
};

/// Runs the registry tool directly, with no hooks.
fn ungated() -> InnerCaller {
    Arc::new(|parent: ToolContext, name: String, input: JsonValue| {
        async move {
            let tool = Arc::clone(
                parent
                    .agent()
                    .tool_registry()
                    .resolve(&name)
                    .map_err(|error| error.to_string())?,
            );
            let context = strands::testing::tool_context(ToolUse::new(name, "ptc_test", input));
            let mut stream = tool.stream(context);
            while let Some(item) = stream.next().await {
                if let ToolStreamItem::Outcome(ToolOutcome::Result(result)) = item {
                    return Ok(InnerCall::Result(result));
                }
            }
            Err("the tool returned no result".to_owned())
        }
        .boxed()
    })
}

fn caller_with(options: MakeProgrammaticToolCallerOptions) -> FunctionTool {
    make_programmatic_tool_caller_with(options, ungated(), LIMITS)
}

fn caller() -> FunctionTool {
    caller_with(MakeProgrammaticToolCallerOptions::default())
}

fn agent_of(tools: Vec<FunctionTool>) -> Agent {
    let tools: Vec<ToolSource> = tools.into_iter().map(Into::into).collect();
    Agent::builder()
        .model(MockMessageModel::new())
        .tools(tools)
        .build()
        .expect("the agent builds")
}

fn agent_with(tools: Vec<FunctionTool>) -> Agent {
    agent_of(std::iter::once(caller()).chain(tools).collect())
}

async fn run_named(agent: &mut Agent, code: &str, name: &str) -> ToolResultBlock {
    let options = DirectToolCallOptions {
        record_direct_tool_call: false,
    };
    agent
        .tool(name)
        .invoke_with(json!({ "code": code }), options)
        .await
        .expect("the direct call returns a result")
}

async fn run(agent: &mut Agent, code: &str) -> ToolResultBlock {
    run_named(agent, code, "programmatic_tool_caller").await
}

fn text(result: &ToolResultBlock) -> String {
    let parts: Vec<&str> = result
        .content()
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => text.text(),
            _ => "",
        })
        .collect();
    parts.join("\n")
}

fn is_error(result: &ToolResultBlock) -> bool {
    *result.status() == ToolResultStatus::Error
}

/// Evaluates `a op b` or a lone number, which is all the cases ask of the TS `calculator`.
fn calculator() -> FunctionTool {
    FunctionTool::new(
        "calculator",
        "Evaluate a simple arithmetic expression.",
        |input: JsonValue, _context| async move {
            let expression = input["expression"].as_str().unwrap_or_default().to_owned();
            let tokens: Vec<&str> = expression.split_whitespace().collect();
            let number = |token: &str| token.parse::<i64>().map_err(ToolError::msg);
            let value = match tokens.as_slice() {
                [only] => number(only)?,
                [left, "+", right] => number(left)? + number(right)?,
                [left, "*", right] => number(left)? * number(right)?,
                _ => return Err(ToolError::msg(format!("cannot evaluate {expression}"))),
            };
            Ok(value.to_string())
        },
    )
}

fn boom() -> FunctionTool {
    FunctionTool::new("boom", "Always fails.", |_input, _context| async move {
        Err::<String, _>(ToolError::msg("kaboom"))
    })
}

fn info() -> FunctionTool {
    FunctionTool::new(
        "info",
        "Returns structured data.",
        |_input, _context| async move { Ok(json!({"name": "demo", "tags": ["a", "b"]})) },
    )
}

fn fetch_url() -> FunctionTool {
    FunctionTool::new(
        "fetch-url",
        "A tool whose name is not a valid identifier.",
        |input: JsonValue, _context| async move {
            Ok(format!("dash:{}", input["value"].as_str().unwrap_or_default()))
        },
    )
}

#[tokio::test]
async fn runs_a_single_tool_call() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print(await calculator(expression=\"10 * 2\"))").await;
    assert!(!is_error(&result));
    assert_eq!(text(&result), "20");
}

#[tokio::test]
async fn loops_over_tool_calls() {
    let mut agent = agent_with(vec![calculator()]);
    let code = "for i in range(3):\n    print(await calculator(expression=f'{i} + 10'))";
    assert_eq!(text(&run(&mut agent, code).await), "10\n11\n12");
}

#[tokio::test(start_paused = true)]
async fn runs_tool_calls_concurrently_with_asyncio_gather() {
    let slow = FunctionTool::new(
        "slow",
        "Sleep, then return the value.",
        |input: JsonValue, _context| async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(input["value"].to_string())
        },
    );
    let mut agent = agent_with(vec![slow]);
    let start = tokio::time::Instant::now();
    let code = "import asyncio\nprint(await asyncio.gather(*[slow(value=i) for i in range(4)]))";
    assert_eq!(text(&run(&mut agent, code).await), "['0', '1', '2', '3']");
    // Serial calls would take 1200 ms of the virtual clock.
    assert!(start.elapsed() < Duration::from_millis(1100));
}

#[tokio::test(start_paused = true)]
async fn bounds_how_many_inner_calls_run_at_once() {
    let now = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (running, highest) = (Arc::clone(&now), Arc::clone(&peak));
    let slow = FunctionTool::new(
        "slow",
        "Track how many calls overlap.",
        move |input: JsonValue, _context| {
            let (running, highest) = (Arc::clone(&running), Arc::clone(&highest));
            async move {
                highest.fetch_max(running.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(50)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                Ok(input["value"].to_string())
            }
        },
    );
    let count = MAX_CONCURRENT_TOOL_CALLS * 3;
    let mut agent = agent_with(vec![slow]);
    let code =
        format!("import asyncio\nprint(len(await asyncio.gather(*[slow(value=i) for i in range({count})])))");
    assert_eq!(text(&run(&mut agent, &code).await), count.to_string());
    assert!(peak.load(Ordering::SeqCst) > 1);
    assert!(peak.load(Ordering::SeqCst) <= MAX_CONCURRENT_TOOL_CALLS);
}

#[tokio::test]
async fn returns_only_printed_output() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(
        &mut agent,
        "x = await calculator(expression=\"6 * 7\")\nprint(\"done\")",
    )
    .await;
    assert_eq!(text(&result), "done");
}

#[tokio::test]
async fn reports_no_output_for_code_that_prints_nothing() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "# nothing here").await;
    assert!(!is_error(&result));
    assert_eq!(text(&result), "(no output)");
}

#[tokio::test]
async fn rejects_positional_arguments_with_a_hint() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print(await calculator(\"1 + 1\"))").await;
    assert!(is_error(&result));
    assert!(
        text(&result).contains("keyword arguments only"),
        "{}",
        text(&result)
    );
    assert!(text(&result).contains("calculator(key=value)"));
}

#[tokio::test]
async fn parses_a_json_text_result_and_leaves_other_text_alone() {
    let json_text = FunctionTool::new(
        "json_text",
        "Returns JSON as text.",
        |_input, _context| async move { Ok("{\"a\": [1, 2]}") },
    );
    let mut agent = agent_with(vec![json_text, calculator()]);
    let code = "r = await json_text()\nprint(type(r).__name__, r[\"a\"][1])\ns = await calculator(expression=\"1 + 1\")\nprint(type(s).__name__, s)";
    assert_eq!(text(&run(&mut agent, code).await), "dict 2\nstr 2");
}

#[tokio::test]
async fn hands_structured_results_to_the_code_as_data() {
    let mut agent = agent_with(vec![info()]);
    let result = run(
        &mut agent,
        "r = await info()\nprint(type(r).__name__, r[\"tags\"][1])",
    )
    .await;
    assert_eq!(text(&result), "dict b");
}

/// The `eval` row of the TS table is left out: Monty 1.5 implements `eval` inside the sandbox
/// (TS runs Monty 0.0.23, where the name does not exist).
#[tokio::test]
async fn keeps_the_host_unreachable() {
    let cases = [
        ("import os\nprint(os.listdir('/'))", "PermissionError"),
        ("print(open('/etc/passwd').read())", "PermissionError"),
        ("import subprocess", "ModuleNotFoundError"),
        ("import socket", "ModuleNotFoundError"),
        ("print(().__class__.__bases__)", "AttributeError"),
        ("print(__builtins__)", "NameError"),
    ];
    let mut agent = agent_with(vec![calculator()]);
    for (code, expected) in cases {
        let result = run(&mut agent, code).await;
        assert!(is_error(&result), "{code}");
        assert!(text(&result).contains(expected), "{code}: {}", text(&result));
    }
}

#[tokio::test]
async fn bounds_a_synchronous_busy_loop_via_the_vm_duration_limit() {
    let limits = Limits {
        max_duration: Duration::from_millis(300),
        ..LIMITS
    };
    let quick =
        make_programmatic_tool_caller_with(MakeProgrammaticToolCallerOptions::default(), ungated(), limits);
    let mut agent = agent_of(vec![quick, calculator()]);
    let start = std::time::Instant::now();
    let result = run(&mut agent, "while True:\n    pass").await;
    assert!(is_error(&result));
    assert!(text(&result).contains("TimeoutError"), "{}", text(&result));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn surfaces_a_tool_error_as_a_catchable_runtime_error() {
    let mut agent = agent_with(vec![boom()]);
    let code = "try:\n    await boom()\nexcept RuntimeError as e:\n    print('caught:', e)";
    let result = run(&mut agent, code).await;
    assert!(!is_error(&result));
    assert_eq!(text(&result), "caught: Tool 'boom' error: Error: kaboom");
}

#[tokio::test]
async fn keeps_output_printed_before_an_error() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print('step 1')\nraise ValueError('late')").await;
    assert!(is_error(&result));
    assert!(
        text(&result).starts_with("Error: step 1\n\nExecution error:\nTraceback"),
        "{}",
        text(&result)
    );
    assert!(text(&result).contains("ValueError: late"));
}

#[tokio::test]
async fn reports_a_syntax_error() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "def broken(:\n    pass").await;
    assert!(is_error(&result));
    assert!(text(&result).contains("Syntax error:"), "{}", text(&result));
    assert!(text(&result).contains("SyntaxError"));
}

#[tokio::test]
async fn reports_a_runtime_error_with_the_user_line_number() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "x = 1\ny = 2\nz = {}['missing']").await;
    assert!(is_error(&result));
    assert!(text(&result).contains("line 3"), "{}", text(&result));
    assert!(text(&result).contains("KeyError"));
}

#[tokio::test]
async fn does_not_expose_itself_to_the_code() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "programmatic_tool_caller").await;
    assert!(is_error(&result));
    assert!(text(&result).contains("NameError"), "{}", text(&result));
}

#[tokio::test]
async fn filters_exposed_tools_with_allowed_tools() {
    let restricted = caller_with(
        MakeProgrammaticToolCallerOptions::builder()
            .allowed_tools(vec!["calculator".to_owned()])
            .name("run_code")
            .build(),
    );
    let mut agent = agent_of(vec![restricted, calculator(), boom()]);
    let result = run_named(
        &mut agent,
        "print(await calculator(expression=\"1 + 1\"))",
        "run_code",
    )
    .await;
    assert_eq!(text(&result), "2");
    let result = run_named(&mut agent, "await boom()", "run_code").await;
    assert!(is_error(&result));
    assert!(text(&result).contains("NameError"), "{}", text(&result));
}

#[test]
fn states_the_contract_in_the_description() {
    for phrase in [
        "await",
        "print()",
        "RuntimeError",
        "keyword arguments",
        "fetch_url",
    ] {
        assert!(
            DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION.contains(phrase),
            "{phrase}"
        );
    }
}

#[tokio::test]
async fn never_exposes_another_programmatic_tool_caller() {
    let other = caller_with(
        MakeProgrammaticToolCallerOptions::builder()
            .name("run_code")
            .allowed_tools(vec![
                "programmatic_tool_caller".to_owned(),
                "calculator".to_owned(),
            ])
            .build(),
    );
    let mut agent = agent_of(vec![caller(), other, calculator()]);
    for (caller, target) in [
        ("run_code", "programmatic_tool_caller"),
        ("programmatic_tool_caller", "run_code"),
    ] {
        let result = run_named(&mut agent, &format!("await {target}(code='print(1)')"), caller).await;
        assert!(is_error(&result));
        assert!(text(&result).contains("NameError"), "{}", text(&result));
    }
    let result = run_named(
        &mut agent,
        "print(await calculator(expression=\"1 + 1\"))",
        "run_code",
    )
    .await;
    assert_eq!(text(&result), "2");
}

#[tokio::test]
async fn calls_a_hyphenated_tool_via_its_identifier_alias() {
    let mut agent = agent_with(vec![fetch_url()]);
    let result = run(&mut agent, "print(await fetch_url(value=\"x\"))").await;
    assert_eq!(text(&result), "dash:x");
}

fn ends_with_truncation_marker(text: &str) -> bool {
    text.rsplit_once("\n[output truncated at ")
        .is_some_and(|(_, tail)| {
            tail.strip_suffix(" characters]").is_some_and(|digits| {
                !digits.is_empty() && digits.chars().all(|digit| digit.is_ascii_digit())
            })
        })
}

#[tokio::test]
async fn truncates_an_error_message_that_is_too_long() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print('before')\nraise ValueError('x' * 600000)").await;
    assert!(is_error(&result));
    assert!(text(&result).starts_with("Error: before\n\nExecution error:"));
    assert!(ends_with_truncation_marker(&text(&result)));
    assert!(text(&result).len() < 200_100);
}

#[tokio::test]
async fn shares_the_cap_between_printed_output_and_the_error() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print('p' * 210000)\nraise ValueError('e' * 210000)").await;
    assert!(text(&result).starts_with("Error: ppp"));
    assert!(text(&result).contains("ValueError: eee"));
    assert!(text(&result).len() < 200_100);
}

#[tokio::test]
async fn truncates_a_syntax_error_that_is_too_long() {
    let mut agent = agent_with(vec![calculator()]);
    let code = format!("x = '{}' +", "A".repeat(400_000));
    let result = run(&mut agent, &code).await;
    assert!(
        text(&result).starts_with("Error: Syntax error:"),
        "{}",
        &text(&result)[..200]
    );
    assert!(text(&result).len() < 200_100);
}

#[test]
fn does_not_buffer_an_oversized_print_in_full() {
    let mut output = Output::default();
    output.write(&"a".repeat(200_000 * 5));
    output.write(&"b".repeat(100));
    assert!(output.buffered_chars() <= 200_001);
    assert!(output.text().ends_with("[output truncated at 200000 characters]"));
}

#[tokio::test]
async fn truncates_output_that_is_too_long() {
    let mut agent = agent_with(vec![calculator()]);
    let result = run(&mut agent, "print('x' * 200500)").await;
    assert!(text(&result).ends_with("[output truncated at 200000 characters]"));
    assert!(text(&result).len() < 200_100);
}

#[tokio::test(start_paused = true)]
async fn enforces_the_timeout() {
    let slow = FunctionTool::new(
        "slow",
        "Block longer than the timeout.",
        |_input, _context| async move {
            tokio::time::sleep(Duration::from_millis(5000)).await;
            Ok("late")
        },
    );
    let quick = caller_with(
        MakeProgrammaticToolCallerOptions::builder()
            .name("quick")
            .timeout(Some(Duration::from_millis(300)))
            .build(),
    );
    let mut agent = agent_of(vec![quick, slow]);
    let result = run_named(&mut agent, "print('start')\nawait slow()", "quick").await;
    assert!(is_error(&result));
    assert_eq!(
        text(&result),
        "Error: start\n\nExecution error: timed out after 0.3 seconds."
    );
}

fn functions(names: &[&str]) -> HashMap<String, String> {
    names
        .iter()
        .map(|name| ((*name).to_owned(), (*name).to_owned()))
        .collect()
}

/// Driver-level port: the direct-call API has no way to hand the tool a cancel signal of the
/// test's own.
#[tokio::test]
async fn honors_the_parent_cancel_signal() {
    let (signal, handle) = CancelSignal::new();
    handle.cancel(CancelOptions::default());
    let call: HostCall = Arc::new(|_name, _input| async { Ok(json!("1")) }.boxed());
    let code = "await calculator(expression=\"1\")".to_owned();
    let (_, end) = run_code(code, functions(&["calculator"]), call, LIMITS, None, &signal).await;
    assert!(matches!(end, RunEnd::Cancelled), "{end:?}");
}

#[tokio::test(start_paused = true)]
async fn cancels_an_in_flight_run_and_makes_no_further_inner_tool_calls() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let call: HostCall = Arc::new(move |_name, _input| {
        let seen = Arc::clone(&seen);
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(json!("ok"))
        }
        .boxed()
    });
    let (signal, handle) = CancelSignal::new();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.cancel(CancelOptions::default());
    });
    let start = tokio::time::Instant::now();
    let code = "while True:\n    await bump()".to_owned();
    let (_, end) = run_code(code, functions(&["bump"]), call, LIMITS, None, &signal).await;
    assert!(matches!(end, RunEnd::Cancelled), "{end:?}");
    assert!(start.elapsed() < Duration::from_millis(2000));
    canceller.await.expect("the canceller ran");
    let made = calls.load(Ordering::SeqCst);
    assert!(made > 1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(calls.load(Ordering::SeqCst), made);
}

const GATED_CODE: &str =
    "try:\n    print(await shell_like(command='rm -rf /'))\nexcept RuntimeError as e:\n    print('ERR', e)";

fn shell_like(calls: &Arc<Mutex<Vec<String>>>) -> FunctionTool {
    let calls = Arc::clone(calls);
    FunctionTool::new(
        "shell_like",
        "Records that it ran.",
        move |input: JsonValue, _context| {
            let calls = Arc::clone(&calls);
            async move {
                let command = input["command"].as_str().unwrap_or_default().to_owned();
                calls.lock().expect("no poisoned lock").push(command.clone());
                Ok(format!("ran:{command}"))
            }
        },
    )
}

/// Seam-level port: the inner caller reports the pending approval that `ToolAgent::call_tool`
/// will report for a `confirm` intervention.
#[tokio::test]
async fn confirm_refuses_the_inner_call_no_prompt_possible_even_with_on_error_proceed() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let asks: InnerCaller = Arc::new(|_parent, _name, _input| {
        async { Ok(InnerCall::NeedsApproval("Run shell?".to_owned())) }.boxed()
    });
    let gated =
        make_programmatic_tool_caller_with(MakeProgrammaticToolCallerOptions::default(), asks, LIMITS);
    let mut agent = agent_of(vec![gated, shell_like(&calls)]);
    let result = run(&mut agent, GATED_CODE).await;
    assert_eq!(
        text(&result),
        "ERR Failed to execute tool 'shell_like': Tool 'shell_like' needs approval (Run shell?), which \
         programmatic_tool_caller cannot prompt for; call it as a normal tool call instead."
    );
    assert!(calls.lock().expect("no poisoned lock").is_empty());
}

/// Not a TS case: the cancel outcome of an inner call is the TS cancel text.
#[tokio::test]
async fn a_cancelled_inner_call_raises_the_cancel_text() {
    let cancelled: InnerCaller =
        Arc::new(|_parent, _name, _input| async { Ok(InnerCall::Cancelled) }.boxed());
    let gated =
        make_programmatic_tool_caller_with(MakeProgrammaticToolCallerOptions::default(), cancelled, LIMITS);
    let mut agent = agent_of(vec![gated, shell_like(&Arc::default())]);
    let result = run(&mut agent, GATED_CODE).await;
    assert_eq!(
        text(&result),
        "ERR Failed to execute tool 'shell_like': programmatic_tool_caller was cancelled; no further tool calls \
         are made."
    );
}

#[tokio::test]
async fn refuses_inner_calls_when_the_agent_hooks_cannot_be_reached() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut agent = agent_of(vec![programmatic_tool_caller(), shell_like(&calls)]);
    let result = run(&mut agent, GATED_CODE).await;
    assert!(
        text(&result).contains("refusing to run it ungated"),
        "{}",
        text(&result)
    );
    assert!(calls.lock().expect("no poisoned lock").is_empty());
}

/// The description is the TS constant, byte for byte.
#[test]
fn the_description_matches_the_ts_constant() {
    let Ok(source) = std::fs::read_to_string("/tmp/ts-spec/harness-ts/src/tools/programmatic-tool-caller.ts")
    else {
        return;
    };
    let start = source
        .find("export const DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION =")
        .expect("the TS constant exists");
    let expected: String = source[start..]
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .map(|line| {
            let literal = line.trim().trim_end_matches(" +");
            &literal[1..literal.len() - 1]
        })
        .collect();
    assert_eq!(DEFAULT_PROGRAMMATIC_TOOL_CALLER_DESCRIPTION, expected);
}
