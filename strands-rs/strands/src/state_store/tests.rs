//! Ported from strands-ts/src/__tests__/state-store.test.ts

use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

use super::*;

fn store(initial: JsonValue) -> StateStore {
    let JsonValue::Object(map) = initial else {
        panic!("expected an object");
    };
    StateStore::from_map(map)
}

struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("contains a function"))
    }
}

mod constructor {
    use super::*;

    #[test]
    fn creates_empty_state_when_no_initial_state_provided() {
        assert_eq!(StateStore::new().get_all(), JsonObject::new());
    }

    #[test]
    fn creates_state_with_initial_values() {
        let state = store(json!({"key1": "value1", "key2": 42}));
        assert_eq!(state.get_value("key1"), Some(json!("value1")));
        assert_eq!(state.get_value("key2"), Some(json!(42)));
    }

    #[test]
    fn stores_deep_copy_of_initial_state() {
        let mut initial = json!({"nested": {"value": "test"}});
        let state = store(initial.clone());
        initial["nested"]["value"] = json!("changed");
        assert_eq!(state.get_value("nested"), Some(json!({"value": "test"})));
    }
}

mod get {
    use super::*;

    #[test]
    fn returns_undefined_when_key_does_not_exist() {
        assert_eq!(StateStore::new().get_value("nonexistent"), None);
        assert_eq!(StateStore::new().get::<String>("nonexistent").unwrap(), None);
    }

    #[test]
    fn returns_value_when_key_exists() {
        let state = store(json!({"key1": "value1"}));
        assert_eq!(state.get::<String>("key1").unwrap().as_deref(), Some("value1"));
    }

    #[test]
    fn returns_deep_copy_that_cannot_mutate_stored_state() {
        let state = store(json!({"nested": {"value": "test"}}));
        let mut retrieved = state.get_value("nested").unwrap();
        retrieved["value"] = json!("changed");
        assert_eq!(state.get_value("nested"), Some(json!({"value": "test"})));
    }

    #[test]
    fn infers_correct_type_with_generic_state_interface() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct User {
            name: String,
            age: u32,
        }
        let state = store(json!({"user": {"name": "John", "age": 30}, "count": 5, "items": ["a", "b"]}));
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
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn returns_undefined_for_non_existent_key_with_typed_interface() {
        let state = store(json!({"existing": "value"}));
        assert_eq!(state.get::<String>("existing").unwrap().as_deref(), Some("value"));
        assert_eq!(StateStore::new().get::<String>("existing").unwrap(), None);
    }

    #[test]
    fn a_value_of_another_type_is_an_error() {
        let state = store(json!({"count": "five"}));
        assert!(state.get::<u32>("count").is_err());
    }
}

mod set {
    use super::*;

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
        state.set("key", json!({"nested": "value"})).unwrap();
        assert_eq!(state.get_value("key"), Some(json!({"nested": "value"})));
    }

    #[test]
    fn sets_array_value_successfully() {
        let state = StateStore::new();
        state.set("key", [1, 2, 3]).unwrap();
        assert_eq!(state.get_value("key"), Some(json!([1, 2, 3])));
    }

    #[test]
    fn overwrites_existing_value() {
        let state = store(json!({"key": "old"}));
        state.set("key", "new").unwrap();
        assert_eq!(state.get_value("key"), Some(json!("new")));
    }

    #[test]
    fn stores_deep_copy_that_cannot_mutate_stored_state() {
        let state = StateStore::new();
        let mut value = json!({"nested": "value"});
        state.set("key", &value).unwrap();
        value["nested"] = json!("changed");
        assert_eq!(state.get_value("key"), Some(json!({"nested": "value"})));
    }

    #[test]
    fn throws_error_for_function_in_value() {
        let state = StateStore::new();
        let error = state.set("key", Unserializable).unwrap_err();
        assert_eq!(error.name(), "JsonValidationError");
        assert_eq!(
            error.to_string(),
            "value for key \"key\" cannot be serialized: contains a function"
        );
        assert_eq!(state.get_value("key"), None);
    }

    #[test]
    fn accepts_typed_value_with_generic_state_interface() {
        #[derive(Serialize)]
        struct User {
            name: &'static str,
            age: u32,
        }
        let state = StateStore::new();
        state
            .set(
                "user",
                User {
                    name: "Jane",
                    age: 25,
                },
            )
            .unwrap();
        assert_eq!(state.get_value("user"), Some(json!({"name": "Jane", "age": 25})));
    }
}

mod delete {
    use super::*;

    #[test]
    fn removes_existing_key() {
        let state = store(json!({"key1": "value1", "key2": "value2"}));
        state.delete("key1");
        assert_eq!(state.get_value("key1"), None);
        assert_eq!(state.get_value("key2"), Some(json!("value2")));
    }

    #[test]
    fn does_not_throw_error_for_non_existent_key() {
        StateStore::new().delete("nonexistent");
    }

    #[test]
    fn keeps_the_order_of_the_remaining_keys() {
        let state = store(json!({"a": 1, "b": 2, "c": 3}));
        state.delete("a");
        assert_eq!(state.keys(), ["b", "c"]);
    }
}

mod clear {
    use super::*;

    #[test]
    fn removes_all_values() {
        let state = store(json!({"key1": "value1", "key2": "value2"}));
        state.clear();
        assert!(state.keys().is_empty());
    }

    #[test]
    fn works_on_empty_state() {
        let state = StateStore::new();
        state.clear();
        assert!(state.keys().is_empty());
    }
}

mod get_all {
    use super::*;

    #[test]
    fn returns_object_with_all_state() {
        let state = store(json!({"key1": "value1", "key2": 42}));
        assert_eq!(
            JsonValue::Object(state.get_all()),
            json!({"key1": "value1", "key2": 42})
        );
    }

    #[test]
    fn returns_empty_object_for_empty_state() {
        assert!(StateStore::new().get_all().is_empty());
    }
}

mod keys {
    use super::*;

    #[test]
    fn returns_array_of_all_keys() {
        let state = store(json!({"key1": "value1", "key2": "value2"}));
        assert_eq!(state.keys(), ["key1", "key2"]);
    }

    #[test]
    fn returns_empty_array_for_empty_state() {
        assert!(StateStore::new().keys().is_empty());
    }

    #[test]
    fn returns_new_array_each_time() {
        let state = store(json!({"key1": "value1"}));
        let mut first = state.keys();
        first.push("mutated".into());
        assert_eq!(state.keys(), ["key1"]);
    }
}

mod state_to_json_symbol {
    use super::*;

    #[test]
    fn returns_deep_copy_of_state() {
        let state = store(json!({"key1": "value1"}));
        let mut json = state.to_json();
        json["key1"] = json!("changed");
        assert_eq!(state.to_json(), json!({"key1": "value1"}));
    }
}

mod load_state_from_json_symbol {
    use super::*;

    #[test]
    fn replaces_state_with_json_data() {
        let state = store(json!({"old": "value"}));
        state.load_json(json!({"new": "data"}));
        assert_eq!(state.to_json(), json!({"new": "data"}));
    }

    #[test]
    fn clears_state_when_given_non_object() {
        let state = store(json!({"key": "value"}));
        state.load_json(json!([1, 2]));
        assert!(state.keys().is_empty());
        state.set("key", 1).unwrap();
        state.load_json(JsonValue::Null);
        assert!(state.keys().is_empty());
    }
}

mod handles {
    use super::*;

    #[test]
    fn clones_share_state() {
        let state = StateStore::new();
        let clone = state.clone();
        clone.set("key", 1).unwrap();
        assert_eq!(state.get_value("key"), Some(json!(1)));
    }

    #[test]
    fn snapshot_is_independent() {
        let state = store(json!({"key": 1}));
        let snapshot = state.snapshot();
        snapshot.set("key", 2).unwrap();
        state.set("other", 3).unwrap();
        assert_eq!(state.get_value("key"), Some(json!(1)));
        assert_eq!(snapshot.keys(), ["key"]);
    }
}
