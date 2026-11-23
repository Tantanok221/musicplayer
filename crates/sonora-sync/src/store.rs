use rusqlite::{Connection, OptionalExtension, params};
use sonora_sync_protocol::{Device, PlaybackState};
use std::{path::Path, time::Duration};

#[derive(thiserror::Error, Debug)]
pub enum StoreError {
    #[error("Could not access relay database")]
    Database(#[from] rusqlite::Error),
    #[error("Could not read or encode relay playback state")]
    Json(#[from] serde_json::Error),
}

pub(crate) struct Store(Connection);

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS devices (
                username TEXT NOT NULL,
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                os TEXT NOT NULL,
                last_seen INTEGER NOT NULL,
                PRIMARY KEY (username, id)
            );
            CREATE TABLE IF NOT EXISTS playback (
                username TEXT PRIMARY KEY NOT NULL,
                data TEXT NOT NULL
            );",
        )?;
        Ok(Self(db))
    }

    pub fn devices(&self, user: &str) -> Result<Vec<Device>, StoreError> {
        let mut query = self.0.prepare_cached(
            "SELECT id, name, os, last_seen FROM devices WHERE username = ?1 ORDER BY id",
        )?;
        let rows = query.query_map([user], |row| {
            Ok(Device {
                id: row.get(0)?,
                name: row.get(1)?,
                os: row.get(2)?,
                last_seen: row.get(3)?,
                online: false,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn save_device(&self, user: &str, device: &Device) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO devices VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(username, id) DO UPDATE SET
             name = excluded.name, os = excluded.os, last_seen = excluded.last_seen",
            params![user, device.id, device.name, device.os, device.last_seen],
        )?;
        Ok(())
    }

    pub fn playback(&self, user: &str) -> Result<Option<PlaybackState>, StoreError> {
        let stored: Option<String> = self
            .0
            .query_row(
                "SELECT data FROM playback WHERE username = ?1",
                [user],
                |row| row.get(0),
            )
            .optional()?;
        stored
            .map(|state| serde_json::from_str(&state))
            .transpose()
            .map_err(Into::into)
    }

    pub fn save_playback(&self, user: &str, state: &PlaybackState) -> Result<(), StoreError> {
        self.0.execute(
            "INSERT INTO playback VALUES (?1, ?2)
             ON CONFLICT(username) DO UPDATE SET data = excluded.data",
            params![user, serde_json::to_string(state)?],
        )?;
        Ok(())
    }
}
