//! Ported from strands-ts/src/__tests__/state-store.test.ts

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, json};

use super::StateStore;
use crate::error::Error;
use crate::types::json::JsonValue;

fn store(initial: JsonValue) -> StateStore {
    let JsonValue::Object(map) = initial else {
        panic!("initial state must be an object");
    };
    StateStore::from(map)
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct User {
    name: String,
    age: u64,
}

#[test]
fn creates_empty_state_when_no_initial_state_provided() {
    let state = StateStore::new();
    assert_eq!(state.get_all(), Map::new());
}

#[test]
fn creates_state_with_initial_values() {
    let state = store(json!({ "key1": "value1", "key2": 42 }));
    assert_eq!(state.get_value("key1"), Some(json!("value1")));
    assert_eq!(state.get_value("key2"), Some(json!(42)));
}

#[test]
fn stores_deep_copy_of_initial_state() {
    let mut initial = json!({ "nested": { "value": "test" } });
    let state = store(initial.clone());
    initial["nested"]["value"] = json!("changed");
    assert_eq!(state.get_value("nested"), Some(json!({ "value": "test" })));
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

#[test]
fn infers_correct_type_with_generic_state_interface() {
    let state = store(json!({ "user": { "name": "John", "age": 30 }, "count": 5, "items": ["a", "b"] }));
    let user: Option<User> = state.get("user").unwrap();
    let count: Option<u64> = state.get("count").unwrap();
    let items: Option<Vec<String>> = state.get("items").unwrap();
    assert_eq!(
        user,
        Some(User {
            name: "John".into(),
            age: 30
        })
    );
    assert_eq!(count, Some(5));
    assert_eq!(items, Some(vec!["a".to_owned(), "b".to_owned()]));
}

#[test]
fn returns_undefined_for_non_existent_key_with_typed_interface() {
    let state = store(json!({ "existing": "value" }));
    assert_eq!(state.get::<String>("existing").unwrap().as_deref(), Some("value"));
    assert_eq!(StateStore::new().get::<String>("existing").unwrap(), None);
}

#[test]
fn get_fails_when_the_value_does_not_deserialize_as_the_requested_type() {
    let state = store(json!({ "count": "five" }));
    assert!(matches!(state.get::<u64>("count"), Err(Error::Other(_))));
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
fn throws_error_for_function_in_value() {
    let state = StateStore::new();
    let unserializable: BTreeMap<(u8, u8), u8> = BTreeMap::from([((1, 2), 3)]);
    let error = state.set("key", unserializable).unwrap_err();
    assert!(matches!(error, Error::JsonValidation(_)));
    assert!(
        error
            .to_string()
            .starts_with("value for key \"key\" cannot be serialized: ")
    );
    assert_eq!(state.get_value("key"), None);
}

#[test]
fn accepts_typed_value_with_generic_state_interface() {
    let state = StateStore::new();
    let user = User {
        name: "Alice".into(),
        age: 25,
    };
    state.set("user", &user).unwrap();
    assert_eq!(state.get::<User>("user").unwrap(), Some(user));
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
    let state = StateStore::new();
    state.delete("nonexistent");
    assert!(state.keys().is_empty());
}

#[test]
fn supports_typed_usage_with_generic_state_interface() {
    let state = store(json!({ "user": { "name": "John", "age": 30 }, "count": 5 }));
    state.delete("user");
    assert_eq!(state.get::<User>("user").unwrap(), None);
    assert_eq!(state.get::<u64>("count").unwrap(), Some(5));
}

#[test]
fn delete_keeps_the_order_of_the_remaining_keys() {
    let state = store(json!({ "a": 1, "b": 2, "c": 3 }));
    state.delete("a");
    assert_eq!(state.keys(), ["b", "c"]);
}

#[test]
fn removes_all_values() {
    let state = store(json!({ "key1": "value1", "key2": "value2" }));
    state.clear();
    assert_eq!(state.get_all(), Map::new());
}

#[test]
fn works_on_empty_state() {
    let state = StateStore::new();
    state.clear();
    assert_eq!(state.get_all(), Map::new());
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
    assert_eq!(StateStore::new().get_all(), Map::new());
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
    let mut keys = state.keys();
    keys.push("extra".into());
    assert_eq!(state.keys(), ["key1"]);
}

#[test]
fn returns_deep_copy_of_state() {
    let state = store(json!({ "key1": "value1", "nested": { "deep": true } }));
    let mut json = state.state_to_json();
    assert_eq!(json, json!({ "key1": "value1", "nested": { "deep": true } }));
    json["nested"]["deep"] = json!(false);
    assert_eq!(state.get_value("nested"), Some(json!({ "deep": true })));
}

#[test]
fn replaces_state_with_json_data() {
    let state = store(json!({ "old": "data" }));
    state.load_state_from_json(json!({ "new": "data", "count": 42 }));
    assert_eq!(
        JsonValue::Object(state.get_all()),
        json!({ "new": "data", "count": 42 })
    );
}

#[test]
fn clears_state_when_given_non_object() {
    let state = store(json!({ "key": "value" }));
    state.load_state_from_json(JsonValue::Null);
    assert_eq!(state.get_all(), Map::new());
    state.set("key", "value").unwrap();
    state.load_state_from_json(json!([1, 2]));
    assert_eq!(state.get_all(), Map::new());
}

#[test]
fn clones_share_state() {
    let state = StateStore::new();
    let clone = state.clone();
    clone.set("key", "value").unwrap();
    assert_eq!(state.get_value("key"), Some(json!("value")));
}

#[test]
fn snapshot_is_independent_of_the_original() {
    let state = store(json!({ "key": "value" }));
    let snapshot = state.snapshot();
    snapshot.set("key", "changed").unwrap();
    state.set("other", 1).unwrap();
    assert_eq!(state.get_value("key"), Some(json!("value")));
    assert_eq!(snapshot.keys(), ["key"]);
}

// Expected orders from TS `StateStore.keys()` and `JSON.stringify(state.getAll())` in node 22.
#[test]
fn lists_array_index_keys_first_ascending_as_ts_does() {
    let state = StateStore::new();
    for key in ["b", "2", "1"] {
        state.set(key, key).unwrap();
    }
    assert_eq!(state.keys(), ["1", "2", "b"]);
    assert_eq!(
        crate::types::json::to_string(&state.get_all()).unwrap(),
        r#"{"1":"1","2":"2","b":"b"}"#
    );
}

#[test]
fn keeps_keys_that_are_not_array_indexes_in_insertion_order() {
    let state = StateStore::new();
    for key in ["b", "2", "1", "01", "-1", "4294967295", "4294967294"] {
        state.set(key, key).unwrap();
    }
    assert_eq!(
        state.keys(),
        ["1", "2", "4294967294", "b", "01", "-1", "4294967295"]
    );
    let all: Vec<String> = state.get_all().keys().cloned().collect();
    assert_eq!(all, state.keys());
}
