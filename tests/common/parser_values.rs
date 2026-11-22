use proptest::prelude::*;
use serde_json::Value;

pub fn json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(|v| serde_json::json!(v)),
        ".{0,80}".prop_map(Value::String),
    ]
    .prop_recursive(3, 48, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::btree_map(".{0,16}", inner, 0..6)
                .prop_map(|v| Value::Object(v.into_iter().collect())),
        ]
    })
}
