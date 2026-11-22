use crate::smart_rules::{RuleGroup, RuleNode, SmartRules};
use serde_json::{Value, json};

pub const FIELDS: &[(&str, &str)] = &[
    ("title", "Title"),
    ("album", "Album"),
    ("artist", "Artist"),
    ("albumartist", "Album artist"),
    ("genre", "Genre"),
    ("filetype", "File type"),
    ("year", "Year"),
    ("rating", "Rating"),
    ("playcount", "Play count"),
    ("duration", "Duration (seconds)"),
    ("bitrate", "Bitrate"),
    ("bpm", "BPM"),
    ("loved", "Loved"),
    ("dateadded", "Date added"),
    ("lastplayed", "Last played"),
    ("dateloved", "Date loved"),
];

pub fn operators(field: &str) -> &'static [(&'static str, &'static str)] {
    match field {
        "year" | "rating" | "playcount" | "duration" | "bitrate" | "bpm" => &[
            ("is", "is"),
            ("isNot", "is not"),
            ("gt", "greater than"),
            ("lt", "less than"),
            ("inTheRange", "in the range"),
        ],
        "loved" => &[("is", "is")],
        "dateadded" | "lastplayed" | "dateloved" => &[
            ("before", "before"),
            ("after", "after"),
            ("inTheLast", "in the last"),
            ("notInTheLast", "not in the last"),
        ],
        _ => &[
            ("is", "is"),
            ("isNot", "is not"),
            ("contains", "contains"),
            ("notContains", "does not contain"),
            ("startsWith", "starts with"),
            ("endsWith", "ends with"),
        ],
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Condition {
        field: String,
        op: String,
        value: String,
    },
    Group {
        any: bool,
        rows: Vec<Row>,
    },
    Advanced(RuleNode),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u64,
    pub kind: RowKind,
}

impl Row {
    pub fn has_advanced(&self) -> bool {
        match &self.kind {
            RowKind::Advanced(_) => true,
            RowKind::Group { rows, .. } => rows.iter().any(Self::has_advanced),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Template {
    RecentlyAdded,
    MostPlayed,
    NotPlayed,
    Loved,
    DailyMix,
}

impl Template {
    pub const ALL: [Self; 5] = [
        Self::RecentlyAdded,
        Self::MostPlayed,
        Self::NotPlayed,
        Self::Loved,
        Self::DailyMix,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "Recently Added",
            Self::MostPlayed => "Most Played",
            Self::NotPlayed => "Not Played in 90 Days",
            Self::Loved => "Loved",
            Self::DailyMix => "Daily Mix",
        }
    }
    fn rules(self) -> SmartRules {
        serde_json::from_value(match self {
            Self::RecentlyAdded=>json!({"all":[{"inTheLast":{"dateadded":"30d"}}],"sort":"dateadded","order":"desc","limit":100}),
            Self::MostPlayed=>json!({"all":[{"gt":{"playcount":0}}],"sort":"playcount","order":"desc","limit":100}),
            Self::NotPlayed=>json!({"all":[{"notInTheLast":{"lastplayed":"90d"}}],"sort":"random","limit":100}),
            Self::Loved=>json!({"all":[{"is":{"loved":true}}],"sort":"dateloved","order":"desc","limit":100}),
            Self::DailyMix=>json!({"all":[{"gt":{"duration":0}}],"sort":"random","limit":25,"refreshDelay":"1d"}),
        }).unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub id: Option<String>,
    pub name: String,
    pub any: bool,
    pub rows: Vec<Row>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub limit: String,
    original: SmartRules,
    next_id: u64,
}

impl Editor {
    pub fn new() -> Self {
        let mut editor = Self::load(None, String::new(), SmartRules::default());
        editor.add_condition(None);
        editor
    }

    pub fn load(id: Option<String>, name: String, rules: SmartRules) -> Self {
        let mut editor = Self {
            id,
            name,
            any: rules.root.any,
            rows: vec![],
            sort: rules.sort.clone(),
            order: rules.order.clone(),
            limit: rules.limit.map(|n| n.to_string()).unwrap_or_default(),
            original: rules.clone(),
            next_id: 0,
        };
        editor.rows = rules
            .root
            .items
            .into_iter()
            .map(|n| editor.row(n, 0))
            .collect();
        editor
    }

    fn row(&mut self, node: RuleNode, depth: usize) -> Row {
        let id = self.next_id;
        self.next_id += 1;
        let kind = match node {
            RuleNode::Group(group) if depth == 0 => RowKind::Group {
                any: group.any,
                rows: group
                    .items
                    .into_iter()
                    .map(|n| self.row(n, depth + 1))
                    .collect(),
            },
            RuleNode::Condition {
                ref op,
                ref field,
                ref value,
            } if FIELDS.iter().any(|(f, _)| f == field)
                && operators(field).iter().any(|(o, _)| o == op)
                && editable_value(field, op, value) =>
            {
                RowKind::Condition {
                    field: field.clone(),
                    op: op.clone(),
                    value: match value {
                        Value::String(s) => s.clone(),
                        _ => value.to_string(),
                    },
                }
            }
            node => RowKind::Advanced(node),
        };
        Row { id, kind }
    }

    pub fn row_mut(&mut self, id: u64) -> Option<&mut Row> {
        fn find(rows: &mut [Row], id: u64) -> Option<&mut Row> {
            for row in rows {
                if row.id == id {
                    return Some(row);
                }
                if let RowKind::Group { rows, .. } = &mut row.kind
                    && let Some(row) = find(rows, id)
                {
                    return Some(row);
                }
            }
            None
        }
        find(&mut self.rows, id)
    }

    pub fn add_condition(&mut self, parent: Option<u64>) {
        let row = Row {
            id: self.next_id,
            kind: RowKind::Condition {
                field: "title".into(),
                op: "is".into(),
                value: String::new(),
            },
        };
        self.next_id += 1;
        match parent {
            None => self.rows.push(row),
            Some(id) => {
                if let Some(Row {
                    kind: RowKind::Group { rows, .. },
                    ..
                }) = self.row_mut(id)
                {
                    rows.push(row);
                }
            }
        }
    }

    pub fn add_group(&mut self) {
        let id = self.next_id;
        self.next_id += 1;
        self.rows.push(Row {
            id,
            kind: RowKind::Group {
                any: false,
                rows: vec![],
            },
        });
        self.add_condition(Some(id));
    }

    pub fn remove(&mut self, id: u64) {
        fn remove(rows: &mut Vec<Row>, id: u64) {
            rows.retain(|r| r.id != id || r.has_advanced());
            for row in rows {
                if let RowKind::Group { rows, .. } = &mut row.kind {
                    remove(rows, id);
                }
            }
        }
        remove(&mut self.rows, id);
    }

    pub fn field(&mut self, id: u64, field: &str) {
        if let Some(Row {
            kind:
                RowKind::Condition {
                    field: f,
                    op,
                    value,
                },
            ..
        }) = self.row_mut(id)
        {
            *f = field.into();
            *op = operators(field)[0].0.into();
            *value = if field == "loved" {
                "true".into()
            } else {
                String::new()
            };
        }
    }

    pub fn template(&mut self, template: Template) {
        let advanced: Vec<_> = self
            .rows
            .iter()
            .filter(|r| r.has_advanced())
            .cloned()
            .collect();
        let rules = template.rules();
        self.next_id = 0;
        self.rows = rules
            .root
            .items
            .into_iter()
            .map(|n| self.row(n, 0))
            .collect();
        fn rekey(row: &mut Row, next: &mut u64) {
            row.id = *next;
            *next += 1;
            if let RowKind::Group { rows, .. } = &mut row.kind {
                for row in rows {
                    rekey(row, next);
                }
            }
        }
        for mut row in advanced {
            rekey(&mut row, &mut self.next_id);
            self.rows.push(row);
        }
        self.any = false;
        self.sort = rules.sort;
        self.order = rules.order;
        self.limit = rules.limit.map(|n| n.to_string()).unwrap_or_default();
        self.original.refresh_delay = rules.refresh_delay;
        if self.name.trim().is_empty() {
            self.name = template.label().into();
        }
    }

    pub fn build(&self) -> Result<SmartRules, &'static str> {
        if self.name.trim().is_empty() {
            return Err("Enter a playlist name");
        }
        if self.rows.is_empty() {
            return Err("Add at least one condition");
        }
        let mut rules = self.original.clone();
        rules.root = RuleGroup {
            any: self.any,
            items: self.rows.iter().map(build_row).collect::<Result<_, _>>()?,
        };
        rules.sort = self.sort.clone();
        rules.order = self.order.clone();
        rules.limit = if self.limit.trim().is_empty() {
            None
        } else {
            Some(
                self.limit
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("Enter a positive whole-number limit")?,
            )
        };
        Ok(rules)
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

fn editable_value(field: &str, op: &str, value: &Value) -> bool {
    if field == "loved" {
        value.is_boolean()
    } else if op == "inTheRange" {
        value
            .as_array()
            .is_some_and(|a| a.len() == 2 && a.iter().all(Value::is_number))
    } else if matches!(
        field,
        "year" | "rating" | "playcount" | "duration" | "bitrate" | "bpm"
    ) {
        value.is_number()
    } else {
        value.is_string()
    }
}

fn build_row(row: &Row) -> Result<RuleNode, &'static str> {
    Ok(match &row.kind {
        RowKind::Advanced(node) => node.clone(),
        RowKind::Group { any, rows } => {
            if rows.is_empty() {
                return Err("Add a condition to every group");
            }
            RuleNode::Group(RuleGroup {
                any: *any,
                items: rows.iter().map(build_row).collect::<Result<_, _>>()?,
            })
        }
        RowKind::Condition { field, op, value } => {
            if value.trim().is_empty() {
                return Err("Enter a value for every condition");
            }
            let value = match field.as_str() {
                "loved" => Value::Bool(value.parse().map_err(|_| "Choose true or false")?),
                "year" | "rating" | "playcount" | "duration" | "bitrate" | "bpm" => {
                    let parsed: Value = serde_json::from_str(value.trim())
                        .map_err(|_| "Enter a number (or [minimum, maximum] for a range)")?;
                    if op == "inTheRange" {
                        if !parsed
                            .as_array()
                            .is_some_and(|a| a.len() == 2 && a.iter().all(Value::is_number))
                        {
                            return Err("Enter a range as [minimum, maximum]");
                        }
                    } else if !parsed.is_number() {
                        return Err("Enter a number");
                    }
                    parsed
                }
                _ => Value::String(value.clone()),
            };
            RuleNode::Condition {
                field: field.clone(),
                op: op.clone(),
                value,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_groups_match_sort_and_validation() {
        let mut e = Editor::new();
        assert_eq!(e.build().unwrap_err(), "Enter a playlist name");
        e.name = "Test".into();
        assert!(e.build().is_err());
        e.field(0, "year");
        if let RowKind::Condition { op, value, .. } = &mut e.row_mut(0).unwrap().kind {
            *op = "inTheRange".into();
            *value = "[1990, 2000]".into();
        }
        e.add_group();
        e.add_condition(Some(1));
        e.remove(3);
        e.remove(2);
        e.remove(1);
        e.any = true;
        e.sort = Some("year".into());
        e.order = Some("desc".into());
        e.limit = "20".into();
        assert_eq!(
            serde_json::to_value(e.build().unwrap()).unwrap(),
            json!({"any":[{"inTheRange":{"year":[1990,2000]}}],"sort":"year","order":"desc","limit":20})
        );
        e.limit = "-1".into();
        assert!(e.build().is_err());
        e.limit.clear();
        e.remove(0);
        assert!(e.build().is_err());
    }
    #[test]
    fn all_templates_build_and_preserve_advanced_rules() {
        for t in Template::ALL {
            let rules:SmartRules=serde_json::from_value(json!({"any":[{"future":{"x":1}}],"offset":3,"refreshDelay":"2d","limitPercent":10})).unwrap();
            let mut e = Editor::load(Some("smart".into()), "Name".into(), rules);
            e.remove(0);
            assert_eq!(e.rows.len(), 1);
            e.template(t);
            let result = e.build().unwrap();
            assert_eq!(result.extra["offset"], 3);
            assert_eq!(result.limit_percent, Some(10));
            assert!(e.rows.iter().any(Row::has_advanced));
        }
    }
    #[test]
    fn loaded_rules_round_trip_including_deep_and_unsupported_conditions() {
        let value = json!({"all":[{"is":{"genre":"Jazz"}},{"any":[{"all":[{"is":{"loved":true}}]}]},{"is":{"unknownField":5}},{"is":{"genre":["Jazz"]}},{"is":{"year":"2000"}}],"sort":"-year,title","order":"desc","limit":5,"refreshDelay":"2h","future":true});
        let mut e = Editor::load(
            Some("smart".into()),
            "Name".into(),
            serde_json::from_value(value.clone()).unwrap(),
        );
        assert_eq!(serde_json::to_value(e.build().unwrap()).unwrap(), value);
        e.remove(1);
        assert_eq!(e.rows.len(), 5);
    }

    #[test]
    fn templates_have_expected_rules() {
        let expected = [
            json!({"all":[{"inTheLast":{"dateadded":"30d"}}],"sort":"dateadded","order":"desc","limit":100}),
            json!({"all":[{"gt":{"playcount":0}}],"sort":"playcount","order":"desc","limit":100}),
            json!({"all":[{"notInTheLast":{"lastplayed":"90d"}}],"sort":"random","limit":100}),
            json!({"all":[{"is":{"loved":true}}],"sort":"dateloved","order":"desc","limit":100}),
            json!({"all":[{"gt":{"duration":0}}],"sort":"random","limit":25,"refreshDelay":"1d"}),
        ];
        for (template, expected) in Template::ALL.into_iter().zip(expected) {
            let mut e = Editor::new();
            e.template(template);
            assert_eq!(e.name, template.label());
            assert_eq!(serde_json::to_value(e.build().unwrap()).unwrap(), expected);
        }
    }

    #[test]
    fn nested_groups_and_invalid_values() {
        let mut e = Editor::new();
        e.name = "Name".into();
        e.remove(0);
        e.add_group();
        if let RowKind::Group { any, rows } = &mut e.row_mut(1).unwrap().kind {
            *any = true;
            if let RowKind::Condition { value, .. } = &mut rows[0].kind {
                *value = "Jazz".into();
            }
        }
        assert_eq!(
            serde_json::to_value(e.build().unwrap()).unwrap(),
            json!({"all":[{"any":[{"is":{"title":"Jazz"}}]}]})
        );
        e.field(2, "year");
        if let RowKind::Condition { value, .. } = &mut e.row_mut(2).unwrap().kind {
            *value = "not a number".into();
        }
        assert!(e.build().is_err());
        e.field(2, "loved");
        assert!(e.build().is_ok());
        if let RowKind::Condition { value, .. } = &mut e.row_mut(2).unwrap().kind {
            *value = "maybe".into();
        }
        assert!(e.build().is_err());
        e.remove(2);
        assert_eq!(e.build().unwrap_err(), "Add a condition to every group");
    }

    #[test]
    fn template_keeps_advanced_group_even_with_unfinished_conditions() {
        let rules = serde_json::from_value(json!({"all":[{"any":[{"future":{"x":1}}]}]})).unwrap();
        let mut e = Editor::load(None, "Name".into(), rules);
        e.add_condition(Some(0));
        e.template(Template::Loved);
        assert!(e.rows.iter().any(Row::has_advanced));
        assert_eq!(e.build().unwrap_err(), "Enter a value for every condition");
    }
}
