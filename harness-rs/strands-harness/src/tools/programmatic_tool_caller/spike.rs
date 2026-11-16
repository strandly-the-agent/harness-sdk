//! Spike: a Monty program calls one host function across an `.await` inside `tokio::spawn`, with
//! every interpreter step on `spawn_blocking`.

use monty::{MontyRun, RunProgress};
use monty_types::{
    CompileOptions, ExtFunctionResult, MontyException, MontyObject, NameLookupResult, PrintWriter, ResourceTracker,
};

type Step = Result<RunProgress, MontyException>;

async fn step(
    mut output: String,
    run: impl FnOnce(PrintWriter<'_>) -> Step + Send + 'static,
) -> (Step, String) {
    tokio::task::spawn_blocking(move || {
        let progress = run(PrintWriter::CollectString(&mut output, None));
        (progress, output)
    })
    .await
    .expect("the interpreter step panicked")
}

async fn double(n: i64) -> i64 {
    tokio::task::yield_now().await;
    n * 2
}

#[tokio::test]
async fn a_monty_program_calls_a_host_function_across_an_await_inside_tokio_spawn() {
    let task = tokio::spawn(async {
        let code = "x = await double(n=21)\nprint(x)\nprint(await double(n=x))";
        let run = MontyRun::new(code.to_owned(), "<spike>", vec![], CompileOptions::default()).expect("the code parses");
        let (mut progress, mut output) = step(String::new(), move |print| {
            run.start(vec![], ResourceTracker::default(), print)
        })
        .await;
        let mut pending = Vec::new();
        loop {
            let next = match progress.expect("the code runs") {
                RunProgress::Complete(_) => break,
                RunProgress::NameLookup(lookup) => {
                    let answer = if lookup.name == "double" {
                        NameLookupResult::Value(MontyObject::function("double", None))
                    } else {
                        NameLookupResult::Undefined
                    };
                    step(output, move |print| lookup.resume(answer, print)).await
                }
                RunProgress::FunctionCall(call) => {
                    let n = call.args.kwarg("n").and_then(|value| value.as_int()).expect("an int n");
                    pending.push((call.call_id, n));
                    step(output, move |print| call.resume_pending(print)).await
                }
                RunProgress::ResolveFutures(futures) => {
                    let mut results = Vec::new();
                    for (call_id, n) in pending.drain(..) {
                        results.push((call_id, ExtFunctionResult::Return(MontyObject::int(double(n).await))));
                    }
                    step(output, move |print| futures.resume(results, print)).await
                }
                RunProgress::OsCall(call) => panic!("unexpected OS call: {call:?}"),
            };
            (progress, output) = next;
        }
        output
    });
    assert_eq!(task.await.expect("the task ran"), "42\n84\n");
}
