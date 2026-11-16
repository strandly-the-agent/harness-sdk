//! Ports `memory/__tests__/memory-manager.test.ts`.

use std::future::Future;
use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::Agent;
use crate::injection::InjectionConfig;
use crate::memory::test_support::{
    MockStore, assistant, init_agent, mock_agent, tool_result, tool_use, user,
};
use crate::memory::{MemoryMetadata, MemorySetting};
use crate::middleware::InvokeModelStage;
use crate::test_support::WarnRecorder;
use crate::testing::MockMessageModel;
use crate::tools::FunctionTool;
use crate::types::messages::{TextBlock, ToolResultBlock, ToolResultContent};
use crate::types::tools::ToolResultStatus;
use crate::vended_tools::test_support::{context_for, stream_result};

fn store(name: &str) -> Arc<MockStore> {
    Arc::new(MockStore::new(name))
}

fn writable(name: &str) -> Arc<MockStore> {
    Arc::new(MockStore::writable(name))
}

fn with_entries(name: &str, contents: &[&str]) -> Arc<MockStore> {
    Arc::new(MockStore::new(name).with_entries(contents))
}

fn failing(name: &str) -> Arc<MockStore> {
    let store = MockStore::writable(name);
    store.set_failing(true);
    Arc::new(store)
}

fn config(stores: Vec<Arc<MockStore>>) -> MemoryManagerConfig {
    MemoryManagerConfig {
        stores: stores
            .into_iter()
            .map(|store| store as Arc<dyn MemoryStore>)
            .collect(),
        ..Default::default()
    }
}

fn manager(stores: Vec<Arc<MockStore>>) -> MemoryManager {
    MemoryManager::new(config(stores)).unwrap()
}

fn with_add_tool(stores: Vec<Arc<MockStore>>) -> MemoryManager {
    MemoryManager::new(MemoryManagerConfig {
        add_tool_config: Some(MemorySetting::On),
        ..config(stores)
    })
    .unwrap()
}

fn scoped_add_tool(stores: Vec<Arc<MockStore>>, names: &[&str]) -> Result<MemoryManager> {
    MemoryManager::new(MemoryManagerConfig {
        add_tool_config: Some(
            MemoryAddToolConfig {
                stores: Some(names.iter().map(|name| (*name).to_owned()).collect()),
                ..Default::default()
            }
            .into(),
        ),
        ..config(stores)
    })
}

fn error_of(result: Result<MemoryManager>) -> String {
    result.unwrap_err().to_string()
}

fn tool_names(manager: &MemoryManager) -> Vec<String> {
    manager
        .get_tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect()
}

fn named_tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(FunctionTool::new(
        name,
        format!("test tool {name}"),
        |_input: JsonValue, _context| async move { Ok::<_, ToolError>("ok") },
    ))
}

fn with_tools(mut store: MockStore, names: &[&str]) -> Arc<MockStore> {
    store.tools = names.iter().map(|name| named_tool(name)).collect();
    Arc::new(store)
}

fn tool(manager: &MemoryManager, name: &str) -> Arc<dyn Tool> {
    manager
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == name)
        .unwrap()
}

async fn invoke(manager: &MemoryManager, name: &str, input: JsonValue) -> ToolResultBlock {
    let agent = mock_agent();
    stream_result(&*tool(manager, name), context_for(&agent, name, input)).await
}

async fn search_tool(manager: &MemoryManager, input: JsonValue) -> ToolResultBlock {
    invoke(manager, "search_memory", input).await
}

async fn add_tool(manager: &MemoryManager, input: JsonValue) -> ToolResultBlock {
    invoke(manager, "add_memory", input).await
}

/// The JSON a successful tool call returned.
fn returned(result: &ToolResultBlock) -> JsonValue {
    assert_eq!(result.status(), &ToolResultStatus::Success, "{result:?}");
    match &result.content()[0] {
        ToolResultContent::Json(block) => block.json().clone(),
        other => panic!("expected json, got {other:?}"),
    }
}

/// The message of a failed tool call.
fn thrown(result: &ToolResultBlock) -> String {
    assert_eq!(result.status(), &ToolResultStatus::Error, "{result:?}");
    result.error().unwrap().to_string()
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn entries_json(entries: &[MemoryEntry]) -> JsonValue {
    serde_json::to_value(entries).unwrap()
}

fn limit(max_search_results: usize) -> SearchOptions {
    SearchOptions {
        max_search_results: Some(max_search_results),
    }
}

fn add_calls(store: &MockStore) -> Vec<(String, Option<MemoryMetadata>)> {
    store.add_calls.lock().unwrap().clone()
}

fn search_calls(store: &MockStore) -> Vec<(String, SearchOptions)> {
    store.search_calls.lock().unwrap().clone()
}

fn only(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| (*name).to_owned()).collect())
}

// constructor

#[test]
fn throws_when_stores_array_is_empty() {
    assert_eq!(
        error_of(MemoryManager::new(MemoryManagerConfig::default())),
        "MemoryManager: at least one store is required"
    );
}

#[test]
fn creates_instance_with_valid_config() {
    assert_eq!(manager(vec![store("test")]).name(), "strands:memory-manager");
}

#[test]
fn throws_when_two_stores_share_a_name() {
    assert_eq!(
        error_of(MemoryManager::new(config(vec![store("dup"), store("dup")]))),
        "MemoryManager: duplicate store name 'dup'"
    );
}

#[test]
fn throws_when_a_store_is_writable_but_has_no_add_method() {
    let broken = Arc::new(MockStore {
        writable: true,
        ..MockStore::new("broken")
    });
    assert_eq!(
        error_of(MemoryManager::new(config(vec![broken]))),
        "MemoryManager: store 'broken' is writable but has no add or addMessages method"
    );
}

#[test]
fn throws_when_add_tool_config_is_enabled_but_no_stores_are_writable() {
    let result = MemoryManager::new(MemoryManagerConfig {
        add_tool_config: Some(true.into()),
        ..config(vec![store("a")])
    });
    assert_eq!(
        error_of(result),
        "MemoryManager: addToolConfig is enabled but no writable stores implement add"
    );
}

#[test]
fn allows_add_tool_config_true_with_a_single_writable_store() {
    let manager = with_add_tool(vec![writable("a")]);
    assert!(tool_names(&manager).contains(&"add_memory".to_owned()));
}

#[test]
fn allows_add_tool_config_true_with_multiple_writable_stores() {
    let manager = with_add_tool(vec![writable("a"), writable("b")]);
    assert!(tool_names(&manager).contains(&"add_memory".to_owned()));
}

#[test]
fn throws_when_add_tool_config_stores_names_a_non_existent_store() {
    assert_eq!(
        error_of(scoped_add_tool(vec![writable("a")], &["nonexistent"])),
        "MemoryManager: addToolConfig store 'nonexistent' not found"
    );
}

#[test]
fn throws_when_add_tool_config_stores_names_a_non_writable_store() {
    assert_eq!(
        error_of(scoped_add_tool(
            vec![writable("a"), store("readonly")],
            &["readonly"]
        )),
        "MemoryManager: addToolConfig store 'readonly' is not writable"
    );
}

/// Rust names the store: an instance in TS `addToolConfig.stores` resolves to its name.
#[tokio::test]
async fn accepts_memory_store_instances_not_just_names_in_add_tool_config_stores() {
    let personal = writable("personal");
    let team = writable("team");
    let manager = scoped_add_tool(vec![personal.clone(), team.clone()], &[personal.name()]).unwrap();
    add_tool(&manager, json!({ "entries": ["fact"] })).await;
    assert_eq!(add_calls(&personal), [("fact".to_owned(), None)]);
    assert!(add_calls(&team).is_empty());
}

#[test]
fn throws_when_an_add_tool_config_stores_instance_is_not_a_configured_store() {
    let stray = writable("stray");
    assert_eq!(
        error_of(scoped_add_tool(vec![writable("configured")], &[stray.name()])),
        "MemoryManager: addToolConfig store 'stray' not found"
    );
}

// getTools

#[test]
fn registers_search_tool_by_default() {
    let manager = manager(vec![store("test")]);
    assert_eq!(tool_names(&manager), ["search_memory"]);
    let search = tool(&manager, "search_memory");
    assert_eq!(search.description(), SEARCH_TOOL_DESCRIPTION);
    assert_eq!(
        serde_json::to_value(search.tool_spec()).unwrap(),
        json!({
            "name": "search_memory",
            "description": "Search long-term memory for facts, preferences, or context from previous conversations. Use when you need background about the user or topic that may have been discussed before.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for" },
                    "maxSearchResults": { "description": "Maximum number of results per store", "type": "number" },
                    "stores": {
                        "description": "Filter to specific stores by name. Omit to search all available stores.",
                        "type": "array",
                        "items": { "type": "string" },
                    },
                },
                "required": ["query"],
                "additionalProperties": false,
            },
        })
    );
}

#[test]
fn registers_add_tool_when_add_tool_config_is_enabled() {
    let manager = with_add_tool(vec![writable("test")]);
    assert_eq!(tool_names(&manager), ["search_memory", "add_memory"]);
    assert_eq!(
        serde_json::to_value(tool(&manager, "add_memory").tool_spec()).unwrap(),
        json!({
            "name": "add_memory",
            "description": "Add facts, preferences, or decisions to long-term memory so they are remembered across conversations. Use when the user shares something worth recalling later.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entries": {
                        "minItems": 1,
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Data to add to long-term memory",
                    },
                    "stores": {
                        "description": "Target specific stores by name. Omit to add to all writable stores.",
                        "type": "array",
                        "items": { "type": "string" },
                    },
                },
                "required": ["entries"],
                "additionalProperties": false,
            },
        })
    );
}

#[test]
fn does_not_register_add_tool_by_default() {
    assert_eq!(tool_names(&manager(vec![writable("test")])), ["search_memory"]);
}

#[test]
fn returns_empty_array_when_search_tool_config_is_false_and_add_tool_config_is_false() {
    let manager = MemoryManager::new(MemoryManagerConfig {
        search_tool_config: Some(false.into()),
        add_tool_config: Some(false.into()),
        ..config(vec![writable("test")])
    })
    .unwrap();
    assert!(manager.get_tools().is_empty());
}

#[test]
fn uses_custom_tool_names_from_memory_tool_config() {
    let manager = MemoryManager::new(MemoryManagerConfig {
        search_tool_config: Some(MemoryToolConfig::builder().name("recall").build().into()),
        add_tool_config: Some(MemoryAddToolConfig::builder().name("remember").build().into()),
        ..config(vec![writable("test")])
    })
    .unwrap();
    assert_eq!(tool_names(&manager), ["recall", "remember"]);
}

#[test]
fn includes_store_descriptions_in_search_tool_description() {
    let personal = Arc::new(MockStore::new("personal").with_description("User preferences"));
    let manager = manager(vec![personal]);
    assert_eq!(
        tool(&manager, "search_memory").description(),
        format!(
            "{SEARCH_TOOL_DESCRIPTION}\n\nAvailable memory stores:\n- personal: User preferences\n\nYou can target one or more memory stores by name if you know which domains are relevant, or omit the stores parameter to search all."
        )
    );
}

#[test]
fn includes_store_descriptions_in_add_tool_description() {
    let notes = Arc::new(MockStore::writable("notes").with_description("Personal notes"));
    let manager = with_add_tool(vec![notes]);
    assert_eq!(
        tool(&manager, "add_memory").description(),
        format!(
            "{ADD_TOOL_DESCRIPTION}\n\nAvailable writable stores:\n- notes: Personal notes\n\nYou can target a specific store by name to route facts to the right place, or omit to add to all available writable stores."
        )
    );
}

#[test]
fn aggregates_tools_provided_by_stores_via_get_tools() {
    let manager = manager(vec![with_tools(MockStore::new("kb"), &["kb_query"])]);
    assert_eq!(tool_names(&manager), ["search_memory", "kb_query"]);
}

#[test]
fn aggregates_store_tools_across_multiple_stores_alongside_the_manager_tools() {
    let a = with_tools(MockStore::writable("a"), &["a_tool"]);
    let b = with_tools(MockStore::new("b"), &["b_tool"]);
    let manager = with_add_tool(vec![a, b]);
    assert_eq!(
        tool_names(&manager),
        ["search_memory", "add_memory", "a_tool", "b_tool"]
    );
}

#[test]
fn includes_store_tools_even_when_the_manager_registers_no_tools_of_its_own() {
    let manager = MemoryManager::new(MemoryManagerConfig {
        search_tool_config: Some(false.into()),
        ..config(vec![with_tools(MockStore::new("kb"), &["kb_query"])])
    })
    .unwrap();
    assert_eq!(tool_names(&manager), ["kb_query"]);
}

// search

#[tokio::test]
async fn queries_all_stores_and_concatenates_results() {
    let manager = manager(vec![
        with_entries("a", &["fact one"]),
        with_entries("b", &["fact two"]),
    ]);
    let results = manager
        .search("query", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(
        entries_json(&results),
        json!([
            { "content": "fact one", "storeName": "a" },
            { "content": "fact two", "storeName": "b" },
        ])
    );
}

#[tokio::test]
async fn resolves_a_stores_per_instance_max_search_results_when_the_caller_omits_it() {
    let store = Arc::new(MockStore {
        max_search_results: Some(5),
        ..MockStore::new("a")
    });
    let manager = manager(vec![store.clone()]);
    manager
        .search("query", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(search_calls(&store), [("query".to_owned(), limit(5))]);
}

#[tokio::test]
async fn forwards_an_explicit_max_search_results_override_to_each_store() {
    let store = Arc::new(MockStore {
        max_search_results: Some(5),
        ..MockStore::new("a")
    });
    let manager = manager(vec![store.clone()]);
    let options = MemorySearchOptions {
        max_search_results: Some(2),
        stores: None,
    };
    manager.search("query", options).await.unwrap();
    assert_eq!(search_calls(&store), [("query".to_owned(), limit(2))]);
}

#[tokio::test]
async fn falls_back_to_the_sdk_default_when_neither_caller_nor_store_specifies_a_limit() {
    let store = store("a");
    let manager = manager(vec![store.clone()]);
    manager
        .search("query", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(search_calls(&store), [("query".to_owned(), limit(3))]);
}

#[tokio::test]
async fn filters_to_named_stores_when_options_stores_is_provided() {
    let personal = with_entries("personal", &["personal fact"]);
    let team = with_entries("team", &["team fact"]);
    let manager = manager(vec![personal, team.clone()]);
    let options = MemorySearchOptions {
        max_search_results: None,
        stores: only(&["personal"]),
    };
    let results = manager.search("query", options).await.unwrap();
    assert_eq!(
        entries_json(&results),
        json!([{ "content": "personal fact", "storeName": "personal" }])
    );
    assert_eq!(team.search_count(), 0);
}

#[tokio::test]
async fn gracefully_handles_store_failures() {
    let failing = Arc::new(MockStore {
        search_error: Some("network error".to_owned()),
        ..MockStore::new("failing")
    });
    let manager = manager(vec![failing, with_entries("ok", &["fact"])]);
    let results = manager
        .search("query", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(
        entries_json(&results),
        json!([{ "content": "fact", "storeName": "ok" }])
    );
}

#[tokio::test]
async fn searches_all_stores_when_stores_option_is_omitted() {
    let manager = manager(vec![
        with_entries("a", &["fact one"]),
        with_entries("b", &["fact two"]),
    ]);
    let results = manager
        .search("query", MemorySearchOptions::default())
        .await
        .unwrap();
    assert_eq!(
        results
            .iter()
            .map(|entry| entry.store_name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[tokio::test]
async fn searches_no_stores_when_stores_option_is_an_empty_array() {
    let a = with_entries("a", &["fact one"]);
    let b = with_entries("b", &["fact two"]);
    let manager = manager(vec![a.clone(), b.clone()]);
    let options = MemorySearchOptions {
        max_search_results: None,
        stores: Some(Vec::new()),
    };
    assert!(manager.search("query", options).await.unwrap().is_empty());
    assert_eq!(a.search_count() + b.search_count(), 0);
}

#[tokio::test]
async fn search_throws_a_not_found_error_when_a_named_store_does_not_exist() {
    let personal = with_entries("personal", &["fact"]);
    let manager = manager(vec![personal.clone()]);
    let options = MemorySearchOptions {
        max_search_results: None,
        stores: only(&["nonexistent"]),
    };
    let error = manager.search("query", options).await.unwrap_err();
    assert_eq!(error.to_string(), "MemoryManager: store 'nonexistent' not found");
    assert_eq!(personal.search_count(), 0);
}

// add

async fn add_to(manager: &MemoryManager, content: &str, stores: &[&str]) -> Result<()> {
    let options = MemoryAddOptions {
        metadata: None,
        stores: only(stores),
    };
    manager.add(content, options).await
}

#[tokio::test]
async fn writes_to_all_writable_stores() {
    let (a, b) = (writable("a"), writable("b"));
    let manager = manager(vec![a.clone(), b.clone()]);
    manager
        .add("user likes coffee", MemoryAddOptions::default())
        .await
        .unwrap();
    assert_eq!(add_calls(&a), [("user likes coffee".to_owned(), None)]);
    assert_eq!(add_calls(&b), [("user likes coffee".to_owned(), None)]);
}

#[tokio::test]
async fn passes_metadata_to_stores() {
    let a = writable("a");
    let manager = manager(vec![a.clone()]);
    let metadata: MemoryMetadata = serde_json::from_value(json!({ "source": "user" })).unwrap();
    let options = MemoryAddOptions {
        metadata: Some(metadata.clone()),
        stores: None,
    };
    manager.add("fact", options).await.unwrap();
    assert_eq!(add_calls(&a), [("fact".to_owned(), Some(metadata))]);
}

#[tokio::test]
async fn add_filters_to_named_stores_when_options_stores_is_provided() {
    let (personal, team) = (writable("personal"), writable("team"));
    let manager = manager(vec![personal.clone(), team.clone()]);
    add_to(&manager, "my preference", &["personal"]).await.unwrap();
    assert_eq!(add_calls(&personal), [("my preference".to_owned(), None)]);
    assert!(add_calls(&team).is_empty());
}

#[tokio::test]
async fn dedupes_duplicate_store_names_so_each_store_is_written_once() {
    let personal = writable("personal");
    let manager = manager(vec![personal.clone()]);
    add_to(&manager, "fact", &["personal", "personal"]).await.unwrap();
    assert_eq!(add_calls(&personal).len(), 1);
}

#[tokio::test]
async fn throws_when_no_writable_stores_match() {
    let manager = manager(vec![store("a")]);
    let error = manager
        .add("fact", MemoryAddOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MemoryManager: no writable store matched");
}

#[tokio::test]
async fn add_throws_a_not_found_error_when_a_named_store_does_not_exist() {
    let manager = manager(vec![writable("a")]);
    let error = add_to(&manager, "fact", &["nonexistent"]).await.unwrap_err();
    assert_eq!(error.to_string(), "MemoryManager: store 'nonexistent' not found");
}

#[tokio::test]
async fn throws_a_read_only_error_when_a_named_store_cannot_be_written() {
    let manager = manager(vec![store("readonly")]);
    let error = add_to(&manager, "fact", &["readonly"]).await.unwrap_err();
    assert_eq!(error.to_string(), "MemoryManager: store 'readonly' is read-only");
}

#[tokio::test]
async fn awaits_writes_and_throws_aggregate_error_naming_the_failed_store_on_partial_failure() {
    let ok = writable("ok");
    let manager = manager(vec![failing("failing"), ok.clone()]);
    let error = manager
        .add("fact", MemoryAddOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "MemoryManager: store writes failed: failing");
    assert_eq!(add_calls(&ok), [("fact".to_owned(), None)]);
}

#[tokio::test]
async fn dispatches_writes_to_all_targeted_stores() {
    let (a, b) = (writable("a"), writable("b"));
    let manager = manager(vec![a.clone(), b.clone()]);
    manager.add("fact", MemoryAddOptions::default()).await.unwrap();
    assert_eq!(add_calls(&a), [("fact".to_owned(), None)]);
    assert_eq!(add_calls(&b), [("fact".to_owned(), None)]);
}

// tool store scoping

fn personal_and_team() -> (Arc<MockStore>, Arc<MockStore>, MemoryManager) {
    let personal = with_entries("personal", &["personal fact"]);
    let team = with_entries("team", &["team fact"]);
    let manager = manager(vec![personal.clone(), team.clone()]);
    (personal, team, manager)
}

fn writable_personal_and_team() -> (Arc<MockStore>, Arc<MockStore>, MemoryManager) {
    let (personal, team) = (writable("personal"), writable("team"));
    let manager = with_add_tool(vec![personal.clone(), team.clone()]);
    (personal, team, manager)
}

const OUT_OF_SCOPE: &str = "none of the requested memory stores are available";

#[tokio::test]
async fn search_tool_queries_all_stores_when_model_omits_stores() {
    let (personal, team, manager) = personal_and_team();
    search_tool(&manager, json!({ "query": "q" })).await;
    assert_eq!((personal.search_count(), team.search_count()), (1, 1));
}

#[tokio::test]
async fn search_tool_result_attributes_each_entry_to_its_source_store() {
    let (_personal, _team, manager) = personal_and_team();
    let result = search_tool(&manager, json!({ "query": "q" })).await;
    assert_eq!(
        returned(&result),
        json!({ "$value": [
            { "content": "personal fact", "storeName": "personal" },
            { "content": "team fact", "storeName": "team" },
        ] })
    );
}

#[tokio::test]
async fn search_tool_treats_an_empty_stores_array_as_omitted_searches_all() {
    let (personal, team, manager) = personal_and_team();
    search_tool(&manager, json!({ "query": "q", "stores": [] })).await;
    assert_eq!((personal.search_count(), team.search_count()), (1, 1));
}

#[tokio::test]
async fn search_tool_targets_only_the_requested_store_when_in_scope() {
    let (personal, team, manager) = personal_and_team();
    search_tool(&manager, json!({ "query": "q", "stores": ["personal"] })).await;
    assert_eq!((personal.search_count(), team.search_count()), (1, 0));
}

#[test]
fn search_tool_keeps_valid_names_and_warns_on_out_of_scope_names() {
    let (personal, team, manager) = personal_and_team();
    let ((), warnings) = WarnRecorder::capture(|| {
        block_on(async {
            search_tool(
                &manager,
                json!({ "query": "q", "stores": ["personal", "nonexistent"] }),
            )
            .await;
        });
    });
    assert_eq!((personal.search_count(), team.search_count()), (1, 0));
    assert_eq!(
        warnings,
        ["requested=<nonexistent> | ignoring memory stores outside this tool's scope"]
    );
}

#[tokio::test]
async fn search_tool_throws_when_every_requested_store_is_out_of_scope() {
    let personal = with_entries("personal", &["personal fact"]);
    let manager = manager(vec![personal.clone()]);
    let result = search_tool(&manager, json!({ "query": "q", "stores": ["nonexistent"] })).await;
    assert_eq!(
        thrown(&result),
        "MemoryManager: requested=<nonexistent> | none of the requested memory stores are available; available stores: personal"
    );
    assert_eq!(personal.search_count(), 0);
}

#[tokio::test]
async fn add_tool_writes_to_all_writable_stores_when_model_omits_stores() {
    let (personal, team, manager) = writable_personal_and_team();
    add_tool(&manager, json!({ "entries": ["fact"] })).await;
    assert_eq!(add_calls(&personal), [("fact".to_owned(), None)]);
    assert_eq!(add_calls(&team), [("fact".to_owned(), None)]);
}

#[tokio::test]
async fn add_tool_treats_an_empty_stores_array_as_omitted_writes_to_all_writable() {
    let (personal, team, manager) = writable_personal_and_team();
    add_tool(&manager, json!({ "entries": ["fact"], "stores": [] })).await;
    assert_eq!((add_calls(&personal).len(), add_calls(&team).len()), (1, 1));
}

#[tokio::test]
async fn add_tool_is_scoped_to_add_tool_config_stores_excludes_other_writable_stores() {
    let (personal, team) = (writable("personal"), writable("team"));
    let manager = scoped_add_tool(vec![personal.clone(), team.clone()], &["personal"]).unwrap();
    add_tool(&manager, json!({ "entries": ["fact"] })).await;
    assert_eq!(add_calls(&personal), [("fact".to_owned(), None)]);
    assert!(add_calls(&team).is_empty());
}

#[tokio::test]
async fn add_tool_rejects_a_writable_store_excluded_from_add_tool_config_stores_extraction_only_store() {
    let extraction_only = writable("extraction-only");
    let manager =
        scoped_add_tool(vec![writable("personal"), extraction_only.clone()], &["personal"]).unwrap();
    let result = add_tool(
        &manager,
        json!({ "entries": ["fact"], "stores": ["extraction-only"] }),
    )
    .await;
    assert!(thrown(&result).contains(OUT_OF_SCOPE));
    assert!(add_calls(&extraction_only).is_empty());
}

#[tokio::test]
async fn add_tool_excludes_read_only_stores_from_its_scope() {
    let personal = writable("personal");
    let manager = with_add_tool(vec![personal.clone(), store("readonly")]);
    let result = add_tool(&manager, json!({ "entries": ["fact"], "stores": ["readonly"] })).await;
    assert!(thrown(&result).contains(OUT_OF_SCOPE));
    assert!(add_calls(&personal).is_empty());
}

#[test]
fn add_tool_keeps_valid_names_and_warns_on_out_of_scope_names() {
    let (personal, team, manager) = writable_personal_and_team();
    let ((), warnings) = WarnRecorder::capture(|| {
        block_on(async {
            add_tool(
                &manager,
                json!({ "entries": ["fact"], "stores": ["personal", "nonexistent"] }),
            )
            .await;
        });
    });
    assert_eq!(add_calls(&personal), [("fact".to_owned(), None)]);
    assert!(add_calls(&team).is_empty());
    assert_eq!(
        warnings,
        ["requested=<nonexistent> | ignoring memory stores outside this tool's scope"]
    );
}

#[tokio::test]
async fn add_tool_throws_when_every_requested_store_is_out_of_scope() {
    let personal = writable("personal");
    let manager = with_add_tool(vec![personal.clone()]);
    let result = add_tool(
        &manager,
        json!({ "entries": ["fact"], "stores": ["nonexistent"] }),
    )
    .await;
    assert_eq!(
        thrown(&result),
        "MemoryManager: requested=<nonexistent> | none of the requested memory stores are available; available stores: personal"
    );
    assert!(add_calls(&personal).is_empty());
}

#[tokio::test]
async fn add_tool_rejects_an_empty_entries_array() {
    let personal = writable("personal");
    let manager = with_add_tool(vec![personal.clone()]);
    let result = add_tool(&manager, json!({ "entries": [] })).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    assert!(add_calls(&personal).is_empty());
}

#[tokio::test]
async fn add_tool_returns_stored_count_by_default_awaits_writes() {
    let manager = with_add_tool(vec![writable("notes")]);
    let result = add_tool(&manager, json!({ "entries": ["a", "b"] })).await;
    assert_eq!(returned(&result), json!({ "stored": 2 }));
}

#[tokio::test]
async fn add_tool_throws_a_flattened_error_with_concrete_reasons_when_entries_fail_default_awaits() {
    let manager = with_add_tool(vec![failing("failing")]);
    let result = add_tool(&manager, json!({ "entries": ["a", "b"] })).await;
    // The reasons are the store errors, not the per-entry aggregate `add` reports.
    assert_eq!(
        thrown(&result),
        "MemoryManager: failed to add 2 of 2 entries: write error; write error"
    );
}

fn fire_and_forget(store: Arc<MockStore>) -> MemoryManager {
    MemoryManager::new(MemoryManagerConfig {
        add_tool_config: Some(
            MemoryAddToolConfig {
                wait_for_writes: Some(false),
                ..Default::default()
            }
            .into(),
        ),
        ..config(vec![store])
    })
    .unwrap()
}

#[tokio::test]
async fn add_tool_with_wait_for_writes_false_returns_accepted_count_fire_and_forget() {
    let notes = writable("notes");
    let manager = fire_and_forget(notes.clone());
    let result = add_tool(&manager, json!({ "entries": ["a", "b"] })).await;
    assert_eq!(returned(&result), json!({ "accepted": 2 }));
    tokio::task::yield_now().await;
    assert_eq!(notes.added(), ["a", "b"]);
}

#[tokio::test]
async fn add_tool_with_wait_for_writes_false_returns_accepted_even_when_a_store_write_fails_swallows_it() {
    let manager = fire_and_forget(failing("failing"));
    let result = add_tool(&manager, json!({ "entries": ["a", "b"] })).await;
    assert_eq!(returned(&result), json!({ "accepted": 2 }));
}

// initAgent

#[tokio::test]
async fn does_not_throw() {
    let manager = manager(vec![store("test")]);
    let mut agent = mock_agent();
    init_agent(&manager, &mut agent).await;
}

fn with_injection(
    stores: Vec<Arc<MockStore>>,
    injection: MemorySetting<MemoryInjectionConfig>,
) -> MemoryManager {
    MemoryManager::new(MemoryManagerConfig {
        injection: Some(injection),
        ..config(stores)
    })
    .unwrap()
}

#[tokio::test]
async fn does_not_register_injection_middleware_when_injection_is_disabled() {
    let manager = with_injection(vec![store("test")], MemorySetting::Off);
    let mut agent = mock_agent();
    init_agent(&manager, &mut agent).await;
    assert!(!agent.middleware().has::<InvokeModelStage>());
}

#[tokio::test]
async fn registers_an_invoke_model_stage_input_middleware_when_injection_is_enabled() {
    let manager = with_injection(vec![store("test")], MemorySetting::On);
    let mut agent = mock_agent();
    init_agent(&manager, &mut agent).await;
    assert!(agent.middleware().has::<InvokeModelStage>());
}

/// Runs a real agent, so the fold is observed on what the model receives.
#[tokio::test]
async fn wires_the_registered_middleware_to_the_memory_provide_pipeline_folds_a_search_hit() {
    let store = with_entries("s", &["dark mode preferred"]);
    let manager = with_injection(vec![store.clone()], MemorySetting::On);
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("ok").into()]);
    let mut agent = Agent::builder()
        .model(model.clone())
        .messages(vec![user("earlier"), assistant("prior")])
        .memory_manager(manager)
        .printer(false)
        .build()
        .unwrap();

    agent.invoke("what is my plan").await.unwrap();

    let sent = &model.requests()[0].messages;
    assert_eq!(
        serde_json::to_value(sent[2].content()).unwrap(),
        json!([
            { "text": "what is my plan" },
            { "text": "\n\n<memory>\n<entry source=\"s\">dark mode preferred</entry>\n</memory>" },
        ])
    );
    assert_eq!(
        serde_json::to_value(sent[1].content()).unwrap(),
        json!([{ "text": "prior" }])
    );
    assert_eq!(search_calls(&store), [("what is my plan".to_owned(), limit(5))]);
    // The injected text is sent, not kept in the history.
    assert_eq!(
        serde_json::to_value(agent.messages()[2].content()).unwrap(),
        json!([{ "text": "what is my plan" }])
    );
}

/// The configured trigger reaches the shared injection engine: a predicate that declines skips
/// the search and the fold.
#[tokio::test]
async fn passes_the_configured_trigger_to_the_injection_middleware() {
    let store = with_entries("s", &["dark mode preferred"]);
    let declined = MemoryInjectionConfig::builder()
        .trigger(InjectionConfig::new().with_trigger_fn(|context| context.messages.len() > 1))
        .build();
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new("ok").into()]);
    model.add_turn(vec![TextBlock::new("ok").into()]);
    let mut agent = Agent::builder()
        .model(model.clone())
        .memory_manager(injecting(store.clone(), declined))
        .printer(false)
        .build()
        .unwrap();

    agent.invoke("first").await.unwrap();
    assert_eq!(
        serde_json::to_value(model.requests()[0].messages[0].content()).unwrap(),
        json!([{ "text": "first" }])
    );
    assert!(search_calls(&store).is_empty());

    agent.invoke("second").await.unwrap();
    assert_eq!(
        serde_json::to_value(model.requests()[1].messages[2].content()).unwrap(),
        json!([
            { "text": "second" },
            { "text": "\n\n<memory>\n<entry source=\"s\">dark mode preferred</entry>\n</memory>" },
        ])
    );
}

// injection

async fn provide(manager: &MemoryManager, messages: &[Message]) -> Option<String> {
    let config = manager.inner.injection.clone().unwrap_or_default();
    manager.provide_memory_context(messages, &config).await.unwrap()
}

fn injecting(store: Arc<MockStore>, injection: MemoryInjectionConfig) -> MemoryManager {
    with_injection(vec![store], MemorySetting::Config(injection))
}

fn ask() -> Vec<Message> {
    vec![assistant("prior"), user("ask")]
}

#[test]
fn defaults_to_an_empty_config_enabled_when_injection_is_omitted() {
    let config = manager(vec![store("s")]).inner.injection.clone().unwrap();
    assert!(config.trigger.is_none() && config.max_entries.is_none());
    assert!(config.query.is_none() && config.format.is_none());
}

#[test]
fn is_false_when_injection_is_explicitly_false() {
    let manager = with_injection(vec![store("s")], false.into());
    assert!(manager.inner.injection.is_none());
}

#[test]
fn resolves_to_an_empty_config_when_injection_is_true() {
    let manager = with_injection(vec![store("s")], true.into());
    let config = manager.inner.injection.clone().unwrap();
    assert!(config.trigger.is_none() && config.max_entries.is_none());
}

#[test]
fn passes_an_injection_config_object_through_unchanged() {
    let manager = injecting(
        store("s"),
        MemoryInjectionConfig {
            max_entries: Some(5),
            ..Default::default()
        },
    );
    assert_eq!(manager.inner.injection.clone().unwrap().max_entries, Some(5));
}

#[tokio::test]
async fn uses_the_latest_user_ask_on_a_user_turn_adaptive_default() {
    let store = with_entries("s", &["fact"]);
    let manager = with_injection(vec![store.clone()], MemorySetting::On);
    provide(&manager, &[assistant("prior step"), user("what is my plan")]).await;
    assert_eq!(search_calls(&store), [("what is my plan".to_owned(), limit(5))]);
}

#[tokio::test]
async fn uses_the_most_recent_assistant_text_on_an_autonomous_tool_result_turn() {
    let store = with_entries("s", &["fact"]);
    let manager = with_injection(vec![store.clone()], MemorySetting::On);
    provide(
        &manager,
        &[user("task"), assistant("the previous step result"), tool_result()],
    )
    .await;
    assert_eq!(
        search_calls(&store),
        [("the previous step result".to_owned(), limit(5))]
    );
}

#[tokio::test]
async fn honors_a_custom_query() {
    let store = with_entries("s", &["fact"]);
    let manager = injecting(
        store.clone(),
        MemoryInjectionConfig {
            query: Some(Arc::new(|_| Ok(Some("custom query".to_owned())))),
            ..Default::default()
        },
    );
    provide(&manager, &ask()).await;
    assert_eq!(search_calls(&store), [("custom query".to_owned(), limit(5))]);
}

#[tokio::test]
async fn skips_returns_undefined_when_a_custom_query_returns_undefined() {
    let store = with_entries("s", &["fact"]);
    let manager = injecting(
        store.clone(),
        MemoryInjectionConfig {
            query: Some(Arc::new(|_| Ok(None))),
            ..Default::default()
        },
    );
    assert_eq!(provide(&manager, &ask()).await, None);
    assert_eq!(store.search_count(), 0);
}

#[tokio::test]
async fn fails_open_returns_undefined_when_a_custom_query_throws() {
    let store = with_entries("s", &["fact"]);
    let manager = injecting(
        store.clone(),
        MemoryInjectionConfig {
            query: Some(Arc::new(|_| Err(Error::other("boom")))),
            ..Default::default()
        },
    );
    assert_eq!(provide(&manager, &ask()).await, None);
    assert_eq!(store.search_count(), 0);
}

#[tokio::test]
async fn skips_when_the_latest_assistant_message_has_no_text_pure_tool_use() {
    let store = with_entries("s", &["fact"]);
    let manager = with_injection(vec![store.clone()], MemorySetting::On);
    assert_eq!(provide(&manager, &[tool_use(), tool_result()]).await, None);
    assert_eq!(store.search_count(), 0);
}

#[tokio::test]
async fn returns_undefined_when_search_yields_no_entries() {
    let store = with_entries("s", &[]);
    let manager = with_injection(vec![store.clone()], MemorySetting::On);
    assert_eq!(provide(&manager, &ask()).await, None);
    assert_eq!(store.search_count(), 1);
}

fn join_contents(entries: &[MemoryEntry], separator: &str) -> String {
    entries
        .iter()
        .map(|entry| entry.content.as_str())
        .collect::<Vec<_>>()
        .join(separator)
}

#[tokio::test]
async fn honors_max_entries_and_caps_the_rendered_entries() {
    let store = with_entries("s", &["A", "B", "C"]);
    let manager = injecting(
        store.clone(),
        MemoryInjectionConfig {
            max_entries: Some(2),
            format: Some(Arc::new(|entries| Ok(join_contents(entries, ",")))),
            ..Default::default()
        },
    );
    let text = provide(&manager, &ask()).await;
    assert_eq!(search_calls(&store), [("ask".to_owned(), limit(2))]);
    assert_eq!(text.as_deref(), Some("A,B"));
}

#[tokio::test]
async fn default_renders_a_memory_block_with_per_entry_source_attribution() {
    let manager = with_injection(
        vec![with_entries("s", &["dark mode preferred"])],
        MemorySetting::On,
    );
    assert_eq!(
        provide(&manager, &ask()).await.as_deref(),
        Some("<memory>\n<entry source=\"s\">dark mode preferred</entry>\n</memory>")
    );
}

#[test]
fn omits_the_source_attribute_for_an_entry_with_no_store_name() {
    assert_eq!(
        default_injection_format(&[MemoryEntry::new("no source")]),
        "<memory>\n<entry>no source</entry>\n</memory>"
    );
}

#[test]
fn escapes_xml_in_entry_content_and_source_so_untrusted_text_cannot_break_the_block() {
    let entry = MemoryEntry::new("a < b & c > d </entry>").with_store_name("pre\"f");
    assert_eq!(
        default_injection_format(&[entry]),
        "<memory>\n<entry source=\"pre&quot;f\">a &lt; b &amp; c &gt; d &lt;/entry&gt;</entry>\n</memory>"
    );
}

#[tokio::test]
async fn honors_a_custom_format() {
    let manager = injecting(
        with_entries("s", &["A"]),
        MemoryInjectionConfig {
            format: Some(Arc::new(|entries| {
                Ok(format!("[{}]", join_contents(entries, "|")))
            })),
            ..Default::default()
        },
    );
    assert_eq!(provide(&manager, &ask()).await.as_deref(), Some("[A]"));
}

#[tokio::test]
async fn fails_open_returns_undefined_when_a_custom_format_throws() {
    let store = with_entries("s", &["fact"]);
    let manager = injecting(
        store.clone(),
        MemoryInjectionConfig {
            format: Some(Arc::new(|_| Err(Error::other("boom")))),
            ..Default::default()
        },
    );
    assert_eq!(provide(&manager, &ask()).await, None);
    assert_eq!(store.search_count(), 1);
}

// AgentConfig integration

#[test]
fn auto_wraps_memory_manager_config_into_memory_manager_instance() {
    let agent = Agent::builder()
        .model(MockMessageModel::new())
        .memory_manager(config(vec![store("test")]))
        .build()
        .unwrap();
    assert!(agent.memory_manager().is_some());
}

#[test]
fn passes_through_memory_manager_instance_unchanged() {
    let manager = manager(vec![store("test")]);
    let agent = Agent::builder()
        .model(MockMessageModel::new())
        .memory_manager(manager.clone())
        .build()
        .unwrap();
    assert!(Arc::ptr_eq(
        &agent.memory_manager().unwrap().inner,
        &manager.inner
    ));
}

#[test]
fn sets_memory_manager_to_undefined_when_not_configured() {
    let agent = Agent::builder().model(MockMessageModel::new()).build().unwrap();
    assert!(agent.memory_manager().is_none());
}

/// Rust-side wiring: the memory plugin sits after delegation and before the context manager.
#[test]
fn registers_memory_between_delegation_and_the_session_manager() {
    let agent = Agent::builder()
        .model(MockMessageModel::new())
        .memory_manager(manager(vec![store("test")]))
        .build()
        .unwrap();
    let names: Vec<String> = agent
        .registration_order()
        .iter()
        .map(|plugin| plugin.name().to_owned())
        .collect();
    let memory = names
        .iter()
        .position(|name| name == "strands:memory-manager")
        .unwrap();
    let delegation = names
        .iter()
        .position(|name| name == "strands:agent-delegation")
        .unwrap();
    assert!(delegation < memory);
    assert_eq!(memory, names.len() - 2, "{names:?}");
}
