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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExplorerFilter {
    Genre(String),
    Year(i32),
}
impl ExplorerFilter {
    pub fn query(&self) -> crate::model::LibraryQuery {
        let mut query = crate::model::LibraryQuery::default();
        match self {
            Self::Genre(genre) => query.selection.genres = vec![genre.clone()],
            Self::Year(year) => query.year = Some(*year),
        }
        query
    }
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
pub enum NavigatorMode {
    Artist,
    #[default]
    AlbumArtist,
    Year,
    Genre,
}
impl NavigatorMode {
    pub const ALL: [Self; 4] = [Self::Artist, Self::AlbumArtist, Self::Year, Self::Genre];
    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::AlbumArtist => "Album Artist",
            Self::Year => "Year",
            Self::Genre => "Genre",
        }
    }
    pub fn all_label(self) -> &'static str {
        match self {
            Self::Artist => "All Artists",
            Self::AlbumArtist => "All Album Artists",
            Self::Year => "All Years",
            Self::Genre => "All Genres",
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct NavigatorView {
    pub artists: Vec<Facet>,
    pub album_artists: Vec<Facet>,
    pub years: Vec<Facet>,
    pub genres: Vec<Facet>,
}
impl NavigatorView {
    pub fn values(&self, mode: NavigatorMode) -> &[Facet] {
        match mode {
            NavigatorMode::Artist => &self.artists,
            NavigatorMode::AlbumArtist => &self.album_artists,
            NavigatorMode::Year => &self.years,
            NavigatorMode::Genre => &self.genres,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Explorer,
    Queue,
    Playlists,
    NowPlaying,
}
impl Workspace {
    pub const ALL: [Self; 4] = [
        Self::Explorer,
        Self::Queue,
        Self::Playlists,
        Self::NowPlaying,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Explorer => "Music Explorer",
            Self::Queue => "Queue",
            Self::Playlists => "Playlists",
            Self::NowPlaying => "Now Playing",
        }
    }
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

pub fn cover_layout(pane_width: f32) -> (usize, f32) {
    let available = (pane_width - 24.).max(0.);
    let columns = ((available + 16.) / (128. + 16.)).floor().max(1.) as usize;
    let side = ((available - (columns - 1) as f32 * 16.) / columns as f32).clamp(0., 150.);
    (columns, side)
}

/// A jump starts a fresh row at its target, keeping every preceding album accessible.
pub struct CoverRows {
    columns: usize,
    sections: Vec<(usize, usize, usize)>,
    rows: usize,
}
impl CoverRows {
    pub fn new(albums: usize, columns: usize, jump: Option<usize>) -> Self {
        Self::grouped(albums, columns, jump, &[])
    }
    pub fn grouped(albums: usize, columns: usize, jump: Option<usize>, groups: &[usize]) -> Self {
        let columns = columns.max(1);
        let mut starts = vec![0];
        starts.extend(groups.iter().copied().filter(|i| *i < albums));
        starts.extend(jump.filter(|i| *i < albums));
        starts.push(albums);
        starts.sort_unstable();
        starts.dedup();
        let mut sections = Vec::new();
        let mut rows = 0;
        for range in starts.windows(2) {
            sections.push((range[0], range[1], rows));
            rows += (range[1] - range[0]).div_ceil(columns);
        }
        Self {
            columns,
            sections,
            rows,
        }
    }
    pub fn row_count(&self) -> usize {
        self.rows
    }
    pub fn row_for_album(&self, index: usize) -> usize {
        self.sections
            .iter()
            .find(|(start, end, _)| *start <= index && index < *end)
            .map_or(0, |(start, _, row)| row + (index - start) / self.columns)
    }
    pub fn albums_in_row(&self, row: usize) -> std::ops::Range<usize> {
        let section = self
            .sections
            .iter()
            .rev()
            .find(|(_, _, offset)| *offset <= row);
        section.map_or(0..0, |(start, end, offset)| {
            let first = start + (row - offset) * self.columns;
            first..(first + self.columns).min(*end)
        })
    }
}

pub fn panel_widths(window_width: f32, left: Option<f32>, right: Option<f32>) -> (f32, f32) {
    let left_min = if left.is_some() { 140. } else { 0. };
    let right_min = if right.is_some() { 180. } else { 0. };
    let left_extra = left.map_or(0., |width| width.clamp(140., 380.) - left_min);
    let right_extra = right.map_or(0., |width| width.clamp(180., 500.) - right_min);
    let available = (window_width - 320. - left_min - right_min - 4.).max(0.);
    let scale = if left_extra + right_extra > 0. {
        (available / (left_extra + right_extra)).min(1.)
    } else {
        1.
    };
    (
        left_min + left_extra * scale,
        right_min + right_extra * scale,
    )
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutPreferences {
    pub navigator_mode: NavigatorMode,
    pub explorer_view: MainView,
    pub show_navigator: bool,
    /// Float the navigator over the library only while the pointer is at the left edge.
    pub navigator_auto_hide: bool,
    pub show_information: bool,
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
            navigator_mode: NavigatorMode::default(),
            explorer_view: MainView::AlbumCovers,
            show_navigator: true,
            navigator_auto_hide: false,
            show_information: true,
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
