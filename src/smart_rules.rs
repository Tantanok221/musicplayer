use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmartRules {
    pub root: RuleGroup,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: Option<u32>,
    pub limit_percent: Option<u32>,
    pub refresh_delay: Option<String>,
    pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuleGroup {
    pub any: bool,
    pub items: Vec<RuleNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuleNode {
    Group(RuleGroup),
    Condition {
        op: String,
        field: String,
        value: Value,
    },
    Raw(Value),
}

impl RuleGroup {
    fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.len() != 1 {
            return None;
        }
        let (key, items) = object.iter().next()?;
        if key != "all" && key != "any" {
            return None;
        }
        Some(Self {
            any: key == "any",
            items: items
                .as_array()?
                .iter()
                .cloned()
                .map(RuleNode::from_value)
                .collect(),
        })
    }

    fn value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            if self.any { "any" } else { "all" }.into(),
            Value::Array(self.items.iter().map(RuleNode::value).collect()),
        );
        Value::Object(object)
    }
}

impl RuleNode {
    fn from_value(value: Value) -> Self {
        if let Some(group) = RuleGroup::from_value(&value) {
            return Self::Group(group);
        }
        if let Some(object) = value.as_object().filter(|object| object.len() == 1) {
            let (op, fields) = object.iter().next().unwrap();
            if matches!(
                op.as_str(),
                "is" | "isNot"
                    | "gt"
                    | "lt"
                    | "contains"
                    | "notContains"
                    | "startsWith"
                    | "endsWith"
                    | "inTheRange"
                    | "before"
                    | "after"
                    | "inTheLast"
                    | "notInTheLast"
                    | "inPlaylist"
                    | "notInPlaylist"
                    | "isMissing"
                    | "isPresent"
            ) && let Some(fields) = fields.as_object().filter(|fields| fields.len() == 1)
            {
                let (field, value) = fields.iter().next().unwrap();
                return Self::Condition {
                    op: op.clone(),
                    field: field.clone(),
                    value: value.clone(),
                };
            }
        }
        Self::Raw(value)
    }

    fn value(&self) -> Value {
        match self {
            Self::Group(group) => group.value(),
            Self::Condition { op, field, value } => {
                let mut fields = Map::new();
                fields.insert(field.clone(), value.clone());
                let mut object = Map::new();
                object.insert(op.clone(), Value::Object(fields));
                Value::Object(object)
            }
            Self::Raw(value) => value.clone(),
        }
    }
}

impl Serialize for RuleNode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for RuleNode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from_value(Value::deserialize(deserializer)?))
    }
}
impl Serialize for RuleGroup {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value().serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for RuleGroup {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_value(&Value::deserialize(deserializer)?)
            .ok_or_else(|| serde::de::Error::custom("Expected one all/any rule group"))
    }
}

fn take<T: DeserializeOwned>(map: &mut Map<String, Value>, key: &str) -> Option<T> {
    let parsed = serde_json::from_value(map.get(key)?.clone()).ok()?;
    map.remove(key);
    Some(parsed)
}

impl<'de> Deserialize<'de> for SmartRules {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut extra = Map::<String, Value>::deserialize(deserializer)?;
        let key = match (extra.contains_key("all"), extra.contains_key("any")) {
            (true, false) => "all",
            (false, true) => "any",
            _ => return Err(serde::de::Error::custom("Expected one all/any rule group")),
        };
        let mut group = Map::new();
        group.insert(key.into(), extra.remove(key).unwrap());
        let root = RuleGroup::from_value(&Value::Object(group))
            .ok_or_else(|| serde::de::Error::custom("Expected one all/any rule group"))?;
        Ok(Self {
            root,
            sort: take(&mut extra, "sort"),
            order: take(&mut extra, "order"),
            limit: take(&mut extra, "limit"),
            limit_percent: take(&mut extra, "limitPercent"),
            refresh_delay: take(&mut extra, "refreshDelay"),
            extra,
        })
    }
}

impl Serialize for SmartRules {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = self.extra.clone();
        object.extend(self.root.value().as_object().unwrap().clone());
        for (key, value) in [
            ("sort", self.sort.as_ref().map(|s| Value::String(s.clone()))),
            (
                "order",
                self.order.as_ref().map(|s| Value::String(s.clone())),
            ),
            ("limit", self.limit.map(Value::from)),
            ("limitPercent", self.limit_percent.map(Value::from)),
            (
                "refreshDelay",
                self.refresh_delay
                    .as_ref()
                    .map(|s| Value::String(s.clone())),
            ),
        ] {
            if let Some(value) = value {
                object.insert(key.into(), value);
            }
        }
        object.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn navidrome_examples_round_trip() {
        for value in [
            json!({"all":[]}),
            json!({"any":[{"is":{"loved":true}}],"sort":"random","limit":25,"refreshDelay":"1d"}),
            json!({"all":[{"any":[{"contains":{"artist":"Miles"}},{"isNot":{"albumartist":"Various Artists"}}]},{"inTheRange":{"year":[1960,1970]}},{"notInTheLast":{"lastplayed":"90d"}}],"sort":"-year,title","order":"asc","limitPercent":15}),
            json!({"all":[{"after":{"dateadded":"2025-01-01"}},{"before":{"dateloved":"2026-01-01"}},{"inTheLast":{"dateadded":"30d"}},{"gt":{"playcount":10}},{"lt":{"duration":300}},{"startsWith":{"title":"A"}},{"endsWith":{"filetype":"flac"}},{"notContains":{"genre":"Rock"}}],"order":"desc","offset":4}),
        ] {
            let rules: SmartRules = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(rules).unwrap(), value);
        }
    }

    #[test]
    fn unknown_and_malformed_nodes_and_metadata_are_preserved() {
        let nodes = vec![
            json!({"future":{"genre":["Jazz"]}}),
            json!({"is":{"genre":"Jazz","year":2020}}),
            json!({"all":[],"any":[]}),
            json!({"all":false}),
            json!({"is":"Jazz"}),
            json!(null),
            json!([1, 2]),
        ];
        let value = json!({"all":nodes,"sort":null,"limit":-1,"limitPercent":1.5,"refreshDelay":{"future":true},"future":{"x":1}});
        let rules: SmartRules = serde_json::from_value(value.clone()).unwrap();
        assert!(
            rules
                .root
                .items
                .iter()
                .all(|n| matches!(n, RuleNode::Raw(_)))
        );
        assert_eq!(serde_json::to_value(rules).unwrap(), value);
    }

    #[test]
    fn invalid_root_is_rejected_instead_of_discarded() {
        for value in [
            json!(null),
            json!({}),
            json!({"all":null}),
            json!({"all":[],"any":[]}),
        ] {
            assert!(serde_json::from_value::<SmartRules>(value).is_err());
        }
    }
}
