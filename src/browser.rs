//! MusicBee-style browsing: selecting a value narrows the library without playing it.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct BrowserSelection {
    pub genres: Vec<String>,
    pub artists: Vec<String>,
    pub albums: Vec<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserColumn {
    Genre,
    Artist,
    Album,
}
impl BrowserSelection {
    pub fn select(&mut self, column: BrowserColumn, value: Option<String>, multiple: bool) {
        let values = match column {
            BrowserColumn::Genre => {
                self.artists.clear();
                self.albums.clear();
                &mut self.genres
            }
            BrowserColumn::Artist => {
                self.albums.clear();
                &mut self.artists
            }
            BrowserColumn::Album => &mut self.albums,
        };
        match value {
            None => values.clear(),
            Some(value) if multiple => {
                if let Some(index) = values.iter().position(|v| v == &value) {
                    values.remove(index);
                } else {
                    values.push(value);
                }
            }
            Some(value) => {
                values.clear();
                values.push(value);
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty() && self.artists.is_empty() && self.albums.is_empty()
    }
}
#[derive(Clone, Debug, Default)]
pub struct Facet {
    pub id: String,
    pub label: String,
    pub count: usize,
    pub artwork: String,
    pub detail: String,
}
#[derive(Clone, Debug, Default)]
pub struct BrowserView {
    pub genres: Vec<Facet>,
    pub artists: Vec<Facet>,
    pub albums: Vec<Facet>,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Music,
    NowPlaying,
    Playlists,
    Explorer,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MainView {
    #[default]
    AlbumsAndTracks,
    AlbumCovers,
    Tracks,
}
impl MainView {
    pub fn label(self) -> &'static str {
        match self {
            Self::AlbumsAndTracks => "Album and Tracks",
            Self::AlbumCovers => "Album Covers",
            Self::Tracks => "Tracks",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum InfoSource {
    #[default]
    Selected,
    Playing,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum InfoTab {
    #[default]
    Details,
    Lyrics,
    Artwork,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutPreferences {
    pub music_view: MainView,
    pub explorer_view: MainView,
    pub show_navigator: bool,
    pub show_information: bool,
    pub music_browser: bool,
    pub explorer_browser: bool,
    pub left_width: f32,
    pub right_width: f32,
    pub browser_height: f32,
    pub queue_height: f32,
    pub info_source: InfoSource,
    pub info_tab: InfoTab,
}
impl Default for LayoutPreferences {
    fn default() -> Self {
        Self {
            music_view: MainView::AlbumsAndTracks,
            explorer_view: MainView::AlbumCovers,
            show_navigator: true,
            show_information: true,
            music_browser: false,
            explorer_browser: true,
            left_width: 216.,
            right_width: 300.,
            browser_height: 156.,
            queue_height: 280.,
            info_source: InfoSource::Selected,
            info_tab: InfoTab::Details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn upstream_changes_reset_only_downstream_columns() {
        let mut selected = BrowserSelection {
            genres: vec!["Jazz".into()],
            artists: vec!["Artist".into()],
            albums: vec!["album".into()],
        };
        selected.select(BrowserColumn::Artist, Some("Other".into()), false);
        assert_eq!(selected.genres, ["Jazz"]);
        assert_eq!(selected.artists, ["Other"]);
        assert!(selected.albums.is_empty());
        selected.select(BrowserColumn::Genre, Some("Ambient".into()), false);
        assert_eq!(selected.genres, ["Ambient"]);
        assert!(selected.artists.is_empty());
        assert!(selected.albums.is_empty());
    }
    #[test]
    fn modifier_selection_is_a_union_and_all_clears_the_column() {
        let mut selected = BrowserSelection::default();
        selected.select(BrowserColumn::Genre, Some("Jazz".into()), false);
        selected.select(BrowserColumn::Genre, Some("Ambient".into()), true);
        assert_eq!(selected.genres.len(), 2);
        selected.select(BrowserColumn::Genre, Some("Jazz".into()), true);
        assert_eq!(selected.genres, ["Ambient"]);
        selected.select(BrowserColumn::Genre, None, false);
        assert!(selected.is_empty());
    }
}
