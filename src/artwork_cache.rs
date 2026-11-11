//! Decoded artwork has a separate budget from the on-disk download cache.
use gpui_kit::*;
use std::{collections::HashMap, sync::Arc};

/// Lists and grids use small copies (64-400 KB decoded), so this still holds ~80 grid covers.
const BYTE_BUDGET: usize = 32 * 1024 * 1024;
/// Mostly small list and grid copies now, so more fit within the byte budget.
const ENTRY_BUDGET: usize = 256;
struct Entry {
    item: ImageCacheItem,
    frame: u64,
}
#[derive(Default)]
pub struct ArtworkCache {
    entries: HashMap<u64, Entry>,
    frame: u64,
}
impl ArtworkCache {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self::default());
        cx.observe_release(&cache, |cache, cx| {
            for (_, entry) in cache.entries.drain() {
                if let Some(Ok(image)) = entry.item.get() {
                    cx.drop_image(image, None);
                }
            }
        })
        .detach();
        cache
    }
    pub fn begin_frame(&mut self, window: &mut Window, cx: &mut App) {
        let entries = self
            .entries
            .iter()
            .map(|(key, entry)| (*key, entry.frame, entry_bytes(entry)))
            .collect();
        for key in eviction_keys(entries, self.frame) {
            if let Some(entry) = self.entries.remove(&key)
                && let Some(Ok(image)) = entry.item.get()
            {
                cx.drop_image(image, Some(window));
            }
        }
        // Keep the preceding frame's visible images, even when a very large window
        // exceeds the soft budget. Evicting visible art causes reload/redraw loops.
        self.frame += 1;
    }
}
fn eviction_keys(mut entries: Vec<(u64, u64, usize)>, visible_frame: u64) -> Vec<u64> {
    let mut bytes = entries.iter().map(|(_, _, bytes)| bytes).sum::<usize>();
    let mut count = entries.len();
    entries.sort_unstable_by_key(|(_, frame, _)| *frame);
    let mut evicted = Vec::new();
    for (key, frame, size) in entries {
        if !over_budget(bytes, count) || frame >= visible_frame {
            break;
        }
        evicted.push(key);
        bytes = bytes.saturating_sub(size);
        count -= 1;
    }
    evicted
}
fn over_budget(bytes: usize, count: usize) -> bool {
    bytes > BYTE_BUDGET || count > ENTRY_BUDGET
}
fn entry_bytes(entry: &Entry) -> usize {
    entry.item.get().and_then(Result::ok).map_or(0, |image| {
        (0..image.frame_count())
            .filter_map(|frame| image.as_bytes(frame))
            .map(<[u8]>::len)
            .sum()
    })
}
impl ImageCache for ArtworkCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let entry = self.entries.entry(hash(resource)).or_insert_with(|| Entry {
            item: ImageCacheItem::new(resource, cx),
            frame: self.frame,
        });
        entry.frame = self.frame;
        entry.item.use_image(window)
    }
}

#[cfg(test)]
mod tests {
    use super::{BYTE_BUDGET, eviction_keys};
    #[test]
    fn evicts_old_art_by_bytes_but_keeps_visible_art_to_prevent_reload_loops() {
        assert_eq!(eviction_keys(vec![(1, 1, BYTE_BUDGET), (2, 2, 4)], 2), [1]);
        assert!(eviction_keys(vec![(1, 2, BYTE_BUDGET + 1)], 2).is_empty());
        assert!(eviction_keys(vec![(1, 1, 1000)], 2).is_empty());
    }
    #[test]
    fn pending_loads_are_bounded_even_without_decoded_bytes() {
        let entries = (0..258).map(|i| (i, i, 0)).collect();
        assert_eq!(eviction_keys(entries, 257), [0, 1]);
    }
}
