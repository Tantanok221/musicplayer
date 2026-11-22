use serde::{Deserialize, Serialize};

/// Seconds before a timed line starts within which the view arms a wake for
/// it. Longer than the ~200ms between playback updates, so one always lands
/// inside it.
pub const WAKE_LEAD: f64 = 0.4;

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

    /// The first timed line that starts after `position`, with its start in seconds.
    pub fn next_line(&self, position: f64) -> Option<(usize, f64)> {
        (0..self.line.len())
            .filter_map(|index| Some((index, self.line_position(index)?)))
            .filter(|(_, start)| *start > position)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Whether a position-only playback update must redraw lyrics that show
    /// `shown` as the current line and have a wake armed for line `armed`:
    /// the current line moved, or the next one is due before the next update
    /// and nothing will wake the view for it.
    pub fn needs_redraw(&self, position: f64, shown: Option<usize>, armed: Option<usize>) -> bool {
        self.is_timed()
            && (self.active_line(position) != shown
                || self.next_line(position).is_some_and(|(index, start)| {
                    start - position < WAKE_LEAD && armed != Some(index)
                }))
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
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn prop_plain_lyrics_preserve_lines(text in ".{0,256}", position in any::<f64>()) {
            let lyrics = Lyrics::plain(&text);
            prop_assert_eq!(lyrics.line.iter().map(|line| line.value.as_str()).collect::<Vec<_>>(), text.lines().collect::<Vec<_>>());
            prop_assert_eq!(lyrics.active_line(position), None);
            prop_assert_eq!(lyrics.line_position(usize::MAX), None);
            prop_assert!(!lyrics.line_is_active(usize::MAX, Some(usize::MAX)));
            let _ = serde_json::from_str::<Lyrics>(&text);
        }

        #[test]
        fn prop_structured_lyrics_round_trip(entries in proptest::collection::vec((any::<u64>(), ".{0,80}"), 0..24), offset in any::<i64>(), position in any::<f64>()) {
            let lyrics = Lyrics {
                synced: true,
                offset,
                line: entries.into_iter().map(|(start, value)| LyricLine { start: Some(start), value }).collect(),
            };
            let saved = serde_json::to_string(&lyrics).unwrap();
            prop_assert_eq!(&serde_json::from_str::<Lyrics>(&saved).unwrap(), &lyrics);
            let expected = if position.is_finite() {
                lyrics.line.iter().enumerate()
                    .filter(|(_, line)| line.start.unwrap() as f64 <= position.max(0.) * 1000. + offset as f64)
                    .max_by_key(|(_, line)| line.start).map(|(index, _)| index)
            } else { None };
            prop_assert_eq!(lyrics.active_line(position), expected);
            for index in 0..lyrics.line.len() {
                prop_assert!(lyrics.line_position(index).unwrap().is_finite());
            }
        }
    }

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
    fn position_updates_redraw_only_when_the_line_moves_or_needs_a_wake() {
        let lyrics = timed();
        assert_eq!(lyrics.next_line(1.), Some((1, 1.9)));
        assert_eq!(lyrics.next_line(2.9), None);
        // Mid-line and far from the next one: the view is current.
        assert!(!lyrics.needs_redraw(1., Some(0), None));
        // The line moved, forward or back after a seek.
        assert!(lyrics.needs_redraw(2., Some(0), Some(1)));
        assert!(lyrics.needs_redraw(1., Some(2), None));
        // The next line is close: redraw to arm its wake, unless already armed.
        assert!(lyrics.needs_redraw(1.6, Some(0), None));
        assert!(lyrics.needs_redraw(1.6, Some(0), Some(0)));
        assert!(!lyrics.needs_redraw(1.6, Some(0), Some(1)));
        assert!(!Lyrics::plain("First").needs_redraw(1., None, None));
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
