//! Ported from strands-ts/src/tools/__tests__/zod-tool.test.ts, mapped to `#[tool]`, `#[tools]`
//! and `FunctionTool::typed`.
//!
//! Gaps: async generator callbacks (TS-only mechanism; streaming tools implement `Tool`
//! directly), the `z.void()` cases (no Rust form; a tool without parameters covers them), and
//! `instanceof` (a compile-time trait bound here). `invoke()` cases run through
//! `FunctionTool::typed`, since a `#[tool]` function is called directly.

use std::sync::Arc;

use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::interrupt::InterruptParams;
use crate::testing::MockMessageModel;
use crate::tool_list;
use crate::tools::tool::ToolStreamItem;
use crate::tools::{FunctionTool, Tool, ToolContext, ToolError, ToolOutcome, ToolSource};
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::json::JsonValue;
use crate::types::tools::ToolUse;
use crate::{tool, tools};

fn context(input: JsonValue) -> ToolContext {
    let agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    ToolContext::new(
        ToolUse::new("testTool", "test-123", input),
        agent.tool_agent(),
        InvocationState::new(),
    )
}

async fn items(tool: &dyn Tool, input: JsonValue) -> Vec<ToolStreamItem> {
    tool.stream(context(input)).collect().await
}

async fn result(tool: &dyn Tool, input: JsonValue) -> JsonValue {
    let items = items(tool, input).await;
    assert_eq!(items.len(), 1, "no stream events, one outcome");
    match items.into_iter().next() {
        Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) => serde_json::to_value(result).unwrap(),
        other => panic!("expected a result, got {other:?}"),
    }
}

fn schema(tool: &dyn Tool) -> JsonValue {
    tool.tool_spec().input_schema().cloned().unwrap()
}

async fn text(tool: &dyn Tool, input: JsonValue) -> JsonValue {
    let result = result(tool, input).await;
    assert_eq!(result["status"], "success", "{result}");
    result["content"][0]["text"].clone()
}

async fn error_text(tool: &dyn Tool, input: JsonValue) -> String {
    let result = result(tool, input).await;
    assert_eq!(result["status"], "error", "{result}");
    result["content"][0]["text"].as_str().unwrap().to_owned()
}

mod tool_creation_and_properties {
    use super::*;

    /// Test description
    #[tool(name = "testTool")]
    fn test_tool(value: String) -> String {
        value
    }

    #[test]
    fn creates_tool_with_correct_properties() {
        assert_eq!(TestTool.name(), "testTool");
        assert_eq!(TestTool.description(), "Test description");
        assert_eq!(
            serde_json::to_value(TestTool.tool_spec()).unwrap(),
            json!({
                "name": "testTool",
                "description": "Test description",
                "inputSchema": {
                    "type": "object",
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"],
                    "additionalProperties": false,
                },
            })
        );
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    struct Value {
        value: String,
    }

    #[test]
    fn handles_optional_description() {
        let tool = FunctionTool::typed("testTool", "", |input: Value, _| async move {
            Ok::<_, ToolError>(input.value)
        });
        assert_eq!(tool.name(), "testTool");
        assert_eq!(tool.description(), "");
    }
}

mod invoke_method {
    use super::*;

    #[derive(Deserialize, schemars::JsonSchema)]
    struct Sum {
        a: i64,
        b: i64,
    }

    /// Synchronous tool
    #[tool]
    fn sync(a: i64, b: i64) -> i64 {
        a + b
    }

    #[tokio::test]
    async fn handles_synchronous_callback() {
        let typed = FunctionTool::typed("sync", "Synchronous tool", |input: Sum, _| async move {
            Ok::<_, ToolError>(input.a + input.b)
        });
        assert_eq!(
            typed
                .invoke(json!({"a": 5, "b": 3}), context(json!({})))
                .await
                .unwrap(),
            8
        );
        assert_eq!(text(&Sync, json!({"a": 5, "b": 3})).await, "8");
    }

    /// Async tool
    #[tool(name = "async")]
    async fn async_tool(value: String) -> String {
        format!("Result: {value}")
    }

    #[tokio::test]
    async fn handles_promise_callback() {
        assert_eq!(AsyncTool.name(), "async");
        assert_eq!(text(&AsyncTool, json!({"value": "test"})).await, "Result: test");
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    struct Age {
        #[schemars(range(min = 0, max = 120))]
        age: i64,
    }

    #[tokio::test]
    async fn throws_on_invalid_input() {
        let tool = FunctionTool::typed("validator", "Validates input", |input: Age, _| async move {
            Ok::<_, ToolError>(input.age)
        });
        for age in [-1, 150] {
            let error = tool
                .invoke(json!({"age": age}), context(json!({})))
                .await
                .unwrap_err();
            assert!(
                error.to_string().starts_with("Invalid input at /age: "),
                "{error}"
            );
        }
    }

    const EMAIL_PATTERN: &str = r"^(?:[A-Za-z0-9_'+\-]+\.)*[A-Za-z0-9_'+\-]*[A-Za-z0-9_+-]@(?:[A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$";

    /// Required fields
    #[tool]
    fn required(name: String, #[schemars(email, regex(pattern = EMAIL_PATTERN))] email: String) -> String {
        format!("{name}: {email}")
    }

    #[tokio::test]
    async fn validates_required_fields() {
        let missing_email = error_text(&Required, json!({"name": "John"})).await;
        assert!(
            missing_email.contains("\"email\" is a required property"),
            "{missing_email}"
        );
        let missing_name = error_text(&Required, json!({"email": "invalid-email"})).await;
        assert!(missing_name.starts_with("Error: Invalid input"), "{missing_name}");
        assert_eq!(
            text(&Required, json!({"name": "John", "email": "john@example.com"})).await,
            "John: john@example.com"
        );
    }

    /// Uses context
    #[tool]
    async fn uses_context(#[tool(context)] ctx: &ToolContext, value: String) -> String {
        format!(
            "{}:{}:{value}",
            ctx.tool_use().name(),
            ctx.tool_use().tool_use_id()
        )
    }

    #[tokio::test]
    async fn passes_context_to_callback() {
        assert_eq!(
            text(&UsesContext, json!({"value": "test"})).await,
            "testTool:test-123:test"
        );
    }
}

mod stream_method {
    use super::*;

    /// Synchronous tool
    #[tool]
    pub(super) fn echo(value: String) -> String {
        value
    }

    #[tokio::test]
    async fn streams_synchronous_callback_result() {
        assert_eq!(text(&Echo, json!({"value": "hello"})).await, "hello");
    }

    /// Async tool
    #[tool]
    pub(super) async fn double(value: i64) -> i64 {
        value * 2
    }

    #[tokio::test]
    async fn streams_promise_callback_result() {
        assert_eq!(text(&Double, json!({"value": 21})).await, "42");
    }

    /// Validates input
    #[tool]
    fn validator(#[schemars(range(min = 0))] age: i64) -> i64 {
        age
    }

    #[tokio::test]
    async fn returns_error_result_on_validation_failure() {
        let error = error_text(&Validator, json!({"age": -5})).await;
        assert!(error.contains("age"), "{error}");
    }

    /// Required fields
    #[tool]
    fn required(name: String, value: i64) -> String {
        format!("{name}: {value}")
    }

    #[tokio::test]
    async fn returns_error_result_on_missing_required_fields() {
        let error = error_text(&Required, json!({"name": "test"})).await;
        assert_eq!(error, "Error: Invalid input: \"value\" is a required property");
    }

    /// Throws error
    #[tool]
    fn throws(value: String) -> Result<String, ToolError> {
        let _ = value;
        Err(ToolError::msg("Callback error"))
    }

    #[tokio::test]
    async fn catches_callback_errors_and_returns_error_result() {
        assert_eq!(
            error_text(&Throws, json!({"value": "test"})).await,
            "Error: Callback error"
        );
    }

    /// Throws async error
    #[tool]
    async fn throws_async(value: String) -> Result<String, std::io::Error> {
        let _ = value;
        Err(std::io::Error::other("Async error"))
    }

    #[tokio::test]
    async fn catches_async_callback_errors() {
        assert_eq!(
            error_text(&ThrowsAsync, json!({"value": "test"})).await,
            "Error: Async error"
        );
    }
}

mod complex_scenarios {
    use super::*;

    #[derive(Deserialize, schemars::JsonSchema)]
    struct User {
        name: String,
        age: u32,
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    struct Metadata {
        timestamp: u64,
    }

    /// Nested objects
    #[tool]
    fn nested(user: User, metadata: Metadata) -> String {
        let _ = metadata.timestamp;
        format!("{} ({})", user.name, user.age)
    }

    #[tokio::test]
    async fn handles_nested_object_schemas() {
        let input =
            json!({"user": {"name": "Alice", "age": 30}, "metadata": {"timestamp": 1_700_000_000_000_u64}});
        assert_eq!(text(&Nested, input).await, "Alice (30)");
        assert_eq!(
            schema(&Nested)["properties"]["user"],
            json!({
                "type": "object",
                "properties": {"name": {"type": "string"}, "age": {"type": "integer", "minimum": 0}},
                "required": ["name", "age"],
                "additionalProperties": false,
            })
        );
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "lowercase")]
    enum Operation {
        Add,
        Subtract,
        Multiply,
        Divide,
    }

    /// Basic calculator
    #[tool]
    fn calculator(operation: Operation, a: i64, b: i64) -> i64 {
        match operation {
            Operation::Add => a + b,
            Operation::Subtract => a - b,
            Operation::Multiply => a * b,
            Operation::Divide => a / b,
        }
    }

    #[tokio::test]
    async fn handles_enum_schemas() {
        assert_eq!(
            text(&Calculator, json!({"operation": "add", "a": 5, "b": 3})).await,
            "8"
        );
        assert_eq!(
            text(&Calculator, json!({"operation": "multiply", "a": 4, "b": 7})).await,
            "28"
        );
        assert_eq!(
            schema(&Calculator)["properties"]["operation"],
            json!({"type": "string", "enum": ["add", "subtract", "multiply", "divide"]})
        );
    }

    /// Generates greeting
    #[tool]
    fn greeting(name: String, title: Option<String>) -> String {
        title.map_or_else(|| name.clone(), |title| format!("{title} {name}"))
    }

    #[tokio::test]
    async fn handles_optional_fields() {
        assert_eq!(text(&Greeting, json!({"name": "Smith"})).await, "Smith");
        assert_eq!(
            text(&Greeting, json!({"name": "Smith", "title": "Dr."})).await,
            "Dr. Smith"
        );
        assert_eq!(
            schema(&Greeting)["properties"]["title"],
            json!({"type": "string"})
        );
    }

    /// Sums numbers
    #[tool]
    fn sum(numbers: Vec<i64>) -> i64 {
        numbers.iter().sum()
    }

    #[tokio::test]
    async fn handles_array_schemas() {
        assert_eq!(text(&Sum, json!({"numbers": [1, 2, 3, 4, 5]})).await, "15");
    }
}

mod json_schema_generation {
    use super::*;

    const EMAIL_PATTERN: &str = r"^(?:[A-Za-z0-9_'+\-]+\.)*[A-Za-z0-9_'+\-]*[A-Za-z0-9_+-]@(?:[A-Za-z0-9][A-Za-z0-9\-]*\.)+[A-Za-z]{2,}$";

    /// Test tool
    #[tool]
    fn test(
        name: String,
        age: f64,
        #[schemars(email, regex(pattern = EMAIL_PATTERN))] email: String,
    ) -> &'static str {
        let _ = (name, age, email);
        "result"
    }

    #[test]
    fn generates_valid_json_schema_from_zod_schema() {
        assert_eq!(
            schema(&Test),
            json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "age": {"type": "number"},
                    "email": {
                        "format": "email",
                        "pattern": EMAIL_PATTERN,
                        "type": "string",
                    },
                    "name": {"type": "string"},
                },
                "required": ["name", "age", "email"],
            })
        );
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    #[serde(rename_all = "lowercase")]
    enum Kind {
        Guide,
        Api,
    }

    fn guide() -> Kind {
        Kind::Guide
    }

    fn guide_value() -> &'static str {
        "guide"
    }

    /// Search the docs.
    #[tool]
    fn search_docs(
        /// Search terms
        query: String,
        /// Maximum results
        #[schemars(range(min = 1, max = 50))]
        limit: Option<u32>,
        #[serde(default = "guide")]
        #[schemars(default = "guide_value")]
        kind: Kind,
        filters: Option<Vec<String>>,
    ) -> String {
        let _ = (limit, kind, filters);
        query
    }

    #[test]
    fn matches_the_zod_fixture_up_to_the_logged_differences() {
        let fixture: JsonValue = serde_json::from_str(include_str!(
            "../../../../conformance/fixtures/tools/zod_tool_spec.json"
        ))
        .unwrap();
        let mut expected = fixture;
        expected["inputSchema"]["required"] = json!(["query"]);
        assert_eq!(serde_json::to_value(SearchDocs.tool_spec()).unwrap(), expected);
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    #[expect(dead_code, reason = "schema-only test type")]
    struct SearchInput {
        /// Search terms
        query: String,
        /// Maximum results
        #[schemars(range(min = 1, max = 50))]
        limit: Option<u32>,
        #[serde(default = "guide")]
        #[schemars(default = "guide_value")]
        kind: Kind,
        filters: Option<Vec<String>>,
    }

    #[test]
    fn the_macro_and_function_tool_typed_produce_the_same_spec() {
        let typed = FunctionTool::typed("search_docs", "Search the docs.", |_: SearchInput, _| async {
            Ok::<_, ToolError>("ok")
        });
        assert_eq!(SearchDocs.tool_spec(), typed.tool_spec());
    }
}

mod instanceof_checks {
    use super::*;

    /// Test description
    #[tool]
    fn test_tool(value: String) -> String {
        value
    }

    #[test]
    fn passes_instanceof_tool_check_and_has_invokable_tool_methods() {
        let tool: Arc<dyn Tool> = Arc::new(TestTool);
        assert_eq!(tool.name(), "test_tool");
        assert!(matches!(ToolSource::from(TestTool), ToolSource::Tool(_)));
    }
}

mod optional_input_schema {
    use super::*;

    /// Tool with no input
    #[tool(name = "noInputTool")]
    fn no_input_tool() -> &'static str {
        "result"
    }

    #[test]
    fn creates_tool_with_default_empty_object_schema() {
        assert_eq!(NoInputTool.name(), "noInputTool");
        assert_eq!(NoInputTool.description(), "Tool with no input");
        assert_eq!(
            serde_json::to_value(NoInputTool.tool_spec()).unwrap(),
            json!({
                "name": "noInputTool",
                "description": "Tool with no input",
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            })
        );
    }

    #[derive(Serialize)]
    struct Preferences {
        theme: &'static str,
        language: &'static str,
    }

    /// Gets user preferences
    #[tool]
    fn get_preferences() -> Preferences {
        Preferences {
            theme: "dark",
            language: "en",
        }
    }

    #[tokio::test]
    async fn invoke_works_with_empty_object() {
        let result = result(&GetPreferences, json!({})).await;
        assert_eq!(
            result["content"],
            json!([{"json": {"theme": "dark", "language": "en"}}])
        );
    }

    /// Gets system status
    #[tool]
    async fn get_status() -> JsonValue {
        json!({"status": "operational", "uptime": 99.9})
    }

    #[tokio::test]
    async fn stream_works_with_empty_input() {
        assert_eq!(
            result(&GetStatus, json!({})).await,
            json!({
                "toolUseId": "test-123",
                "status": "success",
                "content": [{"json": {"status": "operational", "uptime": 99.9}}],
            })
        );
    }

    /// Captures input
    #[tool]
    fn capture_input(#[tool(context)] ctx: &ToolContext) -> JsonValue {
        ctx.tool_use().input().clone()
    }

    #[tokio::test]
    async fn callback_receives_empty_object_when_no_schema() {
        assert_eq!(
            result(&CaptureInput, json!({})).await["content"],
            json!([{"json": {}}])
        );
        assert_eq!(
            result(&CaptureInput, json!({"extra": 1})).await["content"],
            json!([{"json": {"extra": 1}}]),
            "a tool without parameters doesn't validate its input"
        );
    }

    /// Async tool
    #[tool]
    async fn async_no_input() -> Result<String, ToolError> {
        Ok("async result".to_owned())
    }

    #[tokio::test]
    async fn works_with_async_callback() {
        assert_eq!(text(&AsyncNoInput, json!({})).await, "async result");
    }
}

mod macro_contract {
    use super::*;

    /// Asks for confirmation.
    #[tool]
    async fn confirm(#[tool(context)] ctx: ToolContext) -> Result<JsonValue, ToolError> {
        Ok(ctx.interrupt(InterruptParams::new("confirm"))?)
    }

    #[tokio::test]
    async fn an_interrupt_raised_through_the_context_ends_with_an_interrupt_outcome() {
        let items = items(&Confirm, json!({})).await;
        let [ToolStreamItem::Outcome(ToolOutcome::Interrupt(interrupt))] = items.as_slice() else {
            panic!("expected an interrupt, got {items:?}");
        };
        assert_eq!(interrupt.id(), "tool:test-123:confirm");
    }

    /// Asks for confirmation on a blocking thread.
    #[tool]
    fn confirm_sync(#[tool(context)] ctx: &ToolContext) -> Result<JsonValue, ToolError> {
        Ok(ctx.interrupt(InterruptParams::new("confirm"))?)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_sync_tool_raises_interrupts_too() {
        let items = items(&ConfirmSync, json!({})).await;
        assert!(matches!(
            items.as_slice(),
            [ToolStreamItem::Outcome(ToolOutcome::Interrupt(_))]
        ));
    }

    /// Panics.
    #[tool]
    fn panics() -> String {
        panic!("tool panicked")
    }

    #[tokio::test]
    async fn a_panic_in_a_sync_tool_resumes_on_the_awaiting_task() {
        let task = tokio::spawn(async { items(&Panics, json!({})).await });
        let error = task.await.unwrap_err();
        assert!(error.is_panic());
    }

    #[tokio::test]
    async fn a_type_mismatch_names_the_path() {
        let error = error_text(&super::stream_method::Double, json!({"value": "x"})).await;
        assert_eq!(
            error,
            "Error: Invalid input at /value: \"x\" is not of type \"integer\""
        );
    }

    #[test]
    fn doc_comments_keep_their_lines() {
        /// First line.
        ///
        /// Second paragraph.
        #[tool]
        fn documented(
            /// The value,
            /// on two lines.
            value: String,
        ) -> String {
            value
        }

        assert_eq!(Documented.description(), "First line.\n\nSecond paragraph.");
        assert_eq!(
            schema(&Documented)["properties"]["value"]["description"],
            "The value,\non two lines."
        );
    }

    #[test]
    fn the_description_stops_at_the_first_rustdoc_section() {
        /// Reads a file.
        ///
        /// # Errors
        ///
        /// When the file is missing.
        #[tool]
        fn read_file(path: String) -> Result<String, ToolError> {
            Ok(path)
        }

        assert_eq!(ReadFile.description(), "Reads a file.");
    }

    struct Inventory {
        stock: u32,
    }

    #[tools]
    impl Inventory {
        /// Look up stock for a product.
        #[tool]
        async fn check_stock(
            &self,
            #[tool(context)] ctx: &ToolContext,
            sku: String,
        ) -> Result<u32, ToolError> {
            let _ = (ctx.tool_use(), sku);
            Ok(self.stock)
        }

        /// Restock a product.
        #[tool(name = "restock")]
        fn add_stock(
            &self,
            /// Units to add.
            units: u32,
        ) -> u32 {
            self.stock + units
        }

        fn helper(&self) -> u32 {
            self.stock
        }
    }

    #[tokio::test]
    async fn tools_bind_methods_to_the_shared_value() {
        let inventory = Arc::new(Inventory { stock: 3 });
        assert_eq!(inventory.helper(), 3);
        let check = inventory.check_stock_tool();
        assert_eq!(check.name(), "check_stock");
        assert_eq!(check.description(), "Look up stock for a product.");
        assert_eq!(text(&check, json!({"sku": "A1"})).await, "3");
        let restock = inventory.add_stock_tool();
        assert_eq!(restock.name(), "restock");
        assert_eq!(text(&restock, json!({"units": 2})).await, "5");
        assert_eq!(
            schema(&restock)["properties"]["units"]["description"],
            "Units to add."
        );
        let names: Vec<String> = ToolSource::List(inventory.tools())
            .into_tools()
            .unwrap()
            .iter()
            .map(|tool| tool.name().to_owned())
            .collect();
        assert_eq!(names, ["check_stock", "restock"]);
        drop(inventory);
        assert_eq!(
            text(&check, json!({"sku": "A1"})).await,
            "3",
            "the tool keeps the value alive"
        );
    }

    #[test]
    fn tool_list_mixes_macro_and_function_tools() {
        let ping = FunctionTool::new("ping", "Ping.", |_, _| async { Ok::<_, ToolError>("pong") });
        let inventory = Arc::new(Inventory { stock: 1 });
        let names: Vec<String> = ToolSource::List(tool_list![
            super::stream_method::Echo,
            ping,
            inventory.check_stock_tool(),
            inventory.tools()
        ])
        .into_tools()
        .unwrap()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
        assert_eq!(names, ["echo", "ping", "check_stock", "check_stock", "restock"]);
    }
}
