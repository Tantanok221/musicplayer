use gpui_kit::assets::IconName as Glyph;
use gpui_kit::base::{Transition, transition};
use gpui_kit::component::{
    button::*,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenuItem},
    resizable::{h_resizable, resizable_panel, v_resizable},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    *,
};
use gpui_kit::{prelude::FluentBuilder, *};
use sonora::{
    browser::*,
    catalog::LibraryView,
    lyrics::Lyrics,
    model::*,
    platform::Account,
    playback::PlayerCommand,
    service::{Command, Event, Mutation, Service},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

actions!(
    sonora,
    [
        Quit,
        TogglePlayback,
        NextTrack,
        PreviousTrack,
        OpenSettings,
        FocusSearch,
        GoBack,
        Refresh,
        ToggleNavigator,
        ToggleInformation,
        ToggleBrowser,
        RestoreLayout,
        PlayingWorkspace,
        QueueWorkspace,
        PlaylistsWorkspace,
        ExplorerWorkspace
    ]
);

#[derive(Clone, Copy)]
struct Ink {
    background: Hsla,
    panel: Hsla,
    line: Hsla,
    text: Hsla,
    muted: Hsla,
    accent: Hsla,
    selection: Hsla,
}
fn ink(cx: &App) -> Ink {
    if cx.theme().is_dark() {
        Ink {
            background: rgb(0x1b1c20).into(),
            panel: rgb(0x202125).into(),
            line: rgb(0x34353a).into(),
            text: rgb(0xd9dadd).into(),
            muted: rgb(0x92949c).into(),
            accent: rgb(0xe3a34c).into(),
            selection: rgb(0x38342e).into(),
        }
    } else {
        Ink {
            background: rgb(0xfafafa).into(),
            panel: rgb(0xf0f0f2).into(),
            line: rgb(0xd8d9dc).into(),
            text: rgb(0x27282d).into(),
            muted: rgb(0x777980).into(),
            accent: rgb(0xa76312).into(),
            selection: rgb(0xefe2cf).into(),
        }
    }
}
fn short_button(id: impl Into<ElementId>, icon: Glyph, label: impl Into<SharedString>) -> Button {
    let label = label.into();
    Button::new(id)
        .ghost()
        .compact()
        .size(px(24.))
        .icon(Icon::new(icon).size_3())
        .tooltip(label.clone())
        .accessibility_label(label)
}
fn panel_title(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .h(px(27.))
        .px_2()
        .flex()
        .items_center()
        .flex_shrink_0()
        .border_b_1()
        .border_color(ink(cx).line)
        .text_size(px(11.))
        .text_color(ink(cx).muted)
        .child(text.into())
}
fn technical(track: &Track) -> String {
    [
        track.suffix.to_uppercase(),
        track
            .bit_depth
            .map(|v| format!("{v} bit"))
            .unwrap_or_default(),
        track
            .sampling_rate
            .map(|v| format!("{:.1} kHz", v as f64 / 1000.))
            .unwrap_or_default(),
        track
            .bit_rate
            .map(|v| format!("{v} kbps"))
            .unwrap_or_default(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ")
}

pub struct MusicApp {
    service: Service,
    workspace: Workspace,
    view: LibraryView,
    query: LibraryQuery,
    explorer_query: LibraryQuery,
    request: u64,
    search: Entity<InputState>,
    search_text: String,
    playlist_search: String,
    queue_search: String,
    titlebar_dragging: bool,
    workspace_scroll: ScrollHandle,
    server_url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    playlist_name: Entity<InputState>,
    seek: Entity<SliderState>,
    volume: Entity<SliderState>,
    preferences: Preferences,
    account: Option<Account>,
    settings: bool,
    tracks: Vec<Arc<Track>>,
    track_total: usize,
    loading_more: bool,
    group_tracks: HashMap<String, Vec<Arc<Track>>>,
    group_requested: HashSet<String>,
    album_list: ListState,
    cover_list: ListState,
    cover_columns: usize,
    alphabet_jump: Option<usize>,
    library_width: f32,
    library_height: f32,
    selected_album: Option<Arc<Album>>,
    selected: Option<Arc<Track>>,
    playlists: Vec<Playlist>,
    playlist: Option<Playlist>,
    playback: PlaybackSnapshot,
    artwork: HashMap<String, PathBuf>,
    art_requested: HashSet<String>,
    art_failures: HashSet<String>,
    lyrics: HashMap<String, Result<Arc<Lyrics>, String>>,
    lyrics_requested: HashSet<String>,
    lyrics_scroll: ScrollHandle,
    lyrics_follow: Option<(String, Option<usize>)>,
    lyrics_scroll_motion: Option<(Instant, f32, f32)>,
    artist_name: Option<String>,
    artist_albums: Option<Vec<Arc<Album>>>,
    artist_origin: Workspace,
    highlight_album: Option<(String, Vec<Arc<Track>>)>,
    highlight_requested: Option<String>,
    library_open: bool,
    playlists_open: bool,
    show_playlist_editor: bool,
    playlist_editor_rename: bool,
    playlist_editor_queue: bool,
    pending_delete: Option<String>,
    status: String,
    error: Option<String>,
    pending_mutation: Option<Mutation>,
    focus: FocusHandle,
    artist_scroll: UniformListScrollHandle,
    track_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}
impl MusicApp {
    pub fn new(
        service: Service,
        events: flume::Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search music"));
        let server_url = cx
            .new(|cx| InputState::new(window, cx).placeholder("http://your-navidrome-server:4533"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let playlist_name = cx.new(|cx| InputState::new(window, cx).placeholder("Playlist name"));
        let seek = cx.new(|_| SliderState::new().min(0.).max(1000.).step(1.));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(0.7)
        });
        let subscriptions = vec![
            cx.subscribe_in(&search, window, |app, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = app.search.read(cx).value().to_string();
                    if value != app.search_text {
                        app.search_text = value.clone();
                        if app.artist_name.is_some() {
                            cx.notify();
                            return;
                        }
                        match app.workspace {
                            Workspace::Explorer => {
                                app.query.text = value;
                                app.query.selection = Default::default();
                                app.request_query(cx);
                            }
                            Workspace::Playlists => app.playlist_search = value,
                            Workspace::Queue => app.queue_search = value,
                            Workspace::NowPlaying => {
                                app.workspace = Workspace::Explorer;
                                app.query = app.explorer_query.clone();
                                app.query.text = value;
                                app.query.selection = Default::default();
                                app.request_query(cx);
                            }
                        }
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&seek, window, |app, _, event: &SliderEvent, _, _| {
                if let SliderEvent::Release(SliderValue::Single(value)) = event {
                    app.service.player.send(PlayerCommand::Seek(
                        *value as f64 / 1000. * app.playback.duration,
                    ));
                }
            }),
            cx.subscribe_in(&volume, window, |app, _, event: &SliderEvent, _, _| {
                if let SliderEvent::Change(SliderValue::Single(value)) = event {
                    app.preferences.volume = *value;
                    app.save_preferences();
                }
            }),
        ];
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                if this
                    .update_in(cx, |app, window, cx| app.event(event, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        service.send(Command::Initialize);
        let query = LibraryQuery::default();
        let request = service.query(query.clone());
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            service,
            workspace: Workspace::Explorer,
            view: LibraryView::default(),
            query: query.clone(),
            explorer_query: query,
            request,
            search,
            search_text: String::new(),
            playlist_search: String::new(),
            queue_search: String::new(),
            titlebar_dragging: false,
            workspace_scroll: ScrollHandle::new(),
            server_url,
            username,
            password,
            playlist_name,
            seek,
            volume,
            preferences: Preferences::default(),
            account: None,
            settings: false,
            tracks: vec![],
            track_total: 0,
            loading_more: false,
            group_tracks: HashMap::new(),
            group_requested: HashSet::new(),
            album_list: ListState::new(0, ListAlignment::Top, px(300.)),
            cover_list: ListState::new(0, ListAlignment::Top, px(200.)),
            cover_columns: 4,
            alphabet_jump: None,
            library_height: 600.,
            library_width: 600.,
            selected_album: None,
            selected: None,
            playlists: vec![],
            playlist: None,
            playback: PlaybackSnapshot::default(),
            artwork: HashMap::new(),
            art_requested: HashSet::new(),
            art_failures: HashSet::new(),
            lyrics: HashMap::new(),
            lyrics_requested: HashSet::new(),
            lyrics_scroll: ScrollHandle::new(),
            lyrics_follow: None,
            lyrics_scroll_motion: None,
            artist_name: None,
            artist_albums: None,
            artist_origin: Workspace::Explorer,
            highlight_album: None,
            highlight_requested: None,
            library_open: true,
            playlists_open: true,
            show_playlist_editor: false,
            playlist_editor_rename: false,
            playlist_editor_queue: false,
            pending_delete: None,
            status: "Opening library…".into(),
            error: None,
            pending_mutation: None,
            focus,
            artist_scroll: UniformListScrollHandle::default(),
            track_scroll: UniformListScrollHandle::default(),
            _subscriptions: subscriptions,
        }
    }
    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Ready(account, prefs) => {
                self.account = account.clone();
                self.settings = account.is_none();
                self.preferences = prefs;
                self.apply_theme(window, cx);
                self.volume.update(cx, |slider, cx| {
                    slider.set_value(self.preferences.volume, window, cx)
                });
                if let Some(account) = account {
                    self.server_url
                        .update(cx, |input, cx| input.set_value(account.url, window, cx));
                    self.username.update(cx, |input, cx| {
                        input.set_value(account.username, window, cx)
                    });
                }
            }
            Event::SignInRequired(message) => {
                self.settings = true;
                self.status = message;
                self.error = None;
            }
            Event::Connected(account, label) => {
                if self
                    .account
                    .as_ref()
                    .is_some_and(|old| old.namespace() != account.namespace())
                {
                    self.artwork.clear();
                    self.artist_name = None;
                    self.artist_albums = None;
                }
                self.account = Some(account);
                self.settings = false;
                self.status = label;
                self.error = None;
                self.art_requested.clear();
                self.art_failures.clear();
                self.lyrics.clear();
                self.lyrics_requested.clear();
                self.lyrics_follow = None;
                self.highlight_album = None;
                self.highlight_requested = None;
                self.password
                    .update(cx, |input, cx| input.set_value("", window, cx));
            }
            Event::View(request, view) => {
                if request == self.request {
                    let ids = view
                        .albums
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>();
                    let old = self
                        .view
                        .albums
                        .iter()
                        .map(|a| a.id.as_str())
                        .collect::<Vec<_>>();
                    let reset = ids != old;
                    self.tracks = view.tracks.clone();
                    self.track_total = view.track_count;
                    self.loading_more = false;
                    self.view = view;
                    if reset {
                        self.alphabet_jump = None;
                        self.album_list.reset(self.album_row_count());
                        self.cover_list.reset(
                            self.cover_rows().row_count()
                                + usize::from(self.alphabet_jump.is_some()),
                        );
                    }
                    self.group_requested.clear();
                    self.group_tracks.clear();
                    if self.selected_album.as_ref().is_some_and(|selected| {
                        !self.view.albums.iter().any(|a| a.id == selected.id)
                    }) {
                        self.selected_album = None;
                    }
                }
            }
            Event::Page(request, offset, tracks) => {
                if request == self.request && offset == self.tracks.len() {
                    self.tracks.extend(tracks);
                    self.loading_more = false;
                }
            }
            Event::GroupTracks(request, id, tracks) => {
                if request == self.request {
                    self.group_tracks.insert(id.clone(), tracks);
                    if let Some(index) = self.view.albums.iter().position(|a| a.id == id) {
                        self.album_list.splice(index..index + 1, 1);
                        let row = self.cover_rows().row_for_album(index);
                        self.cover_list.splice(row..row + 1, 1);
                    }
                }
            }
            Event::Artist(name, albums) => {
                if self.artist_name.as_ref() == Some(&name) {
                    self.artist_albums = Some(albums);
                }
            }
            Event::Album(id, tracks) => {
                if self.highlight_requested.as_ref() == Some(&id) {
                    self.highlight_album = Some((id, tracks));
                }
            }
            Event::Playlists(playlists) => {
                if let Some(current) = &self.playlist {
                    self.playlist = playlists.iter().find(|p| p.id == current.id).cloned();
                }
                self.playlists = playlists;
            }
            Event::Playlist(playlist) => {
                if self.playlist.as_ref().is_some_and(|p| p.id == playlist.id) {
                    self.playlist = Some(playlist);
                }
            }
            Event::TrackUpdated(track) => {
                if self.selected.as_ref().is_some_and(|t| t.id == track.id) {
                    self.selected = Some(track.clone());
                }
                for value in self
                    .tracks
                    .iter_mut()
                    .chain(self.group_tracks.values_mut().flatten())
                    .chain(
                        self.highlight_album
                            .iter_mut()
                            .flat_map(|(_, tracks)| tracks.iter_mut()),
                    )
                {
                    if value.id == track.id {
                        *value = track.clone();
                    }
                }
                if let Some(playlist) = &mut self.playlist {
                    for value in &mut playlist.entry {
                        if value.id == track.id {
                            *value = (*track).clone();
                        }
                    }
                }
            }
            Event::Artwork(id, path) => {
                self.art_failures.remove(&id);
                self.artwork.insert(id, path);
            }
            Event::ArtworkFailed(id) => {
                self.art_failures.insert(id);
            }
            Event::ArtworkRetrying => self.art_failures.clear(),
            Event::Lyrics(id, text) => {
                self.lyrics.insert(id, text);
            }
            Event::Playback(snapshot) => {
                let progress = if snapshot.duration > 0. {
                    snapshot.position / snapshot.duration * 1000.
                } else {
                    0.
                };
                self.seek.update(cx, |slider, cx| {
                    slider.set_value(progress as f32, window, cx)
                });
                self.playback = snapshot;
            }
            Event::Status(status) => self.status = status,
            Event::Error(error) => {
                self.error = Some(error);
                self.loading_more = false;
            }
            Event::MutationDone => {
                self.pending_mutation = None;
                self.error = None;
                self.show_playlist_editor = false;
                self.status = "Saved to Navidrome".into();
            }
        }
        cx.notify();
    }
    fn apply_theme(&self, window: &mut Window, cx: &mut App) {
        let mode = self.preferences.dark.map_or_else(
            || ThemeMode::from(window.appearance()),
            |dark| {
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            },
        );
        Theme::change(mode, Some(window), cx);
        let colors = ink(cx);
        let theme = Theme::global_mut(cx);
        theme.font_size = px(12.);
        theme.radius = px(2.);
        theme.radius_lg = px(3.);
        theme.colors.background = colors.background;
        theme.colors.foreground = colors.text;
        theme.colors.primary = colors.accent;
        theme.colors.primary_foreground = colors.background;
        theme.colors.muted = colors.panel;
        theme.colors.muted_foreground = colors.muted;
        theme.colors.border = colors.line;
    }
    fn save_preferences(&self) {
        self.service
            .send(Command::Preferences(self.preferences.clone()));
    }
    fn request_query(&mut self, cx: &mut Context<Self>) {
        self.request = self.service.query(self.query.clone());
        self.group_requested.clear();
        self.group_tracks.clear();
        self.selected_album = None;
        self.loading_more = false;
        cx.notify();
    }
    fn is_library(&self) -> bool {
        self.artist_name.is_none() && matches!(self.workspace, Workspace::Explorer)
    }
    fn main_view(&self) -> MainView {
        self.preferences.layout.explorer_view
    }
    fn browser_visible(&self) -> bool {
        self.is_library() && self.preferences.layout.explorer_browser
    }
    fn toggle_browser(&mut self, cx: &mut Context<Self>) {
        self.preferences.layout.explorer_browser = !self.preferences.layout.explorer_browser;
        self.save_preferences();
        cx.notify();
    }
    fn cover_rows(&self) -> CoverRows {
        CoverRows::new(
            self.view.albums.len(),
            self.cover_columns,
            self.alphabet_jump,
        )
    }
    fn album_row_count(&self) -> usize {
        self.view.albums.len() + usize::from(self.alphabet_jump.is_some())
    }
    fn jump_to_album(&mut self, index: usize) {
        self.alphabet_jump = Some(index);
        if self.main_view() == MainView::AlbumCovers {
            let rows = self.cover_rows();
            self.cover_list.reset(rows.row_count() + 1);
            self.cover_list.scroll_to(ListOffset {
                item_ix: rows.row_for_album(index),
                offset_in_item: px(0.),
            });
        } else {
            self.album_list.reset(self.album_row_count());
            self.album_list.scroll_to(ListOffset {
                item_ix: index,
                offset_in_item: px(0.),
            });
        }
    }
    fn set_view(&mut self, view: MainView, cx: &mut Context<Self>) {
        self.alphabet_jump = None;
        self.preferences.layout.explorer_view = view;
        self.album_list.reset(self.album_row_count());
        self.cover_list
            .reset(self.cover_rows().row_count() + usize::from(self.alphabet_jump.is_some()));
        self.save_preferences();
        cx.notify();
    }
    fn switch_workspace(
        &mut self,
        workspace: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace == Workspace::Explorer {
            self.explorer_query = self.query.clone();
        }
        self.workspace = workspace;
        if let Some(index) = Workspace::ALL.iter().position(|tab| *tab == workspace) {
            self.workspace_scroll.scroll_to_item(index);
        }
        self.artist_name = None;
        self.artist_albums = None;
        self.lyrics_follow = None;
        self.settings = false;
        self.search_text = match workspace {
            Workspace::Explorer => {
                self.query = self.explorer_query.clone();
                self.query.text.clone()
            }
            Workspace::Playlists => self.playlist_search.clone(),
            Workspace::Queue => self.queue_search.clone(),
            Workspace::NowPlaying => String::new(),
        };
        self.search.update(cx, |input, cx| {
            input.set_value(self.search_text.clone(), window, cx)
        });
        if self.is_library() {
            self.request_query(cx);
        }
        cx.notify();
    }
    fn select_browser(
        &mut self,
        column: BrowserColumn,
        value: Option<String>,
        multiple: bool,
        cx: &mut Context<Self>,
    ) {
        self.query.selection.select(column, value, multiple);
        self.request_query(cx);
    }
    fn select_track(&mut self, track: Arc<Track>, cx: &mut Context<Self>) {
        self.selected = Some(track);
        cx.notify();
    }
    fn select_album(&mut self, album: Arc<Album>, cx: &mut Context<Self>) {
        let old = self
            .selected_album
            .as_ref()
            .and_then(|a| self.view.albums.iter().position(|v| v.id == a.id));
        let current = self.view.albums.iter().position(|a| a.id == album.id);
        self.selected_album = Some(album.clone());
        self.request_group(&album.id);
        for index in [old, current].into_iter().flatten() {
            let row = self.cover_rows().row_for_album(index);
            self.cover_list.splice(row..row + 1, 1);
        }
        cx.notify();
    }
    fn request_group(&mut self, id: &str) {
        if !self.group_tracks.contains_key(id) && self.group_requested.insert(id.into()) {
            self.service.send(Command::GroupTracks(
                self.request,
                if self.artist_name.is_some() {
                    LibraryQuery::default()
                } else {
                    self.query.clone()
                },
                id.into(),
            ));
        }
    }
    fn mutate(&mut self, mutation: Mutation, cx: &mut Context<Self>) {
        self.pending_mutation = Some(mutation.clone());
        self.service.send(Command::Mutate(mutation));
        cx.notify();
    }
    fn play_track(&self, track: &Track) {
        self.service.send(Command::PlayTrack(track.id.clone()));
    }
    fn open_artist(
        &mut self,
        name: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if name.trim().is_empty() {
            return;
        }
        let origin = if self.artist_name.is_some() {
            self.artist_origin
        } else {
            self.workspace
        };
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.artist_origin = origin;
        self.artist_name = Some(name.clone());
        self.artist_albums = None;
        self.selected_album = None;
        self.group_tracks.clear();
        self.group_requested.clear();
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.service.send(Command::Artist(name, id));
        cx.notify();
    }
    fn open_explorer_filter(
        &mut self,
        filter: ExplorerFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.query = filter.query();
        self.explorer_query = self.query.clone();
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.request_query(cx);
        self.focus.focus(window, cx);
    }
    fn metadata_link(
        &self,
        filter: ExplorerFilter,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match &filter {
            ExplorerFilter::Genre(genre) => genre.clone(),
            ExplorerFilter::Year(year) => year.to_string(),
        };
        let kind = match &filter {
            ExplorerFilter::Genre(_) => "genre",
            ExplorerFilter::Year(_) => "year",
        };
        div()
            .id(SharedString::from(key))
            .role(Role::Button)
            .aria_label(format!("Explore {kind}: {label}"))
            .cursor_pointer()
            .hover(|link| link.text_color(ink(cx).accent))
            .child(label)
            .on_click(cx.listener(move |app, _, window, cx| {
                cx.stop_propagation();
                app.open_explorer_filter(filter.clone(), window, cx);
            }))
            .into_any_element()
    }
    fn genre_links(&self, genres: &str, key: String, cx: &mut Context<Self>) -> AnyElement {
        let mut links = div().flex().flex_wrap().min_w_0().items_center().gap_1();
        for (index, genre) in genres
            .split(" · ")
            .map(str::trim)
            .filter(|genre| !genre.is_empty())
            .enumerate()
        {
            if index > 0 {
                links = links.child("·");
            }
            links = links.child(self.metadata_link(
                ExplorerFilter::Genre(genre.into()),
                format!("{key}-{index}"),
                cx,
            ));
        }
        links.into_any_element()
    }
    fn year_link(&self, year: Option<i32>, key: String, cx: &mut Context<Self>) -> AnyElement {
        year.map_or_else(
            || div().into_any_element(),
            |year| self.metadata_link(ExplorerFilter::Year(year), key, cx),
        )
    }
    fn active_filter_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut bar = div()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(colors.line)
            .text_size(px(10.))
            .text_color(colors.muted);
        if !self.query.performer.is_empty() {
            bar = bar.child(
                Button::new("clear-performer-filter")
                    .ghost()
                    .compact()
                    .label(format!("Artist: {} ×", self.query.performer))
                    .accessibility_label("Clear artist filter")
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.performer.clear();
                        app.request_query(cx);
                    })),
            );
        }
        if let Some(year) = self.query.year {
            bar = bar.child(
                Button::new("clear-year-filter")
                    .ghost()
                    .compact()
                    .label(format!("Year: {year} ×"))
                    .accessibility_label(format!("Clear year {year} filter"))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.year = None;
                        app.request_query(cx);
                    })),
            );
        }
        for (index, genre) in self.query.selection.genres.iter().enumerate() {
            let genre = genre.clone();
            bar = bar.child(
                Button::new(("clear-genre-filter", index))
                    .ghost()
                    .compact()
                    .label(format!("Genre: {genre} ×"))
                    .accessibility_label(format!("Clear genre {genre} filter"))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.query.selection.genres.retain(|value| value != &genre);
                        app.query.selection.artists.clear();
                        app.query.selection.albums.clear();
                        app.request_query(cx);
                    })),
            );
        }
        bar.into_any_element()
    }
    fn artist_link(
        &self,
        name: String,
        id: String,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.artist_links(artist_credits(&name, &[], &id), key, cx)
    }
    fn artist_links(
        &self,
        credits: Vec<NamedId>,
        key: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut links = div()
            .flex()
            .flex_wrap()
            .items_center()
            .min_w_0()
            .gap(px(3.));
        if key.starts_with("footer") {
            links = links.flex_nowrap().overflow_hidden();
        }
        for (index, credit) in credits.into_iter().enumerate() {
            if index > 0 {
                links = links.child(div().text_color(ink(cx).muted).child("·"));
            }
            let name = credit.name;
            let id = credit.id;
            links = links.child(
                div()
                    .id(SharedString::from(format!("{key}-{index}")))
                    .role(Role::Button)
                    .aria_label(format!("Open artist {name}"))
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .hover(|link| link.text_color(ink(cx).accent))
                    .child(name.clone())
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        app.open_artist(name.clone(), id.clone(), window, cx);
                    })),
            );
        }
        links.into_any_element()
    }
    fn inspected_track(&self) -> Option<Arc<Track>> {
        match self.preferences.layout.info_source {
            InfoSource::Selected => self
                .selected
                .clone()
                .or_else(|| {
                    self.selected_album
                        .as_ref()
                        .and_then(|a| self.group_tracks.get(&a.id))
                        .and_then(|tracks| tracks.first().cloned())
                })
                .or_else(|| self.playback.track.clone()),
            InfoSource::Playing => self.playback.track.clone(),
        }
    }
    fn art(&mut self, id: &str, side: f32, cx: &App) -> AnyElement {
        if let Some(path) = self.artwork.get(id) {
            return img(path.clone())
                .size(px(side))
                .object_fit(ObjectFit::Cover)
                .into_any_element();
        }
        if !id.is_empty() && self.art_requested.insert(id.into()) {
            self.service.send(Command::Artwork(id.into()));
        }
        div()
            .size(px(side))
            .flex_shrink_0()
            .bg(ink(cx).panel)
            .border_1()
            .border_color(ink(cx).line)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Icon::new(Glyph::Disc3)
                    .size(px((side * 0.32).max(16.)))
                    .text_color(ink(cx).muted),
            )
            .into_any_element()
    }
    fn tabs(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let width = f32::from(window.viewport_size().width);
        let search_width = (width * 0.3).clamp(140., 240.);
        let tabs_width = (width - search_width - 78. - 32. - 24.).clamp(100., 440.);
        let scroll_buttons = tabs_width < 440.;
        let mut tabs = div()
            .id("workspace-tabs")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .h_full()
            .gap_1()
            .overflow_x_scroll()
            .track_scroll(&self.workspace_scroll);
        for workspace in Workspace::ALL {
            let label = workspace.label().to_ascii_uppercase();
            let active = self.workspace == workspace && !self.settings;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("workspace-{label}")))
                    .role(Role::Tab)
                    .aria_label(label.clone())
                    .aria_selected(active)
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .border_b_2()
                    .border_color(if active {
                        colors.accent
                    } else {
                        colors.background
                    })
                    .text_color(if active { colors.accent } else { colors.muted })
                    .text_size(px(11.))
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.panel))
                    .child(label)
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.switch_workspace(workspace, window, cx)
                    })),
            );
        }
        let tabs = div()
            .w(px(tabs_width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .when(scroll_buttons, |row| {
                row.child(
                    short_button("tabs-left", Glyph::ChevronLeft, "Scroll tabs left").on_click(
                        cx.listener(|app, _, _, cx| {
                            let offset = app.workspace_scroll.offset();
                            app.workspace_scroll
                                .set_offset(point(offset.x + px(140.), px(0.)));
                            cx.notify();
                        }),
                    ),
                )
            })
            .child(tabs)
            .when(scroll_buttons, |row| {
                row.child(
                    short_button("tabs-right", Glyph::ChevronRight, "Scroll tabs right").on_click(
                        cx.listener(|app, _, _, cx| {
                            let offset = app.workspace_scroll.offset();
                            app.workspace_scroll
                                .set_offset(point(offset.x - px(140.), px(0.)));
                            cx.notify();
                        }),
                    ),
                )
            });
        let navigator = self.preferences.layout.show_navigator;
        let info = self.preferences.layout.show_information;
        let browser = self.browser_visible();
        div()
            .w_full()
            .min_w_0()
            .h(px(36.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors.line)
            .child(
                // The transparent macOS title bar places traffic lights in this area.
                div().w(px(78.)).h_full().flex_shrink_0(),
            )
            .child(tabs)
            .child(
                div()
                    .id("titlebar-drag")
                    .flex_1()
                    .h_full()
                    .on_double_click(|_, window, _| window.titlebar_double_click())
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|app, _, _, _| app.titlebar_dragging = true),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|app, _, _, _| app.titlebar_dragging = false),
                    )
                    .on_mouse_down_out(cx.listener(|app, _, _, _| app.titlebar_dragging = false))
                    .on_mouse_move(cx.listener(|app, _, window, _| {
                        if app.titlebar_dragging {
                            app.titlebar_dragging = false;
                            window.start_window_move();
                        }
                    })),
            )
            .child(
                short_button("layout-menu", Glyph::PanelsTopLeft, "Configure layout")
                    .dropdown_menu(move |menu, _, _| {
                        menu.item(
                            PopupMenuItem::new("Navigator and artist browser")
                                .checked(navigator)
                                .action(Box::new(ToggleNavigator)),
                        )
                        .item(
                            PopupMenuItem::new("Queue and information")
                                .checked(info)
                                .action(Box::new(ToggleInformation)),
                        )
                        .item(
                            PopupMenuItem::new("Column browser")
                                .checked(browser)
                                .action(Box::new(ToggleBrowser)),
                        )
                        .separator()
                        .menu("Restore initial layout", Box::new(RestoreLayout))
                        .separator()
                        .menu("Settings…", Box::new(OpenSettings))
                    }),
            )
            .child(
                div().w(px(search_width)).flex_shrink_0().px_2().child(
                    Input::new(&self.search)
                        .small()
                        .prefix(Icon::new(Glyph::Search).size_3()),
                ),
            )
            .into_any_element()
    }
    fn navigation_row(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        icon: Glyph,
        active: bool,
        cx: &App,
    ) -> Stateful<Div> {
        let colors = ink(cx);
        let label: SharedString = label.into();
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label.clone())
            .h(px(25.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .text_size(px(11.))
            .text_color(if active { colors.accent } else { colors.text })
            .when(active, |s| s.bg(colors.selection))
            .hover(|s| s.bg(colors.line))
            .child(Icon::new(icon).size_3())
            .child(div().flex_1().min_w_0().truncate().child(label))
    }
    fn navigator(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut tree = div().flex().flex_col();
        tree = tree.child(
            self.navigation_row(
                "library-disclosure",
                "Library",
                if self.library_open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                false,
                cx,
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.library_open = !app.library_open;
                cx.notify();
            })),
        );
        if self.library_open {
            tree = tree.child(
                div()
                    .pl_3()
                    .child(
                        self.navigation_row(
                            "nav-explorer",
                            "Music Explorer",
                            Glyph::Music,
                            self.workspace == Workspace::Explorer && !self.query.favorites,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Explorer, window, cx);
                            app.query = LibraryQuery::default();
                            app.search_text.clear();
                            app.search.update(cx, |s, cx| s.set_value("", window, cx));
                            app.request_query(cx);
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-favorites",
                            "Favorites",
                            Glyph::Heart,
                            self.workspace == Workspace::Explorer && self.query.favorites,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Explorer, window, cx);
                            app.query.favorites = true;
                            app.request_query(cx);
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-queue",
                            "Queue",
                            Glyph::ListMusic,
                            self.workspace == Workspace::Queue,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::Queue, window, cx)
                        })),
                    )
                    .child(
                        self.navigation_row(
                            "nav-now-playing",
                            "Now Playing",
                            Glyph::Disc3,
                            self.workspace == Workspace::NowPlaying,
                            cx,
                        )
                        .on_click(cx.listener(|app, _, window, cx| {
                            app.switch_workspace(Workspace::NowPlaying, window, cx)
                        })),
                    ),
            );
        }
        tree = tree.child(
            self.navigation_row(
                "playlist-disclosure",
                "Playlists",
                if self.playlists_open {
                    Glyph::ChevronDown
                } else {
                    Glyph::ChevronRight
                },
                false,
                cx,
            )
            .child(
                short_button("new-playlist", Glyph::Plus, "New playlist").on_click(cx.listener(
                    |app, _, _, cx| {
                        cx.stop_propagation();
                        app.show_playlist_editor = true;
                        app.playlist_editor_rename = false;
                        cx.notify();
                    },
                )),
            )
            .on_click(cx.listener(|app, _, _, cx| {
                app.playlists_open = !app.playlists_open;
                cx.notify();
            })),
        );
        if self.playlists_open {
            for playlist in self.playlists.clone() {
                let active = self.workspace == Workspace::Playlists
                    && self.playlist.as_ref().is_some_and(|p| p.id == playlist.id);
                tree = tree.child(
                    div().pl_3().child(
                        self.navigation_row(
                            SharedString::from(format!("nav-playlist-{}", playlist.id)),
                            playlist.name.clone(),
                            Glyph::ListMusic,
                            active,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |app, _, window, cx| {
                                app.switch_workspace(Workspace::Playlists, window, cx);
                                app.playlist = Some(playlist.clone());
                                app.selected = None;
                                app.service.send(Command::Playlist(playlist.id.clone()));
                                cx.notify();
                            },
                        )),
                    ),
                );
            }
        }
        let mode = self.preferences.layout.navigator_mode;
        let values = self.view.navigator.values(mode).to_vec();
        let count = values.len();
        let all = match mode {
            NavigatorMode::Artist => self.query.performer.is_empty(),
            NavigatorMode::AlbumArtist => {
                self.query.selection.artists.is_empty() && self.artist_name.is_none()
            }
            NavigatorMode::Year => self.query.year.is_none(),
            NavigatorMode::Genre => {
                self.query.selection.genres.is_empty() && self.query.genre.is_empty()
            }
        };
        let entity = cx.entity();
        let facet_list = uniform_list("navigator-facets", count, move |range, _, cx| {
            entity.update(cx, |app, cx| {
                range
                    .map(|i| {
                        let item = values[i].clone();
                        let active = match mode {
                            NavigatorMode::Artist => app.query.performer == item.id,
                            NavigatorMode::AlbumArtist => {
                                app.query.selection.artists.contains(&item.id)
                                    || app.artist_name.as_ref() == Some(&item.id)
                            }
                            NavigatorMode::Year => {
                                app.query.year.map(|year| year.to_string()).as_ref()
                                    == Some(&item.id)
                            }
                            NavigatorMode::Genre => {
                                app.query.selection.genres.contains(&item.id)
                                    || app.query.genre == item.id
                            }
                        };
                        let art = app.art(&item.artwork, 34., cx);
                        let id = item.id.clone();
                        let artist_id = item.detail.clone();
                        div()
                            .id(("navigator-facet", i))
                            .role(Role::Button)
                            .aria_label(format!("Browse {}: {}", mode.label(), item.label))
                            .h(px(45.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(active, |s| s.bg(ink(cx).selection))
                            .hover(|s| s.bg(ink(cx).line))
                            .child(art)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .truncate()
                                            .text_color(if active {
                                                ink(cx).accent
                                            } else {
                                                ink(cx).text
                                            })
                                            .child(item.label),
                                    )
                                    .child(
                                        div().text_size(px(10.)).text_color(ink(cx).muted).child(
                                            format!(
                                                "{} album{}",
                                                item.count,
                                                if item.count == 1 { "" } else { "s" }
                                            ),
                                        ),
                                    ),
                            )
                            .on_click(cx.listener(move |app, event: &ClickEvent, window, cx| {
                                match mode {
                                    NavigatorMode::AlbumArtist => {
                                        if event.modifiers().platform || event.modifiers().control {
                                            if !app.is_library() {
                                                app.switch_workspace(
                                                    Workspace::Explorer,
                                                    window,
                                                    cx,
                                                );
                                            }
                                            app.select_browser(
                                                BrowserColumn::Artist,
                                                Some(id.clone()),
                                                true,
                                                cx,
                                            );
                                        } else {
                                            app.open_artist(
                                                id.clone(),
                                                artist_id.clone(),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                    NavigatorMode::Artist => {
                                        app.switch_workspace(Workspace::Explorer, window, cx);
                                        app.query = LibraryQuery {
                                            performer: id.clone(),
                                            ..Default::default()
                                        };
                                        app.search_text.clear();
                                        app.search.update(cx, |input, cx| {
                                            input.set_value("", window, cx)
                                        });
                                        app.request_query(cx);
                                    }
                                    NavigatorMode::Year => {
                                        if let Ok(year) = id.parse() {
                                            app.open_explorer_filter(
                                                ExplorerFilter::Year(year),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                    NavigatorMode::Genre => app.open_explorer_filter(
                                        ExplorerFilter::Genre(id.clone()),
                                        window,
                                        cx,
                                    ),
                                }
                            }))
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(&self.artist_scroll)
        .flex_1()
        .size_full();
        let weak = cx.weak_entity();
        let selector = Button::new("navigator-mode")
            .ghost()
            .small()
            .w_full()
            .h(px(26.))
            .rounded_none()
            .text_size(px(11.))
            .font_weight(FontWeight::NORMAL)
            .text_color(colors.muted)
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_size(px(11.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(colors.muted)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().truncate().child(mode.label()))
                    .child(
                        div()
                            .flex_shrink_0()
                            .opacity(0.)
                            .group_hover("navigator-heading", |s| s.opacity(1.))
                            .child(Icon::new(Glyph::ChevronDown).size_3()),
                    ),
            )
            .accessibility_label(format!("Browse library by {}", mode.label()))
            .dropdown_menu(move |mut menu, _, _| {
                for choice in NavigatorMode::ALL {
                    let weak = weak.clone();
                    menu = menu.item(
                        PopupMenuItem::new(choice.label())
                            .checked(choice == mode)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    app.preferences.layout.navigator_mode = choice;
                                    app.artist_scroll.scroll_to_item(0, ScrollStrategy::Top);
                                    app.save_preferences();
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let upper = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.panel)
            .child(panel_title("Navigator", cx))
            .child(
                div()
                    .id("navigation-tree")
                    .overflow_y_scroll()
                    .flex_1()
                    .child(tree),
            );
        let lower = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.panel)
            .child(
                div()
                    .group("navigator-heading")
                    .h(px(27.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(selector),
            )
            .child(
                div()
                    .id("all-navigator-values")
                    .role(Role::Button)
                    .aria_label(mode.all_label())
                    .h(px(35.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .when(all, |s| s.bg(colors.selection))
                    .child(mode.all_label())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child(count.to_string()),
                    )
                    .on_click(cx.listener(move |app, _, window, cx| {
                        app.switch_workspace(Workspace::Explorer, window, cx);
                        match mode {
                            NavigatorMode::Artist => app.query.performer.clear(),
                            NavigatorMode::AlbumArtist => {
                                app.query
                                    .selection
                                    .select(BrowserColumn::Artist, None, false)
                            }
                            NavigatorMode::Year => app.query.year = None,
                            NavigatorMode::Genre => {
                                app.query.genre.clear();
                                app.query
                                    .selection
                                    .select(BrowserColumn::Genre, None, false);
                            }
                        }
                        app.request_query(cx);
                    })),
            )
            .child(facet_list);
        v_resizable("left-panels")
            .child(
                resizable_panel()
                    .size(px(236.))
                    .size_range(px(100.)..px(600.))
                    .child(upper),
            )
            .child(
                resizable_panel()
                    .size_range(px(100.)..px(2000.))
                    .child(lower),
            )
            .into_any_element()
    }
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let label = self.artist_name.as_deref().unwrap_or(match self.workspace {
            Workspace::Queue => "Queue",
            Workspace::NowPlaying => "Now Playing",
            Workspace::Playlists => self
                .playlist
                .as_ref()
                .map_or("Playlists", |p| p.name.as_str()),
            _ => self.main_view().label(),
        });
        let weak = cx.weak_entity();
        let mut left = div().flex().items_center().gap_1();
        if self.is_library() {
            left = left.child(
                Button::new("main-view-menu")
                    .ghost()
                    .small()
                    .label(label.to_string())
                    .icon(Glyph::ChevronDown)
                    .dropdown_menu(move |mut menu, _, _| {
                        for view in [
                            MainView::AlbumsAndTracks,
                            MainView::AlbumCovers,
                            MainView::Tracks,
                        ] {
                            let weak = weak.clone();
                            menu = menu.item(PopupMenuItem::new(view.label()).on_click(
                                move |_, _, cx| {
                                    let _ = weak.update(cx, |app, cx| app.set_view(view, cx));
                                },
                            ));
                        }
                        menu
                    }),
            );
        } else {
            left = left.child(
                div()
                    .px_2()
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.to_string()),
            );
        }
        let mut alphabet = div().flex().items_center().gap(px(1.));
        if self.is_library() && self.main_view() != MainView::Tracks && self.library_width >= 700. {
            for letter in "#ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
                alphabet = alphabet.child(
                    div()
                        .id(SharedString::from(format!("jump-{letter}")))
                        .w(px(13.))
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(9.))
                        .text_color(colors.muted)
                        .cursor_pointer()
                        .hover(|s| s.text_color(colors.accent).bg(colors.line))
                        .child(letter.to_string())
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let found = app.view.albums.iter().position(|album| {
                                let value = if app.query.sort == Sort::Artist {
                                    &album.artist
                                } else {
                                    &album.name
                                };
                                let first = value
                                    .chars()
                                    .next()
                                    .map(|c| c.to_ascii_uppercase())
                                    .unwrap_or('#');
                                if letter == '#' {
                                    !first.is_ascii_alphabetic()
                                } else {
                                    first == letter
                                }
                            });
                            if let Some(index) = found {
                                app.jump_to_album(index);
                                cx.notify();
                            }
                        })),
                );
            }
        }
        div()
            .h(px(30.))
            .px_1()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(colors.line)
            .child(left)
            .child(alphabet)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .when(self.is_library(), |s| {
                        s.child(
                            short_button(
                                "toggle-browser",
                                Glyph::Columns3,
                                "Toggle column browser",
                            )
                            .selected(self.browser_visible())
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_browser(cx))),
                        )
                        .child(
                            short_button("clear-filter", Glyph::ListFilter, "Clear filters")
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.query.selection = Default::default();
                                    app.query.favorites = false;
                                    app.query.genre.clear();
                                    app.query.artist.clear();
                                    app.query.performer.clear();
                                    app.query.year = None;
                                    app.request_query(cx);
                                })),
                        )
                        .child(
                            Button::new("sort-menu")
                                .ghost()
                                .compact()
                                .icon(Glyph::ArrowDownWideNarrow)
                                .tooltip("Sort music")
                                .dropdown_menu({
                                    let weak = cx.weak_entity();
                                    move |mut menu, _, _| {
                                        for (sort, label) in [
                                            (Sort::Artist, "Album Artist / Album"),
                                            (Sort::Album, "Album"),
                                            (Sort::Year, "Year"),
                                            (Sort::Title, "Track title"),
                                            (Sort::Rating, "Rating"),
                                        ] {
                                            let weak = weak.clone();
                                            menu = menu.item(PopupMenuItem::new(label).on_click(
                                                move |_, _, cx| {
                                                    let _ = weak.update(cx, |app, cx| {
                                                        app.query.sort = sort;
                                                        app.request_query(cx);
                                                    });
                                                },
                                            ));
                                        }
                                        menu
                                    }
                                }),
                        )
                    })
                    .child(
                        short_button("refresh-library", Glyph::RefreshCw, "Refresh library")
                            .on_click(
                                cx.listener(|app, _, _, _| app.service.send(Command::Refresh)),
                            ),
                    ),
            )
            .into_any_element()
    }
    fn browser_column(
        &self,
        column: BrowserColumn,
        label: &'static str,
        values: Vec<Facet>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let selected = match column {
            BrowserColumn::Genre => &self.query.selection.genres,
            BrowserColumn::Artist => &self.query.selection.artists,
            BrowserColumn::Album => &self.query.selection.albums,
        };
        let empty = selected.is_empty();
        let selected = selected.clone();
        let entity = cx.entity();
        let count = values.len();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(colors.line)
            .child(panel_title(label, cx))
            .child(
                div()
                    .id(SharedString::from(format!("all-{label}")))
                    .role(Role::Button)
                    .aria_label(format!("All {label}"))
                    .h(px(23.))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .when(empty, |s| s.bg(colors.selection).text_color(colors.accent))
                    .child(format!("All ({count})"))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.select_browser(column, None, false, cx)
                    })),
            )
            .child(
                uniform_list(
                    SharedString::from(format!("browser-{label}")),
                    count,
                    move |range, _, cx| {
                        entity.update(cx, |_, cx| {
                            range
                                .map(|i| {
                                    let item = values[i].clone();
                                    let active = selected.contains(&item.id);
                                    let artist_id = item.detail.clone();
                                    let id = item.id;
                                    div()
                                        .id(("browser-value", i))
                                        .role(Role::Button)
                                        .aria_label(format!("Filter {label}: {}", item.label))
                                        .h(px(22.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .cursor_pointer()
                                        .when(active, |s| {
                                            s.bg(colors.selection).text_color(colors.accent)
                                        })
                                        .hover(|s| s.bg(colors.line))
                                        .child(
                                            div().flex_1().min_w_0().truncate().child(item.label),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(colors.muted)
                                                .child(item.count.to_string()),
                                        )
                                        .on_click(cx.listener(
                                            move |app, event: &ClickEvent, window, cx| {
                                                if column == BrowserColumn::Artist
                                                    && !(event.modifiers().platform
                                                        || event.modifiers().control)
                                                {
                                                    app.open_artist(
                                                        id.clone(),
                                                        artist_id.clone(),
                                                        window,
                                                        cx,
                                                    );
                                                } else {
                                                    app.select_browser(
                                                        column,
                                                        Some(id.clone()),
                                                        event.modifiers().platform
                                                            || event.modifiers().control,
                                                        cx,
                                                    );
                                                }
                                            },
                                        ))
                                })
                                .collect::<Vec<_>>()
                        })
                    },
                )
                .flex_1()
                .size_full(),
            )
            .into_any_element()
    }
    fn column_browser(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .bg(ink(cx).panel)
            .text_size(px(11.))
            .child(self.browser_column(
                BrowserColumn::Genre,
                "Genre",
                self.view.browser.genres.clone(),
                cx,
            ))
            .child(self.browser_column(
                BrowserColumn::Artist,
                "Album Artist",
                self.view.browser.artists.clone(),
                cx,
            ))
            .child(self.browser_column(
                BrowserColumn::Album,
                "Album",
                self.view.browser.albums.clone(),
                cx,
            ))
            .into_any_element()
    }
    fn track_feedback(&self, track: Arc<Track>, key: String, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let starred = track.starred.is_some();
        let rating = track.user_rating.unwrap_or(0);
        let mut stars = div().flex().items_center().flex_shrink_0();
        for value in 1..=5u8 {
            let id = track.id.clone();
            stars = stars.child(
                Button::new(SharedString::from(format!("rating-{key}-{value}")))
                    .ghost()
                    .compact()
                    .w(px(15.))
                    .h(px(23.))
                    .label(if value <= rating { "★" } else { "☆" })
                    .text_color(if value <= rating {
                        colors.accent
                    } else {
                        colors.muted
                    })
                    .tooltip(format!("Rate {value} / 5"))
                    .accessibility_label(format!("Rate {} {value} stars", track.title))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.mutate(
                            Mutation::Rating(id.clone(), if value == rating { 0 } else { value }),
                            cx,
                        );
                    })),
            );
        }
        let id = track.id.clone();
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(2.))
            .child(
                Button::new(SharedString::from(format!("love-{key}")))
                    .ghost()
                    .compact()
                    .w(px(23.))
                    .h(px(24.))
                    .label(if starred { "♥" } else { "♡" })
                    .text_color(if starred { colors.accent } else { colors.muted })
                    .tooltip(if starred {
                        "Remove favorite"
                    } else {
                        "Favorite"
                    })
                    .accessibility_label(format!("Favorite {}", track.title))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.mutate(Mutation::Favorite(id.clone(), !starred), cx);
                    })),
            )
            .child(stars)
            .into_any_element()
    }
    fn track_row(
        &mut self,
        track: Arc<Track>,
        number: usize,
        key: String,
        queue_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let selected = self.selected.as_ref().is_some_and(|t| t.id == track.id);
        let active = self
            .playback
            .track
            .as_ref()
            .is_some_and(|t| t.id == track.id);
        let feedback = self.track_feedback(track.clone(), key.clone(), cx);
        let now = track.clone();
        let play = track.clone();
        let next = track.id.clone();
        let last = track.id.clone();
        let service = self.service.clone();
        let service2 = service.clone();
        let weak = cx.weak_entity();
        let row = div()
            .id(SharedString::from(format!("row-{key}")))
            .role(Role::Group)
            .aria_label(format!("{} · {}", track.title, track.artist_label()))
            .w_full()
            .min_w_0()
            .h(px(27.))
            .px_1()
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(11.))
            .cursor_default()
            .when(selected, |s| s.bg(colors.selection))
            .hover(|s| s.bg(colors.panel))
            .child(
                div()
                    .w(px(23.))
                    .flex_shrink_0()
                    .text_color(if active { colors.accent } else { colors.muted })
                    .child(number.to_string()),
            )
            .child(div().w(px(13.)).flex_shrink_0().when(active, |s| {
                s.child(
                    Icon::new(if self.playback.playing {
                        Glyph::Volume2
                    } else {
                        Glyph::Pause
                    })
                    .size(px(11.))
                    .text_color(colors.accent),
                )
            }))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_color(if active { colors.accent } else { colors.text })
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .w(px(38.))
                    .flex_shrink_0()
                    .text_color(colors.muted)
                    .text_right()
                    .child(duration_label(track.duration)),
            )
            .child(feedback)
            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                app.select_track(now.clone(), cx);
                if event.click_count() == 2 {
                    if let Some(index) = queue_index {
                        app.service.player.send(PlayerCommand::JumpTo(index));
                    } else {
                        app.play_track(&now);
                    }
                }
            }))
            .context_menu(move |menu, _, _| {
                let weak = weak.clone();
                let remove_weak = weak.clone();
                let play = play.clone();
                let service = service.clone();
                let next = next.clone();
                let service2 = service2.clone();
                let last = last.clone();
                let menu = menu
                    .item(PopupMenuItem::new("Play Now").on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |app, _| {
                            if let Some(index) = queue_index {
                                app.service.player.send(PlayerCommand::JumpTo(index));
                            } else {
                                app.play_track(&play);
                            }
                        });
                    }))
                    .item(PopupMenuItem::new("Queue Next").on_click(move |_, _, _| {
                        service.send(Command::Enqueue(vec![next.clone()], true))
                    }))
                    .item(PopupMenuItem::new("Queue Last").on_click(move |_, _, _| {
                        service2.send(Command::Enqueue(vec![last.clone()], false))
                    }));
                if let Some(index) = queue_index {
                    let weak = remove_weak.clone();
                    menu.separator()
                        .item(
                            PopupMenuItem::new("Remove from Queue").on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |app, _| {
                                    app.service.player.send(PlayerCommand::Remove(index))
                                });
                            }),
                        )
                } else {
                    menu
                }
            });
        row.into_any_element()
    }
    fn album_group(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(album) = self.view.albums.get(index).cloned() else {
            return div().into_any_element();
        };
        let selected_panel =
            self.main_view() == MainView::AlbumCovers || self.artist_name.is_some();
        let new_artist =
            !selected_panel && (index == 0 || self.view.albums[index - 1].artist != album.artist);
        self.album_panel(album, new_artist, selected_panel, cx)
    }
    fn album_panel(
        &mut self,
        album: Arc<Album>,
        new_artist: bool,
        selected_panel: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request_group(&album.id);
        let colors = ink(cx);
        let tracks = self.group_tracks.get(&album.id).cloned();
        let cover = self.art(&album.cover_art, 124., cx);
        let selection = album.clone();
        let play_album = album.id.clone();
        let source = self.service.clone();
        let albumtitle = album.name.clone();
        let mut right = div()
            .flex_1()
            .min_w_0()
            .when(self.library_width < 520., |column| column.w_full())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(album.name.clone()),
                    )
                    .child(
                        short_button(
                            SharedString::from(format!("play-album-{}", album.id)),
                            Glyph::Play,
                            "Play album",
                        )
                        .on_click(move |_, _, _| {
                            source.send(Command::PlayAlbum(play_album.clone()))
                        }),
                    ),
            )
            .child(
                div()
                    .pb_1()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .when(selected_panel, |line| {
                        line.child(self.artist_links(
                            album.artist_credits(),
                            format!("album-artist-{}", album.id),
                            cx,
                        ))
                    })
                    .child(self.year_link(album.year, format!("album-year-{}", album.id), cx))
                    .child(self.genre_links(&album.genre, format!("album-genres-{}", album.id), cx))
                    .child(format!(
                        "{} tracks",
                        tracks.as_ref().map_or(album.song_count as usize, Vec::len)
                    )),
            );
        if let Some(tracks) = tracks {
            let discs = tracks
                .iter()
                .filter_map(|t| t.disc_number)
                .collect::<HashSet<_>>();
            let mut last_disc = None;
            for (i, track) in tracks.into_iter().enumerate() {
                if discs.len() > 1 && last_disc != track.disc_number {
                    last_disc = track.disc_number;
                    right = right.child(
                        div()
                            .h(px(23.))
                            .pt_1()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child(format!("Disc {}", last_disc.unwrap_or(1))),
                    );
                }
                let number = track.track.unwrap_or(i as u32 + 1) as usize;
                let key = format!("group-{}-{i}", album.id);
                right = right.child(self.track_row(track, number, key, None, cx));
            }
        } else {
            right = right.child(
                div()
                    .h(px(90.))
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Loading album tracks…"),
            );
        }
        div()
            .w_full()
            .min_w_0()
            .px_3()
            .pb_4()
            .pt_2()
            .flex()
            .flex_col()
            .gap_2()
            .when(new_artist, |s| {
                s.child(
                    div()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(colors.line)
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(self.artist_links(
                            album.artist_credits(),
                            format!("group-artist-{}", album.id),
                            cx,
                        )),
                )
            })
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .gap_3()
                    .items_start()
                    .when(self.library_width < 520., |row| row.flex_col())
                    .child(
                        div()
                            .w(px(124.))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .id(SharedString::from(format!("album-art-{}", album.id)))
                                    .cursor_pointer()
                                    .child(cover)
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.select_album(selection.clone(), cx)
                                    })),
                            )
                            .when(!selected_panel, |cover| {
                                cover.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.muted)
                                        .truncate()
                                        .child(albumtitle),
                                )
                            }),
                    )
                    .child(right),
            )
            .into_any_element()
    }
    fn grouped_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        list(self.album_list.clone(), move |index, _, cx| {
            entity.update(cx, |app, cx| {
                if index == app.view.albums.len() {
                    // Leave room below the last album so a letter can align at the top.
                    div().h(px(app.library_height)).into_any_element()
                } else {
                    app.album_group(index, cx)
                }
            })
        })
        .size_full()
        .into_any_element()
    }
    fn artist_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let name = self.artist_name.clone().unwrap_or_default();
        let mut page = div()
            .id("artist-page")
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .p_4();
        page = page.child(
            Button::new("artist-back")
                .ghost()
                .small()
                .icon(Glyph::ChevronLeft)
                .label("Back")
                .on_click(cx.listener(|app, _, window, cx| {
                    let origin = app.artist_origin;
                    app.switch_workspace(origin, window, cx);
                })),
        );
        let Some(albums) = self.artist_albums.clone() else {
            return page
                .child(div().mt_4().child("Loading artist releases…"))
                .into_any_element();
        };
        let mut hero = div().my_4().flex().items_center().gap_4();
        if let Some(album) = albums.first() {
            hero = hero.child(self.art(&album.cover_art, 80., cx));
        }
        page = page.child(
            hero.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child("ARTIST"),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(25.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(format!(
                                "{} release{} in your library",
                                albums.len(),
                                if albums.len() == 1 { "" } else { "s" }
                            )),
                    ),
            ),
        );
        if albums.is_empty() {
            return page
                .child(
                    div()
                        .text_color(colors.muted)
                        .child("No releases for this artist in your library."),
                )
                .into_any_element();
        }
        if !albums
            .iter()
            .any(|album| normalize(&album.name).contains(&normalize(&self.search_text)))
        {
            return page
                .child(
                    div()
                        .text_color(colors.muted)
                        .child("No releases match your search."),
                )
                .into_any_element();
        }
        let (_, side) = cover_layout((self.library_width - 8.).max(0.));
        for category in [
            ReleaseCategory::Album,
            ReleaseCategory::Ep,
            ReleaseCategory::Single,
            ReleaseCategory::Other,
        ] {
            let releases = albums
                .iter()
                .filter(|album| album.category() == category)
                .filter(|album| normalize(&album.name).contains(&normalize(&self.search_text)))
                .cloned()
                .collect::<Vec<_>>();
            if releases.is_empty() {
                continue;
            }
            let mut section = div().mt_5().min_w_0().child(
                div()
                    .mb_3()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} ({})", category.label(), releases.len())),
            );
            let mut cards = div().flex().flex_wrap().gap_4();
            for album in &releases {
                let open = album.clone();
                let selected = self
                    .selected_album
                    .as_ref()
                    .is_some_and(|selected| selected.id == album.id);
                cards = cards.child(
                    div()
                        .id(SharedString::from(format!("artist-release-{}", album.id)))
                        .role(Role::Button)
                        .aria_label(format!("Select release {}", album.name))
                        .w(px(side))
                        .min_w_0()
                        .cursor_pointer()
                        .pb_2()
                        .border_b_2()
                        .border_color(if selected {
                            colors.accent
                        } else {
                            colors.background
                        })
                        .child(self.art(&album.cover_art, side, cx))
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .truncate()
                                .child(album.name.clone()),
                        )
                        .child(div().text_size(px(10.)).text_color(colors.muted).child(
                            if let Some(year) = album.year {
                                self.metadata_link(
                                    ExplorerFilter::Year(year),
                                    format!("release-year-{}", album.id),
                                    cx,
                                )
                            } else {
                                div().child("Year unknown").into_any_element()
                            },
                        ))
                        .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                            app.select_album(open.clone(), cx);
                            if event.click_count() == 2 {
                                app.service.send(Command::PlayAlbum(open.id.clone()));
                            }
                        })),
                );
            }
            if releases.is_empty() {
                section = section.child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.muted)
                        .child("No releases in this category."),
                );
            } else {
                section = section.child(cards);
            }
            if let Some(album) = self
                .selected_album
                .clone()
                .filter(|selected| releases.iter().any(|album| album.id == selected.id))
            {
                section = section.child(
                    div()
                        .mt_3()
                        .border_y_1()
                        .border_color(colors.line)
                        .child(self.album_panel(album, false, true, cx)),
                );
            }
            page = page.child(section);
        }
        page.into_any_element()
    }
    fn covers_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let (columns, side) = cover_layout(self.library_width);
        if columns != self.cover_columns {
            self.cover_columns = columns;
            self.cover_list
                .reset(self.cover_rows().row_count() + usize::from(self.alphabet_jump.is_some()));
            if let Some(index) = self.alphabet_jump.or_else(|| {
                self.selected_album.as_ref().and_then(|selected| {
                    self.view
                        .albums
                        .iter()
                        .position(|album| album.id == selected.id)
                })
            }) {
                self.cover_list.scroll_to(ListOffset {
                    item_ix: self.cover_rows().row_for_album(index),
                    offset_in_item: px(0.),
                });
            }
        }
        let entity = cx.entity();
        list(self.cover_list.clone(), move |row, _, cx| {
            entity.update(cx, |app, cx| {
                if row == app.cover_rows().row_count() {
                    return div().h(px(app.library_height)).into_any_element();
                }
                let mut cards = div().w_full().min_w_0().flex().gap_4().px_3().pt_3();
                for index in app.cover_rows().albums_in_row(row) {
                    let album = app.view.albums[index].clone();
                    let selected = app
                        .selected_album
                        .as_ref()
                        .is_some_and(|a| a.id == album.id);
                    let art = app.art(&album.cover_art, side, cx);
                    let open = album.clone();
                    let id = album.id.clone();
                    let service = app.service.clone();
                    cards = cards.child(
                        div()
                            .id(("cover", index))
                            .role(Role::Button)
                            .aria_label(format!("Select album {} by {}", album.name, album.artist))
                            .w(px(side))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .pb_2()
                            .border_b_2()
                            .border_color(if selected {
                                ink(cx).accent
                            } else {
                                ink(cx).background
                            })
                            .child(art)
                            .child(
                                div()
                                    .mt_1()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(album.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(ink(cx).muted)
                                    .truncate()
                                    .child(app.artist_links(
                                        album.artist_credits(),
                                        format!("card-artist-{}", album.id),
                                        cx,
                                    )),
                            )
                            .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                                app.select_album(open.clone(), cx);
                                if event.click_count() == 2 {
                                    app.service.send(Command::PlayAlbum(open.id.clone()));
                                }
                            }))
                            .context_menu(move |menu, _, _| {
                                let id = id.clone();
                                let service = service.clone();
                                menu.item(PopupMenuItem::new("Play Album").on_click(
                                    move |_, _, _| service.send(Command::PlayAlbum(id.clone())),
                                ))
                            }),
                    );
                }
                let mut item = div().w_full().min_w_0().flex().flex_col().child(cards);
                if let Some(index) = app
                    .selected_album
                    .as_ref()
                    .and_then(|selected| {
                        app.view
                            .albums
                            .iter()
                            .position(|album| album.id == selected.id)
                    })
                    .filter(|index| app.cover_rows().row_for_album(*index) == row)
                {
                    item = item.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .mt_2()
                            .border_y_1()
                            .border_color(ink(cx).line)
                            .child(app.album_group(index, cx)),
                    );
                }
                item.into_any_element()
            })
        })
        .size_full()
        .into_any_element()
    }
    fn flat_tracks(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let total = self.tracks.len() + usize::from(self.tracks.len() < self.track_total);
        uniform_list("all-tracks", total, move |range, _, cx| {
            entity.update(cx, |app, cx| {
                if range.end + 20 >= app.tracks.len()
                    && app.tracks.len() < app.track_total
                    && !app.loading_more
                {
                    app.loading_more = true;
                    app.service.send(Command::Page(
                        app.request,
                        app.query.clone(),
                        app.tracks.len(),
                    ));
                }
                range
                    .map(|index| {
                        if let Some(track) = app.tracks.get(index).cloned() {
                            app.track_row(track, index + 1, format!("flat-{index}"), None, cx)
                        } else {
                            div()
                                .h(px(27.))
                                .px_3()
                                .text_color(ink(cx).muted)
                                .child("Loading more tracks…")
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            })
        })
        .track_scroll(&self.track_scroll)
        .size_full()
        .px_2()
        .into_any_element()
    }
    fn empty(&self, message: &str, cx: &mut Context<Self>) -> AnyElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .text_color(ink(cx).muted)
            .child(Icon::new(Glyph::Music).size_7())
            .child(message.to_string())
            .child(
                Button::new("empty-clear")
                    .ghost()
                    .small()
                    .label(if self.account.is_none() {
                        "Connect to Navidrome"
                    } else {
                        "Clear search and filters"
                    })
                    .on_click(cx.listener(|app, _, window, cx| {
                        if app.account.is_none() {
                            app.settings = true;
                        } else {
                            app.query = LibraryQuery::default();
                            app.search_text.clear();
                            app.search
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            app.request_query(cx);
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
    fn playing_tracks(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        let colors = ink(cx);
        let count = self.playback.queue.len();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(
                div()
                    .h(px(28.))
                    .px_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.line)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .child(format!("Queue ({count})")),
                    )
                    .child(
                        short_button("save-playing-tracks", Glyph::Save, "Save Queue as playlist")
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.show_playlist_editor = true;
                                app.playlist_editor_rename = false;
                                app.playlist_editor_queue = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                uniform_list("playing-tracks-sidebar", count, move |range, _, cx| {
                    entity.update(cx, |app, cx| {
                        range
                            .map(|index| {
                                let track = app.playback.queue[index].clone();
                                let active = app.playback.index == Some(index);
                                let select = track.clone();
                                let service = app.service.clone();
                                let service2 = service.clone();
                                let service3 = service.clone();
                                div()
                                    .id(("queued", index))
                                    .w_full()
                                    .min_w_0()
                                    .role(Role::Group)
                                    .aria_label(format!("Queue {}: {}", index + 1, track.title))
                                    .h(px(37.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .cursor_pointer()
                                    .when(active, |s| s.bg(colors.selection))
                                    .hover(|s| s.bg(colors.line))
                                    .child(div().w(px(12.)).text_color(colors.accent).when(
                                        active,
                                        |s| {
                                            s.child(
                                                Icon::new(if app.playback.playing {
                                                    Glyph::Volume2
                                                } else {
                                                    Glyph::Pause
                                                })
                                                .size_3(),
                                            )
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .truncate()
                                                    .text_color(if active {
                                                        colors.accent
                                                    } else {
                                                        colors.text
                                                    })
                                                    .child(track.title.clone()),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(colors.muted)
                                                    .truncate()
                                                    .child(app.artist_links(
                                                        track.artist_credits(),
                                                        format!("queue-artist-{index}"),
                                                        cx,
                                                    )),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(colors.muted)
                                            .child(duration_label(track.duration)),
                                    )
                                    .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                                        app.select_track(select.clone(), cx);
                                        if event.click_count() == 2 {
                                            app.service.player.send(PlayerCommand::JumpTo(index));
                                        }
                                    }))
                                    .context_menu(move |menu, _, _| {
                                        let service = service.clone();
                                        let service2 = service2.clone();
                                        let service3 = service3.clone();
                                        menu.item(PopupMenuItem::new("Play Now").on_click(
                                            move |_, _, _| {
                                                service.player.send(PlayerCommand::JumpTo(index))
                                            },
                                        ))
                                        .item(PopupMenuItem::new("Move earlier").on_click(
                                            move |_, _, _| {
                                                service2.player.send(PlayerCommand::Move(
                                                    index,
                                                    index.saturating_sub(1),
                                                ))
                                            },
                                        ))
                                        .separator()
                                        .item(
                                            PopupMenuItem::new("Remove").on_click(
                                                move |_, _, _| {
                                                    service3
                                                        .player
                                                        .send(PlayerCommand::Remove(index))
                                                },
                                            ),
                                        )
                                    })
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .flex_1()
                .size_full(),
            )
            .into_any_element()
    }
    fn track_information(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let source = self.preferences.layout.info_source;
        let tab = self.preferences.layout.info_tab;
        let mut header = div()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .justify_between()
            .flex_shrink_0()
            .border_b_1()
            .border_color(colors.line)
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Track Information"),
            );
        let mut sources = div().flex();
        for (value, label) in [
            (InfoSource::Selected, "Selected"),
            (InfoSource::Playing, "Playing"),
        ] {
            sources = sources.child(
                Button::new(SharedString::from(format!("info-{label}")))
                    .ghost()
                    .compact()
                    .h(px(22.))
                    .px_1()
                    .label(label)
                    .text_size(px(10.))
                    .text_color(if source == value {
                        colors.accent
                    } else {
                        colors.muted
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.preferences.layout.info_source = value;
                        app.save_preferences();
                        cx.notify();
                    })),
            );
        }
        header = header.child(sources);
        let mut tabs = div()
            .h(px(29.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(colors.line);
        for (value, label) in [
            (InfoTab::Details, "Details"),
            (InfoTab::Lyrics, "Lyrics"),
            (InfoTab::Artwork, "Artwork"),
        ] {
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("info-tab-{label}")))
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_selected(value == tab)
                    .px_3()
                    .h_full()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(if value == tab {
                        colors.accent
                    } else {
                        colors.muted
                    })
                    .border_b_1()
                    .border_color(if value == tab {
                        colors.accent
                    } else {
                        colors.line
                    })
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.preferences.layout.info_tab = value;
                        if value == InfoTab::Lyrics {
                            app.preferences.layout.info_source = InfoSource::Playing;
                        }
                        app.lyrics_follow = None;
                        app.save_preferences();
                        cx.notify();
                    })),
            );
        }
        let mut content = div()
            .id("information-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3();
        if let Some(track) = self.inspected_track() {
            match tab {
                InfoTab::Details => {
                    let art = self.art(&track.cover_art, 66., cx);
                    let summary = div()
                        .id("inspected-track")
                        .role(Role::Group)
                        .aria_label(format!(
                            "{} track: {}",
                            if source == InfoSource::Selected {
                                "Selected"
                            } else {
                                "Playing"
                            },
                            track.title
                        ))
                        .flex()
                        .gap_2()
                        .mb_3()
                        .child(art)
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(track.title.clone()),
                                )
                                .child(div().text_size(px(11.)).text_color(colors.muted).child(
                                    self.artist_links(
                                        track.artist_credits(),
                                        format!("track_information-artist-1-{}", track.id),
                                        cx,
                                    ),
                                ))
                                .child(div().text_size(px(10.)).text_color(colors.muted).child(
                                    self.year_link(
                                        track.year,
                                        format!("details-year-{}", track.id),
                                        cx,
                                    ),
                                )),
                        );
                    content = content.child(summary);
                    let mut fields = vec![
                        ("Album", track.album.clone()),
                        ("Album Artist", track.album_artist_label().into()),
                        ("Genre", track.genres_label()),
                        ("Time", duration_label(track.duration)),
                        ("Format", technical(&track)),
                        (
                            "Track",
                            format!(
                                "{} / Disc {}",
                                track.track.map_or("—".into(), |v| v.to_string()),
                                track.disc_number.unwrap_or(1)
                            ),
                        ),
                        ("Rating", format!("{} / 5", track.user_rating.unwrap_or(0))),
                        (
                            "Play count",
                            track.play_count.map_or("—".into(), |v| v.to_string()),
                        ),
                    ];
                    for contributor in &track.contributors {
                        fields.push((contributor.role.as_str(), contributor.artist.name.clone()));
                    }
                    if !track.music_brainz_id.is_empty() {
                        fields.push(("MusicBrainz", track.music_brainz_id.clone()));
                    }
                    if !track.comment.is_empty() {
                        fields.push(("Comment", track.comment.clone()));
                    }
                    for (label, value) in fields {
                        if !value.is_empty() {
                            let value = if label == "Album Artist" {
                                self.artist_link(
                                    value,
                                    track
                                        .album_artists
                                        .first()
                                        .map_or(String::new(), |artist| artist.id.clone()),
                                    format!("details-album-artist-{}", track.id),
                                    cx,
                                )
                            } else if label == "Genre" {
                                self.genre_links(&value, format!("details-genres-{}", track.id), cx)
                            } else {
                                div().child(value).into_any_element()
                            };
                            content = content.child(
                                div()
                                    .mb_2()
                                    .flex()
                                    .gap_2()
                                    .text_size(px(11.))
                                    .child(
                                        div()
                                            .w(px(78.))
                                            .flex_shrink_0()
                                            .text_color(colors.muted)
                                            .child(label.to_string()),
                                    )
                                    .child(div().min_w_0().flex_1().child(value)),
                            );
                        }
                    }
                }
                InfoTab::Artwork => {
                    let side = (self.preferences.layout.right_width - 28.).clamp(160., 480.);
                    content = content
                        .child(self.art(&track.cover_art, side, cx))
                        .child(div().pt_2().text_size(px(11.)).child(track.album.clone()));
                }
                InfoTab::Lyrics => {
                    content = div()
                        .id("information-lyrics")
                        .flex_1()
                        .min_h_0()
                        .child(self.lyrics_content(track.clone(), false, window, cx));
                }
            }
        } else {
            content = content.child(div().text_color(colors.muted).text_size(px(11.)).child(
                if source == InfoSource::Playing {
                    "No track playing."
                } else {
                    "Select a track to view its information."
                },
            ));
        }
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(header)
            .child(tabs)
            .child(content)
            .into_any_element()
    }
    fn lyrics_content(
        &mut self,
        track: Arc<Track>,
        highlight: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        if !self.lyrics.contains_key(&track.id) && self.lyrics_requested.insert(track.id.clone()) {
            self.service.send(Command::Lyrics(track.clone()));
        }
        let mut content = div()
            .id("lyrics-scroll")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.lyrics_scroll)
            .p_3();
        match self.lyrics.get(&track.id).cloned() {
            Some(Ok(lyrics)) if !lyrics.is_empty() => {
                let is_playing = self
                    .playback
                    .track
                    .as_ref()
                    .is_some_and(|playing| playing.id == track.id);
                let active = is_playing
                    .then(|| lyrics.active_line(self.playback.position))
                    .flatten();
                let follow = (track.id.clone(), active);
                if self.lyrics_follow.as_ref() != Some(&follow) {
                    let index = active.unwrap_or(0).saturating_sub(2);
                    if self
                        .lyrics_follow
                        .as_ref()
                        .is_some_and(|(id, _)| id == &track.id)
                        && let Some(bounds) = self.lyrics_scroll.bounds_for_item(index)
                    {
                        let target = f32::from(self.lyrics_scroll.bounds().top() - bounds.top())
                            .clamp(-f32::from(self.lyrics_scroll.max_offset().y), 0.);
                        self.lyrics_scroll_motion = Some((
                            Instant::now(),
                            f32::from(self.lyrics_scroll.offset().y),
                            target,
                        ));
                    } else {
                        self.lyrics_scroll_motion = None;
                        self.lyrics_scroll.scroll_to_top_of_item(index);
                    }
                    self.lyrics_follow = Some(follow);
                }
                if let Some((started, from, target)) = self.lyrics_scroll_motion {
                    let progress = (started.elapsed().as_secs_f32() / 0.4).min(1.);
                    let eased = 1. - (1. - progress).powi(3);
                    self.lyrics_scroll.set_offset(point(
                        self.lyrics_scroll.offset().x,
                        px(from + (target - from) * eased),
                    ));
                    if progress < 1. {
                        window.request_animation_frame();
                    } else {
                        self.lyrics_scroll_motion = None;
                    }
                }
                for (index, line) in lyrics.line.iter().enumerate() {
                    let highlighted = lyrics.line_is_active(index, active);
                    let focus = transition(
                        SharedString::from(format!("lyric-focus-{}-{highlight}-{index}", track.id)),
                        if highlighted { 1_f32 } else { 0_f32 },
                        Transition::new(Duration::from_millis(320)),
                        window,
                        cx,
                    )
                    .clamp(0., 1.);
                    let seek = is_playing.then(|| lyrics.line_position(index)).flatten();
                    let player = self.service.player.clone();
                    content = content.child(
                        div()
                            .id(SharedString::from(format!("lyric-{index}")))
                            .role(Role::Group)
                            .aria_label(format!(
                                "{}{}",
                                if highlighted { "Current lyric: " } else { "" },
                                line.value
                            ))
                            .py_2()
                            .px_2()
                            .relative()
                            .left(px(5. * focus))
                            .border_l_2()
                            .border_color(colors.accent.opacity(focus * 0.8))
                            .bg(colors.accent.opacity(focus * 0.045))
                            .opacity(0.82 + focus * 0.18)
                            .text_size(px(if highlight { 19. + focus } else { 13. }))
                            .line_height(px(if highlight { 30. } else { 21. }))
                            .text_color(colors.muted.blend(colors.accent.opacity(focus)))
                            .font_weight(if highlighted {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(if line.value.is_empty() {
                                " ".to_string()
                            } else {
                                line.value.clone()
                            })
                            .when_some(seek, |row, position| {
                                row.cursor_pointer().on_click(move |_, _, _| {
                                    player.send(PlayerCommand::Seek(position));
                                })
                            }),
                    );
                }
                if !lyrics.is_timed() {
                    content = content.child(
                        div()
                            .mt_3()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .child("Plain lyrics · no timing supplied"),
                    );
                }
            }
            value => {
                let retry = matches!(value, Some(Err(_)));
                let message = match value {
                    None => "Loading lyrics…".to_string(),
                    Some(Ok(_)) => "No lyrics supplied by the server.".to_string(),
                    Some(Err(error)) => error,
                };
                content = content.child(div().text_size(px(12.)).child(message));
                if retry {
                    let id = track.id.clone();
                    content = content.child(
                        Button::new("retry-lyrics")
                            .ghost()
                            .compact()
                            .label("Retry lyrics")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.lyrics.remove(&id);
                                app.lyrics_requested.remove(&id);
                                app.lyrics_follow = None;
                                cx.notify();
                            })),
                    );
                }
            }
        }
        content.into_any_element()
    }
    fn information_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.workspace == Workspace::Queue {
            return self.track_information(window, cx);
        }
        v_resizable("right-panels")
            .on_resize(cx.listener(
                |app, state: &Entity<gpui_kit::component::resizable::ResizableState>, _, cx| {
                    if let Some(size) = state.read(cx).sizes().first() {
                        app.preferences.layout.queue_height = f32::from(*size);
                        app.save_preferences();
                    }
                },
            ))
            .child(
                resizable_panel()
                    .size(px(self.preferences.layout.queue_height))
                    .size_range(px(100.)..px(1200.))
                    .child(self.playing_tracks(cx)),
            )
            .child(
                resizable_panel()
                    .size_range(px(150.)..px(2000.))
                    .child(self.track_information(window, cx)),
            )
            .into_any_element()
    }
    fn queue_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let values = self
            .playback
            .queue
            .iter()
            .enumerate()
            .filter(|(_, track)| {
                normalize(&format!(
                    "{} {} {}",
                    track.title,
                    track.artist_label(),
                    track.album
                ))
                .contains(&normalize(&self.queue_search))
            })
            .map(|(i, t)| (i, t.clone()))
            .collect::<Vec<_>>();
        let count = values.len();
        let entity = cx.entity();
        if count == 0 {
            return self.empty(
                "Queue is empty. Browse your library and choose Play Now or Queue Last.",
                cx,
            );
        }
        uniform_list("queue-main", count, move |range, _, cx| {
            entity.update(cx, |app, cx| {
                range
                    .map(|row| {
                        let (index, track) = values[row].clone();
                        app.track_row(
                            track,
                            index + 1,
                            format!("queue-main-{index}"),
                            Some(index),
                            cx,
                        )
                    })
                    .collect::<Vec<_>>()
            })
        })
        .size_full()
        .px_3()
        .into_any_element()
    }
    fn now_playing_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return self.empty(
                "Play an album or track to see its artwork, details, and lyrics here.",
                cx,
            );
        };
        let colors = ink(cx);
        if !track.album_id.is_empty() && self.highlight_requested.as_ref() != Some(&track.album_id)
        {
            self.highlight_requested = Some(track.album_id.clone());
            self.highlight_album = None;
            self.service.send(Command::Album(track.album_id.clone()));
        }
        let album_tracks = self
            .highlight_album
            .as_ref()
            .filter(|(id, _)| id == &track.album_id)
            .map(|(_, tracks)| tracks.clone())
            .unwrap_or_else(|| {
                self.playback
                    .queue
                    .iter()
                    .filter(|item| {
                        if track.album_id.is_empty() {
                            item.id == track.id
                        } else {
                            item.album_id == track.album_id
                        }
                    })
                    .cloned()
                    .collect()
            });
        let album_time = album_tracks.iter().map(|track| track.duration).sum::<f64>();
        let cover_side = (f32::from(window.viewport_size().width) * 0.3).clamp(260., 340.);
        let cover = self.art(&track.cover_art, cover_side, cx);
        let mut album_list = div().flex().flex_col().gap_1();
        for (index, item) in album_tracks.iter().enumerate() {
            album_list = album_list.child(self.track_row(
                item.clone(),
                item.track.unwrap_or(index as u32 + 1) as usize,
                format!("highlight-album-{index}"),
                None,
                cx,
            ));
        }
        let left = div()
            .id("now-playing-album")
            .w(px(cover_side + 64.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_y_scroll()
            .p_6()
            .bg(colors.panel)
            .child(
                div()
                    .mb_4()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("NOW PLAYING"),
            )
            .child(cover)
            .child(
                div()
                    .mt_4()
                    .text_size(px(24.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(track.album.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(15.))
                    .text_color(colors.muted)
                    .child(
                        self.artist_link(
                            track.album_artist_label().to_string(),
                            track
                                .album_artists
                                .first()
                                .map_or(String::new(), |artist| artist.id.clone()),
                            format!("playing-album-artist-{}", track.id),
                            cx,
                        ),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child(self.year_link(track.year, format!("playing-year-{}", track.id), cx))
                    .child(self.genre_links(
                        &track.genres_label(),
                        format!("playing-genres-{}", track.id),
                        cx,
                    ))
                    .child(format!(
                        "{} tracks · {}",
                        album_tracks.len(),
                        duration_label(album_time)
                    )),
            )
            .child(
                div()
                    .mt_5()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("CURRENT TRACK"),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(12.))
                    .text_color(colors.muted)
                    .child(self.artist_links(
                        track.artist_credits(),
                        format!("now_playing_view-artist-1-{}", track.id),
                        cx,
                    )),
            )
            .child(self.track_row(
                track.clone(),
                track.track.unwrap_or(1) as usize,
                "highlight-current".into(),
                None,
                cx,
            ))
            .child(
                div()
                    .mt_1()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(technical(&track)),
            )
            .child(
                div()
                    .mt_5()
                    .mb_2()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("ON THIS ALBUM"),
            )
            .child(album_list);
        let lyrics = self.lyrics_content(track.clone(), true, window, cx);
        let right = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(colors.line)
            .px_6()
            .pt_6()
            .child(
                div()
                    .mb_2()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("LYRICS"),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .mb_3()
                    .text_size(px(12.))
                    .text_color(colors.muted)
                    .child(self.artist_links(
                        track.artist_credits(),
                        format!("now_playing_view-artist-2-{}", track.id),
                        cx,
                    )),
            )
            .child(div().flex_1().min_h_0().child(lyrics));
        let waveform = self.service.player.waveform();
        let mut bars = div().flex_1().min_h_0().flex().items_center().gap(px(3.));
        for amplitude in waveform {
            bars = bars.child(
                div()
                    .flex_1()
                    .h(px(2. + amplitude.sqrt() * 64.))
                    .rounded_sm()
                    .bg(colors.accent),
            );
        }
        let visualizer = div()
            .id("output-visualizer")
            .role(Role::Group)
            .aria_label("Audio visualization")
            .h(px(108.))
            .flex_shrink_0()
            .border_t_1()
            .border_color(colors.line)
            .px_6()
            .py_3()
            .flex()
            .flex_col()
            .bg(colors.panel)
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child("AUDIO")
                    .child(if self.playback.buffering {
                        "Buffering…"
                    } else if self.playback.playing {
                        "Playing"
                    } else {
                        "Paused"
                    }),
            )
            .child(bars);
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(colors.background)
            .when_some(
                self.error.clone().or_else(|| self.playback.error.clone()),
                |page, error| {
                    page.child(
                        div()
                            .px_6()
                            .py_2()
                            .text_size(px(11.))
                            .text_color(colors.accent)
                            .child(error),
                    )
                },
            )
            .child(div().flex_1().min_h_0().flex().child(left).child(right))
            .child(visualizer)
            .into_any_element()
    }
    fn playlists_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if let Some(playlist) = self.playlist.clone() {
            let tracks = playlist
                .entry
                .iter()
                .cloned()
                .map(Arc::new)
                .collect::<Vec<_>>();
            let values = tracks
                .iter()
                .enumerate()
                .filter(|(_, track)| {
                    normalize(&format!(
                        "{} {} {}",
                        track.title,
                        track.artist_label(),
                        track.album
                    ))
                    .contains(&normalize(&self.playlist_search))
                })
                .map(|(i, t)| (i, t.clone()))
                .collect::<Vec<_>>();
            let count = values.len();
            let entity = cx.entity();
            let play = tracks;
            let weak = cx.weak_entity();
            let rename = playlist.name.clone();
            let rename_id = playlist.id.clone();
            let delete_id = playlist.id.clone();
            let save_id = playlist.id.clone();
            return div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .h(px(34.))
                        .px_2()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .border_b_1()
                        .border_color(colors.line)
                        .child(
                            Button::new("play-playlist")
                                .ghost()
                                .small()
                                .icon(Glyph::Play)
                                .label("Play")
                                .on_click(cx.listener(move |app, _, _, _| {
                                    app.service
                                        .player
                                        .send(PlayerCommand::Replace(play.clone(), 0))
                                })),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!(
                                    "{} tracks · {}",
                                    playlist.song_count,
                                    duration_label(playlist.duration)
                                )),
                        )
                        .child(
                            Button::new("playlist-options")
                                .ghost()
                                .compact()
                                .icon(Glyph::Ellipsis)
                                .tooltip("Playlist options")
                                .dropdown_menu(move |menu, _, _| {
                                    let weak = weak.clone();
                                    let weak2 = weak.clone();
                                    let weak3 = weak.clone();
                                    let name = rename.clone();
                                    let rename_id = rename_id.clone();
                                    let delete_id = delete_id.clone();
                                    let save_id = save_id.clone();
                                    menu.item(PopupMenuItem::new("Rename…").on_click(
                                        move |_, window, cx| {
                                            let _ = weak.update(cx, |app, cx| {
                                                if app
                                                    .playlist
                                                    .as_ref()
                                                    .is_some_and(|p| p.id == rename_id)
                                                {
                                                    app.show_playlist_editor = true;
                                                    app.playlist_editor_rename = true;
                                                    app.playlist_name.update(cx, |input, cx| {
                                                        input.set_value(name.clone(), window, cx)
                                                    });
                                                    cx.notify();
                                                }
                                            });
                                        },
                                    ))
                                    .item(PopupMenuItem::new("Append Queue").on_click(
                                        move |_, _, cx| {
                                            let _ = weak2.update(cx, |app, cx| {
                                                let mut ids =
                                                    app.playlist.as_ref().map_or(vec![], |p| {
                                                        p.entry
                                                            .iter()
                                                            .map(|t| t.id.clone())
                                                            .collect::<Vec<_>>()
                                                    });
                                                ids.extend(
                                                    app.playback.queue.iter().map(|t| t.id.clone()),
                                                );
                                                app.mutate(
                                                    Mutation::ReplacePlaylist(save_id.clone(), ids),
                                                    cx,
                                                );
                                            });
                                        },
                                    ))
                                    .separator()
                                    .item(
                                        PopupMenuItem::new("Delete playlist…").on_click(
                                            move |_, _, cx| {
                                                let _ = weak3.update(cx, |app, cx| {
                                                    app.pending_delete = Some(delete_id.clone());
                                                    cx.notify();
                                                });
                                            },
                                        ),
                                    )
                                }),
                        ),
                )
                .child(
                    uniform_list("playlist-tracks", count, move |range, _, cx| {
                        entity.update(cx, |app, cx| {
                            range
                                .map(|row| {
                                    let (index, track) = values[row].clone();
                                    app.track_row(
                                        track,
                                        index + 1,
                                        format!("playlist-{index}"),
                                        None,
                                        cx,
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                    })
                    .flex_1()
                    .size_full()
                    .px_2(),
                )
                .into_any_element();
        }
        let mut items = div().flex().flex_col().p_2().gap_1();
        for playlist in self.playlists.clone() {
            let name = playlist.name.clone();
            items = items.child(
                div()
                    .id(SharedString::from(format!("playlist-list-{}", playlist.id)))
                    .h(px(35.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(colors.panel))
                    .child(
                        Icon::new(Glyph::ListMusic)
                            .size_4()
                            .text_color(colors.muted),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .text_color(colors.muted)
                            .text_size(px(10.))
                            .child(format!("{} tracks", playlist.song_count)),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.playlist = Some(playlist.clone());
                        app.service.send(Command::Playlist(playlist.id.clone()));
                        cx.notify();
                    })),
            );
        }
        div()
            .id("playlist-manager")
            .size_full()
            .overflow_y_scroll()
            .child(items)
            .into_any_element()
    }
    fn connection_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        div().id("settings").size_full().overflow_y_scroll().p_5().child(div().max_w(px(500.)).w_full().flex().flex_col().gap_3()
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child("Navidrome connection"))
            .child(div().text_size(px(11.)).text_color(colors.muted).child("Enter your server address and credentials. Sonora saves a login token locally so you stay signed in. Your password is not saved. The token file is unencrypted and readable only by your macOS user."))
            .child("Server address").child(Input::new(&self.server_url).small()).child("Username").child(Input::new(&self.username).small()).child("Password").child(Input::new(&self.password).small())
            .child(div().child(Button::new("connect-library").small().label("Connect").on_click(cx.listener(|app,_,_,cx|{app.status="Connecting…".into();app.error=None;app.service.send(Command::Connect(Account{url:app.server_url.read(cx).value().to_string(),username:app.username.read(cx).value().to_string()},app.password.read(cx).value().to_string()));cx.notify();}))))
            .child(div().pt_4().border_t_1().border_color(colors.line).font_weight(FontWeight::SEMIBOLD).child("Playback"))
            .child(div().text_size(px(11.)).text_color(colors.muted).child("ReplayGain"))
            .child(div().flex().gap_2().children([GainMode::Off,GainMode::Track,GainMode::Album].into_iter().enumerate().map(|(i,mode)|{let label=match mode{GainMode::Off=>"Off",GainMode::Track=>"Track",GainMode::Album=>"Album"};Button::new(("gain",i)).small().label(label).selected(self.preferences.gain==mode).on_click(cx.listener(move|app,_,_,cx|{app.preferences.gain=mode.clone();app.save_preferences();cx.notify();}))})))
            .child(div().pt_4().border_t_1().border_color(colors.line).font_weight(FontWeight::SEMIBOLD).child("Appearance"))
            .child(div().flex().gap_2().children([(None,"System"),(Some(false),"Light"),(Some(true),"Dark")].into_iter().enumerate().map(|(i,(value,label))|Button::new(("appearance",i)).small().label(label).selected(self.preferences.dark==value).on_click(cx.listener(move|app,_,window,cx|{app.preferences.dark=value;app.apply_theme(window,cx);app.save_preferences();cx.notify();})))))
            .child(Button::new("restore-layout-settings").small().label("Restore initial panel layout").on_click(cx.listener(|app,_,_,cx|{app.preferences.layout=LayoutPreferences::default();app.save_preferences();cx.notify();})))
            .child(div().pt_3().text_size(px(10.)).text_color(colors.muted).child("Sonora · Rust / GPUI · Navidrome client"))).into_any_element()
    }
    fn player_button(
        &self,
        id: &'static str,
        icon: Glyph,
        label: &'static str,
        command: PlayerCommand,
    ) -> Button {
        let player = self.service.player.clone();
        short_button(id, icon, label).on_click(move |_, _, _| player.send(command.clone()))
    }
    fn player_bar(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let width = f32::from(window.viewport_size().width);
        let compact = width < 1000.;
        let track = self.playback.track.clone();
        let title = track
            .as_ref()
            .map_or("No track playing".into(), |track| track.title.clone());
        let artist_caption = if self.playback.buffering {
            div().child("Buffering…").into_any_element()
        } else if let Some(track) = &track {
            self.artist_links(track.artist_credits(), "footer-artists".into(), cx)
        } else {
            div().into_any_element()
        };
        let cover = track
            .as_ref()
            .map(|track| self.art(&track.cover_art, if compact { 32. } else { 40. }, cx));
        let feedback = track
            .as_ref()
            .map(|track| self.track_feedback(track.clone(), "footer".into(), cx));
        div()
            .id("player-bar")
            .w_full()
            .min_w_0()
            .h(px(56.))
            .px_2()
            .flex_shrink_0()
            .border_t_1()
            .border_color(colors.line)
            .bg(colors.panel)
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .child(self.player_button(
                        "previous",
                        Glyph::SkipBack,
                        "Previous",
                        PlayerCommand::Previous,
                    ))
                    .child(self.player_button(
                        "play-pause",
                        if self.playback.playing {
                            Glyph::Pause
                        } else {
                            Glyph::Play
                        },
                        "Play / pause",
                        PlayerCommand::Toggle,
                    ))
                    .child(self.player_button(
                        "next",
                        Glyph::SkipForward,
                        "Next",
                        PlayerCommand::Next,
                    )),
            )
            .child(
                div()
                    .w(px((width * 0.18).clamp(100., 240.)))
                    .flex_shrink_0()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .when_some(cover, |row, cover| row.child(cover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(11.)).truncate().child(title))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.muted)
                                    .min_w_0()
                                    .child(artist_caption),
                            ),
                    ),
            )
            .when_some(feedback, |bar, feedback| bar.child(feedback))
            .child(
                div().flex_1().min_w(px(40.)).flex().items_center().child(
                    Slider::new(&self.seek)
                        .flex_1()
                        .min_w_0()
                        .accessibility_label("Playback position"),
                ),
            )
            .child(
                div()
                    .w(px(64.))
                    .flex_shrink_0()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .text_center()
                    .child(format!(
                        "{} / {}",
                        duration_label(self.playback.position),
                        duration_label(self.playback.duration)
                    )),
            )
            .child(
                short_button("shuffle", Glyph::Shuffle, "Shuffle")
                    .selected(self.preferences.shuffle)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.preferences.shuffle = !app.preferences.shuffle;
                        app.save_preferences();
                        cx.notify();
                    })),
            )
            .child(
                short_button(
                    "repeat",
                    if self.preferences.repeat == RepeatMode::One {
                        Glyph::Repeat1
                    } else {
                        Glyph::Repeat
                    },
                    "Repeat off / all / one",
                )
                .selected(self.preferences.repeat != RepeatMode::Off)
                .on_click(cx.listener(|app, _, _, cx| {
                    app.preferences.repeat = match app.preferences.repeat {
                        RepeatMode::Off => RepeatMode::All,
                        RepeatMode::All => RepeatMode::One,
                        RepeatMode::One => RepeatMode::Off,
                    };
                    app.save_preferences();
                    cx.notify();
                })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_1()
                    .child(Icon::new(Glyph::Volume2).size_3().text_color(colors.muted))
                    .child(
                        Slider::new(&self.volume)
                            .w(px(if compact { 44. } else { 76. }))
                            .accessibility_label("Volume"),
                    ),
            )
            .into_any_element()
    }
    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let queue_time = self.playback.queue.iter().map(|t| t.duration).sum::<f64>();
        let library = format!(
            "{} tracks · {} albums",
            self.view.track_count,
            self.view.albums.len()
        );
        let artwork_status = if self.art_failures.is_empty() {
            String::new()
        } else {
            format!(
                " · {} covers failed; Refresh to retry",
                self.art_failures.len()
            )
        };
        div()
            .id("library-status")
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .bg(colors.panel)
            .role(Role::Status)
            .aria_label(format!(
                "{library}; {}{artwork_status}. Queue: {}",
                self.status,
                self.playback.queue.len()
            ))
            .h(px(21.))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(colors.line)
            .text_size(px(10.))
            .text_color(colors.muted)
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .child(format!("{library}   |   {}{artwork_status}", self.status)),
            )
            .child(div().ml_3().flex_shrink_0().child(format!(
                "Queued: {} · {}",
                self.playback.queue.len(),
                duration_label(queue_time)
            )))
            .child(
                short_button(
                    "status-right-panel",
                    Glyph::PanelRight,
                    "Show / hide right panel",
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.preferences.layout.show_information =
                        !app.preferences.layout.show_information;
                    app.save_preferences();
                    cx.notify();
                })),
            )
            .into_any_element()
    }
    fn main_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.settings {
            return self.connection_settings(cx);
        }
        if self.artist_name.is_some() {
            return self.artist_page(cx);
        }
        if self.workspace == Workspace::Queue {
            return self.queue_view(cx);
        }
        if self.workspace == Workspace::Playlists {
            return self.playlists_view(cx);
        }
        if self.view.track_count == 0 {
            return self.empty(
                if self.account.is_none() {
                    "Connect your Navidrome server to browse your collection."
                } else {
                    "No tracks match the current search or filters."
                },
                cx,
            );
        }
        match self.main_view() {
            MainView::AlbumsAndTracks => self.grouped_view(cx),
            MainView::AlbumCovers => self.covers_view(cx),
            MainView::Tracks => self.flat_tracks(cx),
        }
    }
    fn center_panel(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let mut center = div().size_full().min_h_0().min_w_0().flex().flex_col();
        if !self.settings {
            center = center.child(self.header(cx));
            if self.is_library()
                && (self.query.year.is_some()
                    || !self.query.selection.genres.is_empty()
                    || !self.query.performer.is_empty())
            {
                center = center.child(self.active_filter_bar(cx));
            }
        }
        if let Some(error) = self.error.clone().or_else(|| self.playback.error.clone()) {
            center =
                center.child(
                    div()
                        .w_full()
                        .px_2()
                        .py_2()
                        .flex_shrink_0()
                        .bg(if cx.theme().is_dark() {
                            rgb(0x423227)
                        } else {
                            rgb(0xf8e9d7)
                        })
                        .flex()
                        .items_start()
                        .gap_2()
                        .text_size(px(11.))
                        .child(div().flex_1().min_w_0().child(error))
                        .child(Button::new("retry-error").small().label("Retry").on_click(
                            cx.listener(|app, _, _, cx| {
                                if let Some(mutation) = app.pending_mutation.clone() {
                                    app.mutate(mutation, cx);
                                } else if app.playback.error.is_some() {
                                    app.service.player.send(PlayerCommand::Retry);
                                } else {
                                    app.service.send(Command::Refresh);
                                }
                                app.error = None;
                                cx.notify();
                            }),
                        ))
                        .child(short_button("dismiss-error", Glyph::X, "Dismiss").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.error = None;
                                app.playback.error = None;
                                app.pending_mutation = None;
                                app.service.player.send(PlayerCommand::ClearError);
                                cx.notify();
                            }),
                        )),
                );
        }
        if let Some(id) = self.pending_delete.clone() {
            center = center.child(
                div()
                    .p_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child("Delete this playlist from Navidrome?")
                    .child(
                        Button::new("confirm-delete")
                            .small()
                            .label("Delete")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.pending_delete = None;
                                app.mutate(Mutation::DeletePlaylist(id.clone()), cx);
                            })),
                    )
                    .child(
                        Button::new("cancel-delete")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.pending_delete = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        if self.show_playlist_editor {
            center = center.child(
                div()
                    .p_2()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.playlist_name).small()),
                    )
                    .child(
                        Button::new("save-playlist-editor")
                            .small()
                            .label(if self.playlist_editor_rename {
                                "Rename"
                            } else {
                                "Create playlist"
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                let name = app.playlist_name.read(cx).value().to_string();
                                if name.trim().is_empty() {
                                    return;
                                }
                                if app.playlist_editor_rename {
                                    if let Some(playlist) = &app.playlist {
                                        app.mutate(
                                            Mutation::RenamePlaylist(playlist.id.clone(), name),
                                            cx,
                                        );
                                    }
                                } else {
                                    let ids = if app.playlist_editor_queue {
                                        app.playback.queue.iter().map(|t| t.id.clone()).collect()
                                    } else {
                                        vec![]
                                    };
                                    app.mutate(Mutation::CreatePlaylist(name, ids), cx);
                                }
                            })),
                    )
                    .child(
                        short_button("cancel-playlist-editor", Glyph::X, "Cancel").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.show_playlist_editor = false;
                                app.playlist_editor_queue = false;
                                cx.notify();
                            }),
                        ),
                    ),
            );
        }
        let content = self.main_content(cx);
        let previous_width = self.library_width;
        let previous_height = self.library_height;
        let weak = cx.weak_entity();
        let content = div()
            .relative()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |bounds, window, _| {
                        let width = f32::from(bounds.size.width);
                        let height = f32::from(bounds.size.height);
                        if width > 0.
                            && height > 0.
                            && ((width - previous_width).abs() > 0.5
                                || (height - previous_height).abs() > 0.5)
                        {
                            window.on_next_frame(move |_, cx| {
                                let _ = weak.update(cx, |app, cx| {
                                    if (app.library_width - width).abs() > 0.5
                                        || (app.library_height - height).abs() > 0.5
                                    {
                                        app.library_width = width;
                                        app.library_height = height;
                                        app.album_list.remeasure();
                                        app.cover_list.remeasure();
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(content);
        if self.browser_visible() && !self.settings {
            center = center.child(
                v_resizable("browser-and-library")
                    .on_resize(cx.listener(
                        |app,
                         state: &Entity<gpui_kit::component::resizable::ResizableState>,
                         _,
                         cx| {
                            if let Some(size) = state.read(cx).sizes().first() {
                                app.preferences.layout.browser_height = f32::from(*size);
                                app.save_preferences();
                            }
                        },
                    ))
                    .child(
                        resizable_panel()
                            .size(px(self.preferences.layout.browser_height))
                            .size_range(px(90.)..px(400.))
                            .child(self.column_browser(cx)),
                    )
                    .child(
                        resizable_panel()
                            .size_range(px(160.)..px(2000.))
                            .child(content),
                    ),
            );
        } else {
            center = center.child(div().flex_1().min_h_0().min_w_0().child(content));
        }
        center.bg(colors.background).into_any_element()
    }
}
impl Render for MusicApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ink(cx);
        let body = if self.workspace == Workspace::NowPlaying && !self.settings {
            if self.playback.playing {
                window.request_animation_frame();
            }
            self.now_playing_view(window, cx)
        } else {
            let (left_width, right_width) = panel_widths(
                f32::from(window.viewport_size().width),
                self.preferences
                    .layout
                    .show_navigator
                    .then_some(self.preferences.layout.left_width),
                (self.preferences.layout.show_information && !self.settings)
                    .then_some(self.preferences.layout.right_width),
            );
            let preferred_width = if self.preferences.layout.show_navigator {
                self.preferences.layout.left_width.clamp(140., 380.)
            } else {
                0.
            } + if self.preferences.layout.show_information && !self.settings
            {
                self.preferences.layout.right_width.clamp(180., 500.)
            } else {
                0.
            };
            let compressed = left_width + right_width < preferred_width - 0.5;
            let left = self.navigator(cx);
            let center = self.center_panel(cx);
            let right = self.information_sidebar(window, cx);
            h_resizable("musicbee-workspace")
                .on_resize(cx.listener(
                    |app, state: &Entity<gpui_kit::component::resizable::ResizableState>, _, cx| {
                        let sizes = state.read(cx).sizes();
                        if let Some(size) = sizes.first().filter(|size| f32::from(**size) >= 140.) {
                            app.preferences.layout.left_width = f32::from(*size);
                        }
                        if let Some(size) = sizes.get(2).filter(|size| f32::from(**size) >= 180.) {
                            app.preferences.layout.right_width = f32::from(*size);
                        }
                        app.save_preferences();
                    },
                ))
                .child(
                    resizable_panel()
                        .visible(self.preferences.layout.show_navigator)
                        .size(px(left_width))
                        .size_range(
                            px(140.)..px(if compressed {
                                left_width.max(141.)
                            } else {
                                380.
                            }),
                        )
                        .fixed()
                        .child(left),
                )
                .child(
                    resizable_panel()
                        .size_range(px(300.)..px(2400.))
                        .child(center),
                )
                .child(
                    resizable_panel()
                        .visible(self.preferences.layout.show_information && !self.settings)
                        .size(px(right_width))
                        .size_range(
                            px(180.)..px(if compressed {
                                right_width.max(181.)
                            } else {
                                500.
                            }),
                        )
                        .fixed()
                        .child(right),
                )
                .into_any_element()
        };
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .key_context("Sonora")
            .track_focus(&self.focus)
            .font_family(".AppleSystemUIFont")
            .text_size(px(12.))
            .text_color(colors.text)
            .bg(colors.background)
            .on_action(|_: &Quit, _, cx| cx.quit())
            .on_action(cx.listener(|app, _: &TogglePlayback, _, _| {
                app.service.player.send(PlayerCommand::Toggle)
            }))
            .on_action(
                cx.listener(|app, _: &NextTrack, _, _| {
                    app.service.player.send(PlayerCommand::Next)
                }),
            )
            .on_action(cx.listener(|app, _: &PreviousTrack, _, _| {
                app.service.player.send(PlayerCommand::Previous)
            }))
            .on_action(cx.listener(|app, _: &OpenSettings, _, cx| {
                app.settings = !app.settings;
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &FocusSearch, window, cx| {
                if app.workspace == Workspace::NowPlaying {
                    app.switch_workspace(Workspace::Explorer, window, cx);
                }
                app.search.read(cx).focus_handle(cx).focus(window, cx)
            }))
            .on_action(cx.listener(|app, _: &Refresh, _, _| app.service.send(Command::Refresh)))
            .on_action(cx.listener(|app, _: &GoBack, window, cx| {
                if app.settings {
                    app.settings = false;
                } else if app.artist_name.is_some() {
                    let origin = app.artist_origin;
                    app.switch_workspace(origin, window, cx);
                } else if app.workspace == Workspace::Playlists {
                    app.playlist = None;
                } else {
                    app.query.selection = Default::default();
                    app.query.genre.clear();
                    app.query.artist.clear();
                    app.query.performer.clear();
                    app.query.year = None;
                    app.request_query(cx);
                }
                app.focus.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ToggleNavigator, _, cx| {
                app.preferences.layout.show_navigator = !app.preferences.layout.show_navigator;
                app.save_preferences();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ToggleInformation, _, cx| {
                app.preferences.layout.show_information = !app.preferences.layout.show_information;
                app.save_preferences();
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &ToggleBrowser, _, cx| app.toggle_browser(cx)))
            .on_action(cx.listener(|app, _: &RestoreLayout, _, cx| {
                app.preferences.layout = LayoutPreferences::default();
                app.save_preferences();
                app.album_list.reset(app.album_row_count());
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &QueueWorkspace, window, cx| {
                app.switch_workspace(Workspace::Queue, window, cx)
            }))
            .on_action(cx.listener(|app, _: &PlayingWorkspace, window, cx| {
                app.switch_workspace(Workspace::NowPlaying, window, cx)
            }))
            .on_action(cx.listener(|app, _: &PlaylistsWorkspace, window, cx| {
                app.switch_workspace(Workspace::Playlists, window, cx)
            }))
            .on_action(cx.listener(|app, _: &ExplorerWorkspace, window, cx| {
                app.switch_workspace(Workspace::Explorer, window, cx)
            }))
            .child(self.tabs(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(body),
            )
            .child(self.status_bar(cx))
            .child(self.player_bar(window, cx))
    }
}
