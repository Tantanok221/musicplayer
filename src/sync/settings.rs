use crate::model::Preferences;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sonora_sync_protocol::VersionedSettings;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedPreferences(pub(crate) Map<String, Value>);

impl SharedPreferences {
    pub fn from_preferences(prefs: &Preferences) -> Self {
        Self(Map::from_iter([
            ("dark".into(), json!(prefs.dark)),
            ("gain".into(), json!(prefs.gain)),
            ("sorting".into(), json!(prefs.sorting)),
            ("playlist_columns".into(), json!(prefs.playlist_columns)),
            (
                "all_missing_releases".into(),
                json!(prefs.all_missing_releases),
            ),
            ("favorites".into(), json!(prefs.favorites)),
        ]))
    }

    pub fn apply(&self, prefs: &mut Preferences) -> serde_json::Result<()> {
        fn assign<T: DeserializeOwned>(
            fields: &Map<String, Value>,
            key: &str,
            field: &mut T,
        ) -> serde_json::Result<()> {
            if let Some(value) = fields.get(key) {
                *field = T::deserialize(value)?;
            }
            Ok(())
        }
        assign(&self.0, "dark", &mut prefs.dark)?;
        assign(&self.0, "gain", &mut prefs.gain)?;
        assign(&self.0, "sorting", &mut prefs.sorting)?;
        assign(&self.0, "playlist_columns", &mut prefs.playlist_columns)?;
        assign(
            &self.0,
            "all_missing_releases",
            &mut prefs.all_missing_releases,
        )?;
        assign(&self.0, "favorites", &mut prefs.favorites)?;
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct SettingsState {
    pub remote: VersionedSettings,
    pending: Map<String, Value>,
}

impl SettingsState {
    pub fn record(&mut self, before: &Preferences, after: &Preferences) {
        let before = SharedPreferences::from_preferences(before);
        let after = SharedPreferences::from_preferences(after);
        self.pending.extend(
            after
                .0
                .into_iter()
                .filter(|(key, value)| before.0.get(key) != Some(value)),
        );
    }

    pub fn receive(&mut self, remote: VersionedSettings) {
        self.pending
            .retain(|key, value| remote.settings.get(key) != Some(value));
        self.remote = remote;
    }

    pub fn merged(&self) -> SharedPreferences {
        let mut settings = self.remote.settings.clone();
        settings.extend(self.pending.clone());
        SharedPreferences(settings)
    }

    pub fn draft(&self) -> Option<VersionedSettings> {
        (!self.pending.is_empty()).then(|| VersionedSettings {
            settings: self.merged().0,
            version: self.remote.version,
        })
    }
}
