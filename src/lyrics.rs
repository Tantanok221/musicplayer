use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Lyrics {
    pub synced: bool,
    pub offset: i64,
    pub line: Vec<LyricLine>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct LyricLine {
    pub start: Option<u64>,
    pub value: String,
}

impl Lyrics {
    pub fn plain(text: &str) -> Self {
        Self {
            line: text
                .lines()
                .map(|value| LyricLine {
                    start: None,
                    value: value.into(),
                })
                .collect(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.line.iter().all(|line| line.value.trim().is_empty())
    }

    pub fn is_timed(&self) -> bool {
        self.synced && self.line.iter().any(|line| line.start.is_some())
    }

    /// OpenSubsonic offsets are milliseconds; positive offsets display lyrics sooner.
    pub fn active_line(&self, position: f64) -> Option<usize> {
        if !self.is_timed() || !position.is_finite() {
            return None;
        }
        let now = position.max(0.) * 1000. + self.offset as f64;
        self.line
            .iter()
            .enumerate()
            .filter_map(|(index, line)| line.start.map(|start| (index, start)))
            .filter(|(_, start)| *start as f64 <= now)
            .max_by_key(|(_, start)| *start)
            .map(|(index, _)| index)
    }

    pub fn line_position(&self, index: usize) -> Option<f64> {
        if !self.is_timed() {
            return None;
        }
        let start = self.line.get(index)?.start?;
        Some(((start as f64 - self.offset as f64) / 1000.).max(0.))
    }

    pub fn line_is_active(&self, index: usize, active: Option<usize>) -> bool {
        active.is_some_and(|active| {
            let start = self.line.get(index).and_then(|line| line.start);
            start.is_some() && start == self.line.get(active).and_then(|line| line.start)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timed() -> Lyrics {
        serde_json::from_value(serde_json::json!({
            "synced": true, "offset": 100,
            "line": [
                {"start": 1000, "value": "First"},
                {"start": 2000, "value": "Second"},
                {"start": 2000, "value": "Translation"},
                {"start": 3000, "value": ""}
            ]
        }))
        .unwrap()
    }

    #[test]
    fn follows_timestamps_offsets_seeks_and_instrumental_gaps() {
        let mut lyrics = timed();
        assert_eq!(lyrics.active_line(0.899), None);
        assert_eq!(lyrics.active_line(0.9), Some(0));
        assert_eq!(lyrics.active_line(1.9), Some(2));
        assert!(lyrics.line_is_active(1, Some(2)));
        assert!(lyrics.line_is_active(2, Some(2)));
        assert_eq!(lyrics.active_line(2.9), Some(3));
        assert_eq!(lyrics.active_line(1.), Some(0));
        assert_eq!(lyrics.line_position(1), Some(1.9));
        lyrics.offset = -100;
        assert_eq!(lyrics.active_line(1.), None);
        assert_eq!(lyrics.active_line(1.1), Some(0));
        assert_eq!(lyrics.line_position(0), Some(1.1));
    }

    #[test]
    fn plain_and_missing_timestamps_do_not_invent_timing() {
        let lyrics = Lyrics::plain("First\n\nSecond");
        assert_eq!(lyrics.line.len(), 3);
        assert_eq!(lyrics.active_line(42.), None);
        assert_eq!(lyrics.line_position(0), None);
        assert!(!lyrics.is_empty());
        assert!(Lyrics::plain(" \n").is_empty());
        let mut lyrics = timed();
        lyrics.line[0].start = None;
        assert_eq!(lyrics.active_line(1.), None);
        assert!(!lyrics.line_is_active(0, Some(0)));
        let saved = serde_json::to_string(&lyrics).unwrap();
        assert_eq!(serde_json::from_str::<Lyrics>(&saved).unwrap(), lyrics);
    }
}
