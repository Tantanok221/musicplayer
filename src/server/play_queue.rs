use super::Server;
use crate::{model::SavedQueue, sync::milliseconds};
use anyhow::Result;

impl Server {
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
