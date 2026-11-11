//! Ported from strands-ts/src/__tests__/state-store.test.ts

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::StateStore;
use crate::error::Error;
use crate::types::json::JsonValue;

fn store(initial: JsonValue) -> StateStore {
    match initial {
        JsonValue::Object(map) => StateStore::from(map),
        _ => panic!("initial state must be an object"),
    }
}

#[test]
fn creates_empty_state_when_no_initial_state_provided() {
    let state = StateStore::new();
    assert!(state.get_all().is_empty());
}

#[test]
fn creates_state_with_initial_values() {
    let state = store(json!({ "key1": "value1", "key2": 42 }));
    assert_eq!(state.get_value("key1"), Some(json!("value1")));
    assert_eq!(state.get_value("key2"), Some(json!(42)));
}

#[test]
fn stores_deep_copy_of_initial_state() {
    let mut initial = json!({ "nested": { "value": "original" } });
    let state = store(initial.clone());
    initial["nested"]["value"] = json!("changed");
    assert_eq!(state.get_value("nested"), Some(json!({ "value": "original" })));
}

#[test]
fn returns_undefined_when_key_does_not_exist() {
    let state = StateStore::new();
    assert_eq!(state.get_value("nonexistent"), None);
    assert_eq!(state.get::<String>("nonexistent").unwrap(), None);
}

#[test]
fn returns_value_when_key_exists() {
    let state = store(json!({ "key1": "value1" }));
    assert_eq!(state.get::<String>("key1").unwrap().as_deref(), Some("value1"));
}

#[test]
fn returns_deep_copy_that_cannot_mutate_stored_state() {
    let state = store(json!({ "nested": { "value": "test" } }));
    let mut retrieved = state.get_value("nested").unwrap();
    retrieved["value"] = json!("changed");
    assert_eq!(state.get_value("nested"), Some(json!({ "value": "test" })));
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct User {
    name: String,
    age: u32,
}

#[test]
fn infers_correct_type_with_generic_state_interface() {
    let state = store(json!({ "user": { "name": "John", "age": 30 }, "count": 5, "items": ["a", "b"] }));
    assert_eq!(
        state.get::<User>("user").unwrap(),
        Some(User {
            name: "John".into(),
            age: 30
        })
    );
    assert_eq!(state.get::<u32>("count").unwrap(), Some(5));
    assert_eq!(
        state.get::<Vec<String>>("items").unwrap(),
        Some(vec!["a".into(), "b".into()])
    );
}

#[test]
fn returns_undefined_for_non_existent_key_with_typed_interface() {
    let state = store(json!({ "existing": "value" }));
    assert_eq!(state.get::<String>("existing").unwrap().as_deref(), Some("value"));
    assert_eq!(StateStore::new().get::<String>("existing").unwrap(), None);
}

#[test]
fn get_reports_a_value_of_the_wrong_type() {
    let state = store(json!({ "count": "five" }));
    assert!(matches!(state.get::<u32>("count"), Err(Error::Json(_))));
}

#[test]
fn sets_string_value_successfully() {
    let state = StateStore::new();
    state.set("key", "value").unwrap();
    assert_eq!(state.get_value("key"), Some(json!("value")));
}

#[test]
fn sets_number_value_successfully() {
    let state = StateStore::new();
    state.set("key", 42).unwrap();
    assert_eq!(state.get_value("key"), Some(json!(42)));
}

#[test]
fn sets_boolean_value_successfully() {
    let state = StateStore::new();
    state.set("key", true).unwrap();
    assert_eq!(state.get_value("key"), Some(json!(true)));
}

#[test]
fn sets_null_value_successfully() {
    let state = StateStore::new();
    state.set("key", JsonValue::Null).unwrap();
    assert_eq!(state.get_value("key"), Some(JsonValue::Null));
}

#[test]
fn sets_object_value_successfully() {
    let state = StateStore::new();
    state.set("key", json!({ "nested": "value" })).unwrap();
    assert_eq!(state.get_value("key"), Some(json!({ "nested": "value" })));
}

#[test]
fn sets_array_value_successfully() {
    let state = StateStore::new();
    state.set("key", [1, 2, 3]).unwrap();
    assert_eq!(state.get_value("key"), Some(json!([1, 2, 3])));
}

#[test]
fn overwrites_existing_value() {
    let state = store(json!({ "key": "old" }));
    state.set("key", "new").unwrap();
    assert_eq!(state.get_value("key"), Some(json!("new")));
}

#[test]
fn stores_deep_copy_that_cannot_mutate_stored_state() {
    let state = StateStore::new();
    let mut value = json!({ "nested": { "value": "test" } });
    state.set("key", &value).unwrap();
    value["nested"]["value"] = json!("changed");
    assert_eq!(
        state.get_value("key"),
        Some(json!({ "nested": { "value": "test" } }))
    );
}

#[test]
fn accepts_typed_value_with_generic_state_interface() {
    let state = StateStore::new();
    state
        .set(
            "user",
            User {
                name: "Alice".into(),
                age: 25,
            },
        )
        .unwrap();
    assert_eq!(
        state.get_value("user"),
        Some(json!({ "name": "Alice", "age": 25 }))
    );
}

#[test]
fn set_rejects_a_value_json_cannot_represent() {
    let state = StateStore::new();
    let value = std::collections::BTreeMap::from([((1, 2), "pair key")]);
    let error = state.set("key", value).unwrap_err();
    assert!(matches!(error, Error::JsonValidation(_)));
    assert!(
        error
            .to_string()
            .starts_with("value for key \"key\" cannot be serialized")
    );
    assert_eq!(state.get_value("key"), None);
}

#[test]
fn removes_existing_key() {
    let state = store(json!({ "key1": "value1", "key2": "value2" }));
    state.delete("key1");
    assert_eq!(state.get_value("key1"), None);
    assert_eq!(state.get_value("key2"), Some(json!("value2")));
}

#[test]
fn does_not_throw_error_for_non_existent_key() {
    StateStore::new().delete("nonexistent");
}

#[test]
fn supports_typed_usage_with_generic_state_interface() {
    let state = store(json!({ "user": { "name": "John" }, "count": 5 }));
    state.delete("user");
    assert_eq!(state.keys(), ["count"]);
}

#[test]
fn removes_all_values() {
    let state = store(json!({ "key1": "value1", "key2": "value2" }));
    state.clear();
    assert!(state.get_all().is_empty());
}

#[test]
fn works_on_empty_state() {
    let state = StateStore::new();
    state.clear();
    assert!(state.get_all().is_empty());
}

#[test]
fn returns_object_with_all_state() {
    let state = store(json!({ "key1": "value1", "key2": 42 }));
    assert_eq!(
        JsonValue::Object(state.get_all()),
        json!({ "key1": "value1", "key2": 42 })
    );
}

#[test]
fn returns_empty_object_for_empty_state() {
    assert!(StateStore::new().get_all().is_empty());
}

#[test]
fn returns_array_of_all_keys() {
    let state = store(json!({ "key1": "value1", "key2": "value2", "key3": "value3" }));
    assert_eq!(state.keys(), ["key1", "key2", "key3"]);
}

#[test]
fn returns_empty_array_for_empty_state() {
    assert!(StateStore::new().keys().is_empty());
}

#[test]
fn returns_new_array_each_time() {
    let state = store(json!({ "key1": "value1" }));
    let mut first = state.keys();
    first.push("extra".into());
    assert_eq!(state.keys(), ["key1"]);
}

#[test]
fn returns_deep_copy_of_state() {
    let state = store(json!({ "key1": "value1", "nested": { "deep": true } }));
    let mut json = state.to_json();
    assert_eq!(json, json!({ "key1": "value1", "nested": { "deep": true } }));
    json["nested"]["deep"] = json!(false);
    assert_eq!(state.get_value("nested"), Some(json!({ "deep": true })));
}

#[test]
fn can_be_accessed_via_serialize_state_serializable_helper() {
    let state = store(json!({ "key1": "value1" }));
    assert_eq!(state.to_json(), json!({ "key1": "value1" }));
}

#[test]
fn replaces_state_with_json_data() {
    let state = store(json!({ "old": "data" }));
    state.load_json(json!({ "new": "data", "count": 42 }));
    assert_eq!(
        JsonValue::Object(state.get_all()),
        json!({ "new": "data", "count": 42 })
    );
}

#[test]
fn clears_state_when_given_non_object() {
    let state = store(json!({ "key": "value" }));
    state.load_json(JsonValue::Null);
    assert!(state.get_all().is_empty());
}

#[test]
fn can_be_accessed_via_load_state_serializable_helper() {
    let state = store(json!({ "old": "data" }));
    state.load_json(json!({ "new": "data" }));
    assert_eq!(JsonValue::Object(state.get_all()), json!({ "new": "data" }));
}

#[test]
fn clones_share_state_and_snapshots_do_not() {
    let state = StateStore::new();
    let shared = state.clone();
    let snapshot = state.snapshot();
    shared.set("key", 1).unwrap();
    assert_eq!(state.get_value("key"), Some(json!(1)));
    assert_eq!(snapshot.get_value("key"), None);
}
