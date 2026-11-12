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
    RecordLabel(String),
}
impl ExplorerFilter {
    pub fn query(&self) -> crate::model::LibraryQuery {
        let mut query = crate::model::LibraryQuery::default();
        match self {
            Self::Genre(genre) => query.selection.genres = vec![genre.clone()],
            Self::Year(year) => query.year = Some(*year),
            Self::RecordLabel(label) => query.record_label = label.trim().into(),
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
    /// The frequency bars along the bottom of Now Playing.
    pub show_spectrum: bool,
    pub left_width: f32,
    pub right_width: f32,
    pub browser_height: f32,
    pub queue_height: f32,
    /// Height of the navigator tree above the artist and genre list.
    pub navigator_height: f32,
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
            show_spectrum: true,
            left_width: 216.,
            right_width: 300.,
            browser_height: 156.,
            queue_height: 280.,
            navigator_height: 236.,
            info_source: InfoSource::Selected,
            info_tab: InfoTab::Details,
        }
    }
}

/// One page of the app: what a tab shows and what Back and Forward return to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Page {
    pub workspace: Workspace,
    /// Artist name and ID when an artist page is open.
    pub artist: Option<(String, String)>,
    /// The music explorer's filters, sort and search.
    pub query: crate::model::LibraryQuery,
    /// Playlist ID and name when a playlist is open.
    pub playlist: Option<(String, String)>,
    /// Search text of the queue or playlists page.
    pub search: String,
}
impl Page {
    pub fn of(workspace: Workspace) -> Self {
        Self {
            workspace,
            ..Default::default()
        }
    }
    pub fn title(&self) -> String {
        if let Some((name, _)) = &self.artist {
            return name.clone();
        }
        match self.workspace {
            Workspace::Explorer if self.query.favorites => "Favorites".into(),
            Workspace::Playlists => self
                .playlist
                .as_ref()
                .map_or_else(|| "Playlists".into(), |(_, name)| name.clone()),
            workspace => workspace.label().into(),
        }
    }
}

/// A top-bar tab. With no page it is an empty "New Tab".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tab {
    pub id: u64,
    pub page: Option<Page>,
    /// Locked tabs cannot be closed, but can still be navigated.
    pub locked: bool,
    /// The page a locked tab was locked on. The tab keeps its name while the
    /// user browses elsewhere in it, and clicking the tab returns here.
    pub home: Option<Page>,
}
impl Tab {
    pub fn title(&self) -> String {
        self.home_page()
            .or(self.page.as_ref())
            .map_or_else(|| "New Tab".into(), Page::title)
    }
    /// Where a locked tab returns when clicked; unlocked tabs have no home.
    pub fn home_page(&self) -> Option<&Page> {
        self.locked.then_some(()).and(self.home.as_ref())
    }
}

/// The user's tabs, kept in preferences so they survive restarts and new builds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tabs {
    pub tabs: Vec<Tab>,
    pub active: usize,
}
impl Default for Tabs {
    fn default() -> Self {
        Self {
            tabs: Workspace::ALL
                .into_iter()
                .zip(1..)
                .map(|(workspace, id)| Tab {
                    id,
                    page: Some(Page::of(workspace)),
                    ..Default::default()
                })
                .collect(),
            active: 0,
        }
    }
}
impl Tabs {
    /// Repairs a saved list: never empty, unique IDs, and a valid active tab.
    pub fn normalized(mut self) -> Self {
        if self.tabs.is_empty() {
            self.tabs.push(Tab {
                id: 1,
                ..Default::default()
            });
        }
        let mut seen = std::collections::HashSet::new();
        let mut next = self.tabs.iter().map(|tab| tab.id).max().unwrap_or(0) + 1;
        for tab in &mut self.tabs {
            if tab.id == 0 || !seen.insert(tab.id) {
                tab.id = next;
                seen.insert(next);
                next += 1;
            }
            // Tabs locked before homes existed keep the page they show.
            if !tab.locked {
                tab.home = None;
            } else if tab.home.is_none() {
                tab.home = tab.page.clone();
            }
        }
        self.active = self.active.min(self.tabs.len() - 1);
        self
    }
    pub fn active(&self) -> &Tab {
        &self.tabs[self.active.min(self.tabs.len() - 1)]
    }
    pub fn active_mut(&mut self) -> &mut Tab {
        let ix = self.active.min(self.tabs.len() - 1);
        &mut self.tabs[ix]
    }
    fn next_id(&self) -> u64 {
        self.tabs.iter().map(|tab| tab.id).max().unwrap_or(0) + 1
    }
    /// Adds an empty tab after the active one and makes it active.
    pub fn new_tab(&mut self) -> u64 {
        let id = self.next_id();
        let at = (self.active + 1).min(self.tabs.len());
        self.tabs.insert(
            at,
            Tab {
                id,
                ..Default::default()
            },
        );
        self.active = at;
        id
    }
    /// Closes an unlocked tab. Closing the active tab activates its right
    /// neighbour, or the left one at the end; the last tab becomes empty.
    pub fn close(&mut self, ix: usize) -> bool {
        if self.tabs.get(ix).is_none_or(|tab| tab.locked) {
            return false;
        }
        if self.tabs.len() == 1 {
            let id = self.tabs[0].id;
            self.tabs[0] = Tab {
                id,
                ..Default::default()
            };
            return true;
        }
        self.tabs.remove(ix);
        if ix < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
    }
    /// Puts a closed tab back where it was and makes it active. A lone empty
    /// tab, as closing the last tab leaves, is replaced rather than kept.
    /// Returns the tab's ID, which changes if another tab has taken it.
    pub fn reopen(&mut self, ix: usize, mut tab: Tab) -> u64 {
        if self.tabs.len() == 1 && self.tabs[0].page.is_none() && !self.tabs[0].locked {
            self.tabs[0] = tab;
            self.active = 0;
            return self.tabs[0].id;
        }
        if tab.id == 0 || self.tabs.iter().any(|other| other.id == tab.id) {
            tab.id = self.next_id();
        }
        let ix = ix.min(self.tabs.len());
        let id = tab.id;
        self.tabs.insert(ix, tab);
        self.active = ix;
        id
    }
    /// Moves a tab, keeping the same tab active.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self
            .tabs
            .iter()
            .position(|tab| tab.id == active)
            .unwrap_or(0);
    }
    pub fn toggle_lock(&mut self, ix: usize) {
        if let Some(tab) = self.tabs.get_mut(ix) {
            tab.locked = !tab.locked;
            tab.home = if tab.locked { tab.page.clone() } else { None };
        }
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_tabs_are_the_four_workspaces() {
        let tabs = Tabs::default();
        let titles: Vec<_> = tabs.tabs.iter().map(Tab::title).collect();
        assert_eq!(
            titles,
            ["Music Explorer", "Queue", "Playlists", "Now Playing"]
        );
        assert_eq!(tabs.active, 0);
        assert!(tabs.tabs.iter().all(|tab| !tab.locked));
    }
    #[test]
    fn closing_respects_locks_neighbours_and_the_last_tab() {
        let mut tabs = Tabs {
            active: 1,
            ..Default::default()
        };
        tabs.toggle_lock(1);
        assert!(!tabs.close(1), "a locked tab stays");
        tabs.toggle_lock(1);
        assert!(tabs.close(1));
        assert_eq!(
            tabs.active().title(),
            "Playlists",
            "the right neighbour takes over"
        );
        tabs.active = 2;
        assert!(tabs.close(2));
        assert_eq!(
            tabs.active().title(),
            "Playlists",
            "at the end the left one does"
        );
        assert!(tabs.close(0));
        assert_eq!(tabs.active, 0);
        assert!(tabs.close(0));
        assert_eq!(tabs.tabs.len(), 1);
        assert!(tabs.tabs[0].page.is_none(), "the last tab becomes empty");
    }
    #[test]
    fn new_tabs_open_empty_after_the_active_one() {
        let mut tabs = Tabs::default();
        let id = tabs.new_tab();
        assert_eq!(tabs.active, 1);
        assert_eq!(tabs.active().id, id);
        assert_eq!(tabs.active().title(), "New Tab");
        assert_eq!(tabs.tabs.len(), 5);
    }
    #[test]
    fn moving_keeps_the_active_tab_and_reset_restores_defaults() {
        let mut tabs = Tabs {
            active: 2,
            ..Default::default()
        };
        tabs.move_tab(2, 0);
        assert_eq!(tabs.active, 0);
        assert_eq!(tabs.active().title(), "Playlists");
        tabs.move_tab(3, 1);
        assert_eq!(tabs.active().title(), "Playlists");
        tabs.toggle_lock(0);
        tabs.reset();
        assert_eq!(tabs, Tabs::default());
    }
    #[test]
    fn page_titles_name_what_the_tab_shows() {
        let artist = Page {
            artist: Some(("ONE OK ROCK".into(), "id".into())),
            ..Page::of(Workspace::Explorer)
        };
        assert_eq!(artist.title(), "ONE OK ROCK");
        let mut favorites = Page::of(Workspace::Explorer);
        favorites.query.favorites = true;
        assert_eq!(favorites.title(), "Favorites");
        let playlist = Page {
            playlist: Some(("p".into(), "Road trip".into())),
            ..Page::of(Workspace::Playlists)
        };
        assert_eq!(playlist.title(), "Road trip");
        assert_eq!(Page::of(Workspace::Queue).title(), "Queue");
    }
    #[test]
    fn saved_tabs_round_trip_and_repair() {
        let mut tabs = Tabs::default();
        tabs.toggle_lock(2);
        tabs.active = 3;
        let json = serde_json::to_string(&tabs).unwrap();
        assert_eq!(serde_json::from_str::<Tabs>(&json).unwrap(), tabs);
        let broken = Tabs {
            tabs: vec![Tab::default(), Tab::default()],
            active: 9,
        }
        .normalized();
        assert_eq!(broken.active, 1);
        assert_ne!(broken.tabs[0].id, broken.tabs[1].id);
        assert!(
            Tabs {
                tabs: vec![],
                active: 0
            }
            .normalized()
            .tabs
            .len()
                == 1
        );
    }
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
    #[test]
    fn a_locked_tab_keeps_its_home_while_browsing() {
        let mut tabs = Tabs::default();
        tabs.toggle_lock(0);
        tabs.tabs[0].page = Some(Page {
            artist: Some(("Dove Cameron".into(), "a1".into())),
            ..Page::of(Workspace::Explorer)
        });
        assert_eq!(tabs.tabs[0].title(), "Music Explorer");
        assert_eq!(
            tabs.tabs[0].home_page(),
            Some(&Page::of(Workspace::Explorer))
        );
        tabs.toggle_lock(0);
        assert_eq!(tabs.tabs[0].title(), "Dove Cameron");
        assert_eq!(tabs.tabs[0].home_page(), None);
    }
    #[test]
    fn reopening_puts_a_tab_back_in_place() {
        let mut tabs = Tabs::default();
        let closed = tabs.tabs[1].clone();
        assert!(tabs.close(1));
        let id = tabs.reopen(1, closed.clone());
        assert_eq!(id, closed.id);
        assert_eq!((tabs.active, tabs.tabs[1].title()), (1, "Queue".into()));
        // A clashing ID is replaced, and the lone empty tab gives way.
        let mut lone = Tabs::default();
        for _ in 0..4 {
            lone.close(0);
        }
        assert_eq!(lone.tabs.len(), 1);
        lone.reopen(0, closed);
        assert_eq!(lone.tabs.len(), 1);
        assert_eq!(lone.tabs[0].title(), "Queue");
    }
}
