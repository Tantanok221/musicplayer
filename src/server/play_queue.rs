use super::Server;
use crate::{model::SavedQueue, sync::milliseconds};
use anyhow::Result;
use serde::Deserialize;

#[derive(Clone)]
pub struct RemotePlayQueue {
    pub changed_by: String,
    pub modified_ms: i64,
    pub queue: SavedQueue,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexedQueue {
    changed_by: String,
    changed: chrono::DateTime<chrono::FixedOffset>,
    current_index: Option<usize>,
    #[serde(default)]
    position: u64,
    #[serde(default)]
    entry: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
}

impl Server {
    pub(crate) async fn remote_play_queue(&self) -> Result<Option<RemotePlayQueue>> {
        if !self.extensions.contains("indexBasedQueue") {
            return Ok(None);
        }
        let body = self.request("getPlayQueueByIndex", &[]).await?;
        let Some(value) = body
            .get("playQueueByIndex")
            .filter(|value| !value.is_null())
        else {
            return Ok(None);
        };
        let result = IndexedQueue::deserialize(value)?;
        let Some(index) = result
            .current_index
            .filter(|index| *index < result.entry.len())
        else {
            return Ok(None);
        };
        Ok(Some(RemotePlayQueue {
            changed_by: result.changed_by,
            modified_ms: result.changed.timestamp_millis(),
            queue: SavedQueue {
                ids: result.entry.into_iter().map(|entry| entry.id).collect(),
                index: Some(index),
                position: result.position as f64 / 1000.,
                ..Default::default()
            },
        }))
    }

    pub(crate) async fn save_play_queue(&self, queue: &SavedQueue) -> Result<()> {
        let mut params: Vec<_> = queue.ids.iter().cloned().map(|id| ("id", id)).collect();
        params.push(("position", milliseconds(queue.position).to_string()));
        let index =
            (!queue.ids.is_empty()).then(|| queue.index.unwrap_or(0).min(queue.ids.len() - 1));
        let endpoint = if self.extensions.contains("indexBasedQueue") {
            if let Some(index) = index {
                params.push(("currentIndex", index.to_string()));
            }
            "savePlayQueueByIndex"
        } else {
            if let Some(index) = index {
                params.push(("current", queue.ids[index].clone()));
            }
            "savePlayQueue"
        };
        self.request(endpoint, &params).await?;
        Ok(())
    }
}
