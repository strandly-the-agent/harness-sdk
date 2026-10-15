//! Rust runtime for the Strands CLI.
//!
//! The CLI spawns the `strands-cli-worker` binary and talks the worker protocol documented in
//! `docs/CLI_WORKER_PROTOCOL.md`: JSON-lines commands on stdin, JSON-lines messages on fd 3.
//! [`run`] is transport-agnostic so tests can drive it over in-memory pipes with a mock model.

pub mod protocol;
pub mod runtime;

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value as JsonValue};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub use protocol::{Command, InitOptions, WorkerMessage};
pub use runtime::{AgentFactory, BuildOptions, BuiltAgent, Runtime};

/// Serve one CLI connection until `close` or EOF. Returns `Err` only when `init` fails.
pub async fn run<R, W>(input: R, output: W, factory: AgentFactory, workspace: PathBuf) -> Result<(), String>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (outbox, rx) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write_messages(rx, output));
    let outcome = serve(input, outbox, factory, workspace).await;
    // Dropping every sender ends the writer; wait so the last line reaches the pipe.
    drop(writer.await);
    outcome
}

async fn serve<R: tokio::io::AsyncBufRead + Unpin>(
    input: R,
    outbox: mpsc::UnboundedSender<WorkerMessage>,
    factory: AgentFactory,
    workspace: PathBuf,
) -> Result<(), String> {
    let mut lines = input.lines();
    let first = match lines.next_line().await {
        Ok(Some(line)) => line,
        _ => return Err("expected an init command".to_owned()),
    };
    let init = match serde_json::from_str::<Command>(&first) {
        Ok(Command::Init(options)) => options,
        Ok(_) => return fail(&outbox, "The first command must be init"),
        Err(error) => return fail(&outbox, &format!("Invalid init command: {error}")),
    };
    let interactive = init.interactive;
    let runtime = match Runtime::open(
        factory,
        outbox.clone(),
        build_options(init),
        interactive,
        workspace,
    )
    .await
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&outbox, &error.to_string()),
    };
    send(
        &outbox,
        WorkerMessage::Ready {
            state: runtime.state(),
        },
    );

    let mut running: Option<JoinHandle<()>> = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let command = match serde_json::from_str::<Command>(&line) {
            Ok(command) => command,
            Err(error) => {
                send(
                    &outbox,
                    WorkerMessage::Error {
                        message: format!("Invalid command: {error}"),
                    },
                );
                continue;
            }
        };
        if let Some(handle) = &running {
            if handle.is_finished() {
                running = None;
            }
        }
        match command {
            Command::Permission { request_id, approved } => runtime.permission(&request_id, approved),
            Command::Cancel => runtime.cancel(),
            Command::Close => {
                runtime.cancel();
                break;
            }
            _ if running.is_some() => send(
                &outbox,
                WorkerMessage::Error {
                    message: "The Rust agent is busy".to_owned(),
                },
            ),
            Command::Prompt { prompt } => {
                let token = runtime.begin_turn();
                let runtime = runtime.clone();
                let outbox = outbox.clone();
                running = Some(tokio::spawn(async move {
                    let reply = runtime.turn(prompt, token).await;
                    send(&outbox, reply);
                }));
            }
            other => send(&outbox, dispatch(&runtime, other).await),
        }
    }
    if let Some(handle) = running {
        // `close` already cancelled the turn; give it a moment to report, then stop waiting so the
        // CLI's own 1 s close timeout is never the thing that ends us.
        if tokio::time::timeout(std::time::Duration::from_millis(500), handle)
            .await
            .is_err()
        {
            tracing::debug!("turn did not finish after close; exiting anyway");
        }
    }
    Ok(())
}

async fn dispatch(runtime: &Runtime, command: Command) -> WorkerMessage {
    let outcome = match command {
        Command::Reset {
            model,
            thinking,
            session_id,
            clear,
        } => runtime
            .reset(model, thinking, session_id, clear)
            .await
            .map(|()| result("reset")),
        Command::Compact => runtime.compact().map(|context| match context {
            Some(context) => protocol::RunResult {
                context: Some(context),
                ..result("compacted")
            },
            None => result("unchanged"),
        }),
        Command::Skill { name } => runtime.activate_skill(&name).await.map(|()| result("activated")),
        Command::Snapshot => runtime.snapshot_json().map(|text| with_text("snapshot", text)),
        Command::Stash => Ok(with_text("stash", "{}".to_owned())),
        Command::Seed { snapshot, .. } => runtime.seed(&snapshot).map(|()| result("seeded")),
        Command::Conversation { snapshot: None, .. } => runtime
            .conversation_json()
            .map(|text| with_text("snapshot", text)),
        Command::Conversation {
            snapshot: Some(snapshot),
            preserve_reasoning,
        } => runtime
            .restore_conversation(snapshot, preserve_reasoning)
            .map(|()| result("restored")),
        Command::ReloadFailed { message } => {
            runtime.record_reload_failure(&message);
            Ok(result("recorded"))
        }
        Command::Init(_) => Err(strands_agents::error::Error::other(
            "The agent is already initialized",
        )),
        Command::Prompt { .. } | Command::Permission { .. } | Command::Cancel | Command::Close => {
            unreachable!("handled by the command loop")
        }
    };
    match outcome {
        Ok(run_result) => WorkerMessage::Result {
            result: run_result,
            state: runtime.state(),
            reload: false,
        },
        Err(error) => WorkerMessage::Error {
            message: error.to_string(),
        },
    }
}

fn result(stop_reason: &str) -> protocol::RunResult {
    protocol::RunResult::stop(stop_reason)
}

fn with_text(stop_reason: &str, text: String) -> protocol::RunResult {
    protocol::RunResult {
        final_text: Some(text),
        ..result(stop_reason)
    }
}

fn fail(outbox: &mpsc::UnboundedSender<WorkerMessage>, message: &str) -> Result<(), String> {
    send(
        outbox,
        WorkerMessage::Error {
            message: message.to_owned(),
        },
    );
    Err(message.to_owned())
}

fn send(outbox: &mpsc::UnboundedSender<WorkerMessage>, message: WorkerMessage) {
    drop(outbox.send(message));
}

async fn write_messages<W: AsyncWrite + Unpin>(
    mut rx: mpsc::UnboundedReceiver<WorkerMessage>,
    mut output: W,
) {
    while let Some(message) = rx.recv().await {
        let Ok(mut line) = serde_json::to_vec(&message) else {
            continue;
        };
        line.push(b'\n');
        if output.write_all(&line).await.is_err() || output.flush().await.is_err() {
            return;
        }
    }
}

/// Turn `init` options into [`BuildOptions`]: apply `assignments`, lift `model`/`effort`.
pub fn build_options(init: InitOptions) -> BuildOptions {
    let mut overrides = init.overrides;
    for assignment in &init.assignments {
        if let Some((path, raw)) = assignment.split_once('=') {
            let value = serde_json::from_str(raw).unwrap_or_else(|_| JsonValue::String(raw.to_owned()));
            assign(
                &mut overrides,
                path.split('.').collect::<Vec<_>>().as_slice(),
                value,
            );
        }
    }
    let model = overrides
        .get("model")
        .and_then(JsonValue::as_str)
        .map(str::to_owned);
    let effort = overrides
        .get("effort")
        .cloned()
        .unwrap_or(JsonValue::String("auto".to_owned()));
    BuildOptions {
        model,
        effort,
        session_id: init.session_id,
        session_dir: init.session_dir,
        mcp_servers: init.mcp_servers,
        skill_paths: init.skill_paths,
        overrides,
    }
}

fn assign(target: &mut Map<String, JsonValue>, path: &[&str], value: JsonValue) {
    match path {
        [] => {}
        [key] => {
            target.insert((*key).to_owned(), value);
        }
        [key, rest @ ..] => {
            let entry = target
                .entry((*key).to_owned())
                .or_insert_with(|| JsonValue::Object(Map::new()));
            if !entry.is_object() {
                *entry = JsonValue::Object(Map::new());
            }
            if let JsonValue::Object(child) = entry {
                assign(child, rest, value);
            }
        }
    }
}

/// Convenience for callers that already hold a boxed reader.
pub type BoxedInput = Box<dyn AsyncBufRead + Unpin + Send>;

/// Share a factory closure.
pub fn factory<F, Fut>(f: F) -> AgentFactory
where
    F: Fn(BuildOptions) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = strands_agents::error::Result<BuiltAgent>> + Send + 'static,
{
    Arc::new(move |options| Box::pin(f(options)))
}
