use crate::sorting::{Grouping, Preset, Scope as SortScope, SortSpec};
use crate::{
    artists::{ArtistPageData, ArtistProfile, ReleaseHit, cache_key, profile_url, search_releases},
    artwork::Priority,
    browser::*,
    catalog::LibraryView,
    discography::{DiscographyView, MissingRelease, ReleaseDetails, listen_links},
    lyrics::Lyrics,
    model::*,
    platform::Account,
    playback::{Placement, PlayerCommand, QueueEdit},
    service::{Command, Connection, DownloadRequest, Event, Mutation, SearchUpdate, Service},
    slskd::{Candidate, Completeness, DownloadJob, JobState},
    thumbnail,
    visualizer::{FFT_SIZE, SPECTRUM_BANDS, Spectrum, frequency_position},
};
use gpui_kit::assets::IconName as Glyph;
use gpui_kit::base::{Transition, transition};
use gpui_kit::component::{
    button::*,
    input::{Input, InputEvent, InputState},
    link::Link,
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    radio::{Radio, RadioGroup},
    resizable::{h_resizable, resizable_panel, v_resizable},
    setting::{SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    slider::{Slider, SliderEvent, SliderState, SliderValue},
    *,
};
use gpui_kit::{prelude::FluentBuilder, *};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
mod account_cache;
mod album_feedback;
mod albums;
mod artist_page;
mod bindings;
mod chrome;
mod discover;
mod events;
mod feedback;
mod information;
mod library;
mod navigation;
mod navigator;
mod now_playing;
mod player_bar;
mod playlist_creation;
mod playlists;
mod settings;
mod sign_in;
mod smart_playlists;
mod tracks;
mod window;
pub use bindings::{playback_bindings, queue_bindings, settings_bindings};
pub use window::restored_window_bounds;
mod context_menu;
use context_menu::menu_row;
mod downloads;
mod queue_controls;
mod sections;
use sections::{Part, style};
mod tabs;
mod tag_editor;
use queue_controls::QueueViewState;
pub use tabs::tab_bindings;
mod sorting_controls;

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
        GoForward,
        Refresh,
        ToggleNavigator,
        ToggleInformation,
        ToggleBrowser,
        RestoreLayout,
        PlayingWorkspace,
        QueueWorkspace,
        PlaylistsWorkspace,
        ExplorerWorkspace,
        NewTab,
        CloseTab,
        ReopenClosedTab,
        ResetTabs,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
        UndoQueueEdit,
        RemoveQueueSelection,
        MoveQueueEarlier,
        MoveQueueLater,
        SelectAllQueued,
        NextQueueEntry,
        PreviousQueueEntry,
        ExtendQueueNext,
        ExtendQueuePrevious,
        PlayQueueSelection
    ]
);

/// Clicking the current rating clears it; any other star sets that rating.
fn rating_after_click(rating: u8, clicked: u8) -> u8 {
    if clicked == rating { 0 } else { clicked }
}

const SPECTRUM_HEIGHT: f32 = 84.;
/// A cover must stay on screen this long before it is downloaded, so fast scrolling
/// doesn't fill the download slots with covers that are already gone.
const ART_SETTLE: Duration = Duration::from_millis(120);

/// Names of the places each queue option inserts after, captured at render.
#[derive(Default)]
struct QueuePlaces {
    current_track: Option<String>,
    current_album: Option<String>,
    last_album: Option<String>,
}

fn quoted(name: &str) -> String {
    let short = if name.chars().count() > 32 {
        format!("{}…", name.chars().take(31).collect::<String>().trim_end())
    } else {
        name.to_string()
    };
    format!("After “{short}”")
}

fn mix(from: Hsla, to: Hsla, t: f32) -> Hsla {
    let (from, to) = (from.to_rgb(), to.to_rgb());
    Hsla::from(Rgba {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: 1.,
    })
}

#[derive(Clone, Copy)]
struct Ink {
    background: Hsla,
    panel: Hsla,
    line: Hsla,
    text: Hsla,
    muted: Hsla,
    accent: Hsla,
    favorite: Hsla,
    flame: Hsla,
    selection: Hsla,
    /// Light text on dark surfaces.
    dark: bool,
}

thread_local! {
    /// Colors that replace the theme's while a view renders over a backdrop.
    static INK_OVERRIDE: std::cell::Cell<Option<Ink>> = const { std::cell::Cell::new(None) };
}

/// Restores the theme's colors when the view that overrode them finishes.
struct InkScope;
impl InkScope {
    fn new(ink: Ink) -> Self {
        INK_OVERRIDE.set(Some(ink));
        Self
    }
}
impl Drop for InkScope {
    fn drop(&mut self) {
        INK_OVERRIDE.set(None);
    }
}

/// sRGB luma (0–255) of a color, as `crate::backdrop` measures images.
fn luma_of(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    (0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b) * 255.
}

/// Position of the Layout page among the settings pages.
const LAYOUT_SETTINGS_PAGE: usize = 4;

/// The navigator as one choice: docked, sliding in from the edge, or gone.
fn navigator_key(layout: &LayoutPreferences) -> &'static str {
    match (layout.show_navigator, layout.navigator_auto_hide) {
        (false, _) => "disabled",
        (true, true) => "auto",
        (true, false) => "show",
    }
}
fn set_navigator(layout: &mut LayoutPreferences, key: &str) {
    layout.show_navigator = key != "disabled";
    layout.navigator_auto_hide = key == "auto";
}

fn ink(cx: &App) -> Ink {
    INK_OVERRIDE
        .get()
        .unwrap_or_else(|| theme_ink(cx.theme().is_dark()))
}
fn theme_ink(dark: bool) -> Ink {
    if dark {
        Ink {
            dark: true,
            background: rgb(0x1b1c20).into(),
            panel: rgb(0x202125).into(),
            line: rgb(0x34353a).into(),
            text: rgb(0xd9dadd).into(),
            muted: rgb(0x92949c).into(),
            accent: rgb(0xe3a34c).into(),
            favorite: rgb(0xe5484d).into(),
            flame: rgb(0xf76b15).into(),
            selection: rgb(0x38342e).into(),
        }
    } else {
        Ink {
            dark: false,
            background: rgb(0xfafafa).into(),
            panel: rgb(0xf0f0f2).into(),
            line: rgb(0xd8d9dc).into(),
            text: rgb(0x27282d).into(),
            muted: rgb(0x777980).into(),
            accent: rgb(0xa76312).into(),
            favorite: rgb(0xd1293d).into(),
            flame: rgb(0xdc4f0b).into(),
            selection: rgb(0xefe2cf).into(),
        }
    }
}
struct DownloadStart {
    release: String,
    /// The search result that was clicked.
    candidate: usize,
    username: String,
    title: String,
    /// The artist page's cache key and the artist as displayed.
    artist_key: String,
    artist_name: String,
    /// Why slskd refused; the results stay so another folder can be picked.
    failure: Option<String>,
}
fn download_caption(job: &DownloadJob) -> String {
    let progress = &job.progress;
    match job.state {
        JobState::Downloaded => "Downloaded · waiting for library scan".into(),
        JobState::Failed if progress.done > 0 => format!(
            "Stopped at {}/{} files · {}",
            progress.done, progress.total, progress.failure
        ),
        JobState::Failed => format!("Download failed · {}", progress.failure),
        JobState::Active if progress.done == 0 && progress.queued > 0 => match progress.place {
            Some(place) => format!("Queued · position {place}"),
            None => "Queued on the peer".into(),
        },
        JobState::Active if progress.active == 0 && progress.queued > 0 => format!(
            "{}/{} downloaded · {} waiting in the peer's queue{}",
            progress.done,
            progress.total,
            progress.queued,
            if job.requeued > 0 {
                format!(", asked again {}×", job.requeued)
            } else {
                String::new()
            }
        ),
        JobState::Active => format!("Downloading {}/{}", progress.done, progress.total),
    }
}

/// A spot at one end of the tab strip for a scroll arrow, covering the tabs beneath.
/// A short fade on its inner side lets the covered tab slip out of view.
fn edge(id: &'static str, background: Hsla, left: bool) -> Stateful<Div> {
    let clear = background.opacity(0.);
    let fade = div().absolute().top_0().bottom_0().w(px(20.));
    let fade = if left {
        fade.right(px(-20.)).bg(linear_gradient(
            90.,
            linear_color_stop(background, 0.),
            linear_color_stop(clear, 1.),
        ))
    } else {
        fade.left(px(-20.)).bg(linear_gradient(
            90.,
            linear_color_stop(clear, 0.),
            linear_color_stop(background, 1.),
        ))
    };
    div()
        .id(id)
        .absolute()
        .top_0()
        .bottom_0()
        .flex()
        .items_center()
        .bg(background)
        .child(fade)
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
    image_cache: Entity<crate::artwork_cache::ArtworkCache>,
    workspace: Workspace,
    view: LibraryView,
    query: LibraryQuery,
    explorer_query: LibraryQuery,
    request: u64,
    search: Entity<InputState>,
    search_text: String,
    queue_search: String,
    queue_ui: QueueViewState,
    titlebar_dragging: bool,
    /// The seek slider value (0–1000) while the user drags it.
    scrub: Option<f32>,
    workspace_scroll: ScrollHandle,
    server_url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    playlist_name: Entity<InputState>,
    smart: smart_playlists::SmartUi,
    slskd_url: Entity<InputState>,
    lastfm_user: Entity<InputState>,
    lastfm_key: Entity<InputState>,
    slskd_key: Entity<InputState>,
    slskd_username: Entity<InputState>,
    slskd_password: Entity<InputState>,
    soulseek_query: Entity<InputState>,
    slskd_status: Option<Result<String, String>>,
    discographies: HashMap<String, Arc<DiscographyView>>,
    selected_missing: Option<String>,
    missing_expanded: bool,
    release_details: HashMap<String, Result<Arc<ReleaseDetails>, String>>,
    release_details_requested: HashSet<String>,
    soulseek: Option<(String, SearchUpdate)>,
    soulseek_open: Option<usize>,
    /// A Download click slskd has not answered yet. Asking the peer for the
    /// whole folder and queueing it can take up to a minute.
    download_start: Option<DownloadStart>,
    downloads: Arc<Vec<DownloadJob>>,
    seek: Entity<SliderState>,
    volume: Entity<SliderState>,
    preferences: Preferences,
    volume_before_mute: Option<f32>,
    account: Option<Account>,
    sign_in: sign_in::SignIn,
    library_connection: sign_in::LibraryConnection,
    settings: bool,
    settings_save: crate::settings_save::SaveFeedback,
    /// The settings page to reopen when the settings view is rebuilt, as it
    /// is when the navigator is docked or undocked.
    settings_page: usize,
    tracks: Vec<Arc<Track>>,
    track_total: usize,
    loading_more: bool,
    group_tracks: HashMap<String, Vec<Arc<Track>>>,
    group_requested: HashSet<String>,
    album_list: ListState,
    cover_list: ListState,
    cover_columns: usize,
    cover_sections: Vec<usize>,
    alphabet_jump: Option<usize>,
    library_width: f32,
    library_height: f32,
    selected_album: Option<Arc<Album>>,
    pending_explorer_album: Option<String>,
    selected: Option<Arc<Track>>,
    selection: crate::selection::TrackSelection,
    playlists: crate::playlists::Playlists,
    playlist_album_list: ListState,
    playlist_track_scroll: ScrollHandle,
    playback: PlaybackSnapshot,
    account_cache: account_cache::AccountCache,
    /// When each missing cover was last asked for; repeats keep visible covers first in line.
    art_requested: HashMap<String, Instant>,
    /// First and last time each missing cover was on screen, to skip downloads for fly-bys.
    art_seen: HashMap<String, library::ArtSettle>,
    /// A missing cover is waiting to settle, so render again shortly to request it.
    art_wake: bool,
    /// When each neighboring cover was last queued for download ahead of scrolling.
    art_prefetched: HashMap<String, Instant>,
    /// Covers whose small copies exist, so tiles can skip decoding the full image.
    art_thumbs: HashSet<String>,
    /// The list and top row being prefetched around, and since when it has been still.
    prefetch_anchor: Option<(&'static str, usize, Instant)>,
    art_failures: HashSet<String>,
    lyrics: HashMap<String, Result<Arc<Lyrics>, String>>,
    lyrics_requested: HashSet<String>,
    lyrics_scroll: ScrollHandle,
    lyrics_follow: Option<(String, Option<usize>)>,
    /// Start time, start offset, and line index of an in-flight follow scroll.
    lyrics_scroll_motion: Option<(Instant, f32, usize)>,
    /// Frames left to re-anchor without animation once fresh layout is known.
    lyrics_snap: u8,
    /// Wakes the UI exactly when the next timed lyric line starts.
    lyrics_wake: Option<(usize, Task<()>)>,
    /// When the last playback snapshot arrived, to extrapolate position between them.
    playback_at: Instant,
    /// A pending paced redraw for animations that don't need display rate.
    /// Pending redraws, one per view, for animations below display rate.
    frame_timers: HashMap<EntityId, Task<()>>,
    /// The window's cached parts; see `sections`.
    sections: HashMap<Part, Entity<sections::Section>>,
    /// Set when the app itself changed, so this frame redraws every part.
    full_frame: bool,
    /// Saves the window frame once a move or resize settles.
    window_save: Option<Task<()>>,
    /// Part of every resizable group's ID. Panels keep the size they first
    /// rendered at, so a new value rebuilds them at the saved sizes.
    layout_generation: u64,
    artist_name: Option<String>,
    artist_id: String,
    artist_request: u64,
    discover_request: u64,
    discover: Option<Arc<crate::discover::Rediscover>>,
    discover_visit: crate::discover::Visit,
    artist_data: Option<ArtistPageData>,
    biography_expanded: bool,
    popular_expanded: bool,
    most_played_expanded: bool,
    artist_albums: Option<Vec<Arc<Album>>>,
    /// The artist page's rows, rendered only while on screen.
    artist_list: ListState,
    artist_rows: Rc<artist_page::Rows>,
    /// Biography line count for the last text and width measured.
    bio_lines: Option<(u64, u32, usize)>,
    artist_origin: Workspace,
    highlight_album: Option<(String, Vec<Arc<Track>>)>,
    highlight_requested: Option<String>,
    album_tracks_expanded: bool,
    up_next_expanded: bool,
    backdrop_shown: Option<String>,
    backdrop_previous: Option<String>,
    backdrop_changed: Instant,
    spectrum: Spectrum,
    spectrum_window: Box<[f32; FFT_SIZE]>,
    spectrum_tick: Option<Instant>,
    library_open: bool,
    playlists_open: bool,
    playlist_rename: Option<String>,
    playlist_creation: Option<playlist_creation::CreationDialog>,
    playlist_creation_focus: FocusHandle,
    tag_editor: Option<tag_editor::TagEditor>,
    /// Album whose songs the tag editor opens on when they arrive.
    tag_editor_album: Option<String>,
    album_deletion: Option<context_menu::AlbumDeletion>,
    album_deletion_focus: FocusHandle,
    tag_notice: Option<crate::tags::TagReport>,
    /// Tag edits handed to the service, oldest first, and the running one's progress.
    tag_jobs: crate::tags::TagJobs<tag_editor::TagEditor>,
    metadata_folder: Entity<InputState>,
    acoustid_key: Entity<InputState>,
    status: String,
    connection: Connection,
    error: Option<String>,
    feedback: crate::feedback::Feedback,
    focus: FocusHandle,
    artist_scroll: UniformListScrollHandle,
    track_scroll: UniformListScrollHandle,
    rating_hover: Option<(String, u8)>,
    information_width: f32,
    navigator_peek: Option<Task<()>>,
    /// The active tab's history; other tabs keep theirs in `histories`.
    /// `None` stands for the empty page a new tab starts on.
    history: History,
    histories: HashMap<u64, History>,
    /// The active tab is a new, empty tab waiting for a page from the navigator.
    blank: bool,
    /// Tabs are only saved once the stored preferences have been loaded.
    prefs_loaded: bool,
    tab_save: Option<Task<()>>,
    /// Closed tabs for Cmd-Shift-T, newest last: position, tab and history.
    closed_tabs: Vec<(usize, crate::browser::Tab, History)>,
    restoring_location: bool,
    _subscriptions: Vec<Subscription>,
}
impl MusicApp {
    /// Read-only navigation state for integration assertions. Mutations still
    /// go through the production input handlers and service events.
    #[cfg(feature = "test-support")]
    pub fn tab_state(&self) -> &crate::browser::Tabs {
        &self.preferences.tabs
    }

    pub fn new(
        service: Service,
        events: flume::Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search music"));
        let server_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://music.example.com"));
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("Username"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });
        let playlist_name = cx.new(|cx| InputState::new(window, cx).placeholder("Playlist name"));
        let metadata_folder =
            cx.new(|cx| InputState::new(window, cx).placeholder("/Volumes/music"));
        let acoustid_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your AcoustID application key")
                .masked(true)
        });
        let slskd_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://your-slskd-host:5030"));
        let lastfm_user = cx.new(|cx| InputState::new(window, cx).placeholder("Optional"));
        let lastfm_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Optional")
                .masked(true)
        });
        let slskd_key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Optional")
                .masked(true)
        });
        let slskd_username = cx.new(|cx| InputState::new(window, cx).placeholder("Optional"));
        let slskd_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Optional")
                .masked(true)
        });
        let soulseek_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Artist and release"));
        let seek = cx.new(|_| SliderState::new().min(0.).max(1000.).step(1.));
        let volume = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.01)
                .default_value(0.7)
        });
        let view = cx.entity().downgrade();
        let mut subscriptions = vec![
            cx.observe_window_bounds(window, |app, window, cx| app.remember_window(window, cx)),
            // Animation stops while hidden, so resume it when the window is shown again.
            window.observe_window_visibility(move |_, _, cx| {
                view.update(cx, |_, cx| cx.notify()).ok();
            }),
            cx.subscribe_in(&playlist_name, window, Self::playlist_name_event),
            cx.subscribe_in(&search, window, |app, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let value = app.search.read(cx).value().to_string();
                    if value != app.search_text {
                        app.search_text = value.clone();
                        if app.blank {
                            app.switch_workspace(Workspace::Explorer, window, cx);
                            app.search_text = value.clone();
                        }
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
                            Workspace::Playlists => app.playlists.set_search(value),
                            Workspace::Queue => app.queue_search = value,
                            Workspace::NowPlaying | Workspace::Discover => {
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
            cx.subscribe_in(&seek, window, |app, _, event: &SliderEvent, _, cx| {
                match event {
                    // While dragging, playback updates must not move the thumb
                    // under the pointer; the elapsed time previews the target.
                    SliderEvent::Change(SliderValue::Single(value)) => {
                        app.scrub = Some(*value);
                        cx.notify();
                    }
                    SliderEvent::Release(SliderValue::Single(value)) => {
                        app.scrub = None;
                        let target = *value as f64 / 1000. * app.playback.duration;
                        app.playback.position = target;
                        app.playback_at = Instant::now();
                        app.service.player.send(PlayerCommand::Seek(target));
                        cx.notify();
                    }
                    _ => {}
                }
            }),
            cx.subscribe_in(&volume, window, |app, _, event: &SliderEvent, _, _| {
                if let SliderEvent::Change(SliderValue::Single(value)) = event {
                    app.preferences.volume = *value;
                    app.volume_before_mute = None;
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
        let image_cache = crate::artwork_cache::ArtworkCache::new(cx);
        Self {
            service,
            image_cache,
            workspace: Workspace::Explorer,
            view: LibraryView::default(),
            query: query.clone(),
            explorer_query: query,
            request,
            search,
            search_text: String::new(),
            queue_search: String::new(),
            queue_ui: QueueViewState::new(cx),
            titlebar_dragging: false,
            scrub: None,
            workspace_scroll: ScrollHandle::new(),
            server_url,
            username,
            password,
            playlist_name,
            smart: Default::default(),
            slskd_url,
            lastfm_user,
            lastfm_key,
            slskd_key,
            slskd_username,
            slskd_password,
            soulseek_query,
            slskd_status: None,
            discographies: HashMap::new(),
            selected_missing: None,
            release_details: HashMap::new(),
            release_details_requested: HashSet::new(),
            missing_expanded: false,
            soulseek: None,
            soulseek_open: None,
            download_start: None,
            downloads: Arc::default(),
            seek,
            volume,
            preferences: Preferences::default(),
            volume_before_mute: None,
            account: None,
            sign_in: Default::default(),
            library_connection: Default::default(),
            settings: false,
            settings_save: Default::default(),
            settings_page: 0,
            tracks: vec![],
            track_total: 0,
            loading_more: false,
            group_tracks: HashMap::new(),
            group_requested: HashSet::new(),
            album_list: ListState::new(0, ListAlignment::Top, px(300.)),
            cover_list: ListState::new(0, ListAlignment::Top, px(200.)),
            cover_columns: 4,
            cover_sections: Vec::new(),
            alphabet_jump: None,
            library_height: 600.,
            library_width: 600.,
            selected_album: None,
            pending_explorer_album: None,
            selected: None,
            selection: Default::default(),
            playlists: Default::default(),
            playlist_album_list: ListState::new(0, ListAlignment::Top, px(200.)),
            playlist_track_scroll: ScrollHandle::new(),
            playback: PlaybackSnapshot::default(),
            account_cache: Default::default(),
            art_requested: HashMap::new(),
            art_seen: HashMap::new(),
            art_wake: false,
            art_prefetched: HashMap::new(),
            art_thumbs: HashSet::new(),
            prefetch_anchor: None,
            art_failures: HashSet::new(),
            lyrics: HashMap::new(),
            lyrics_requested: HashSet::new(),
            lyrics_scroll: ScrollHandle::new(),
            lyrics_follow: None,
            lyrics_scroll_motion: None,
            lyrics_snap: 0,
            lyrics_wake: None,
            playback_at: Instant::now(),
            frame_timers: HashMap::new(),
            sections: HashMap::new(),
            full_frame: true,
            window_save: None,
            layout_generation: 0,
            artist_name: None,
            artist_id: String::new(),
            artist_request: 0,
            discover_request: 0,
            discover: None,
            discover_visit: Default::default(),
            artist_data: None,
            biography_expanded: false,
            popular_expanded: false,
            most_played_expanded: false,
            artist_albums: None,
            artist_list: ListState::new(0, ListAlignment::Top, px(400.)),
            artist_rows: Rc::default(),
            bio_lines: None,
            artist_origin: Workspace::Explorer,
            highlight_album: None,
            highlight_requested: None,
            album_tracks_expanded: false,
            up_next_expanded: false,
            backdrop_shown: None,
            backdrop_previous: None,
            backdrop_changed: Instant::now(),
            spectrum: Spectrum::default(),
            spectrum_window: Box::new([0.; FFT_SIZE]),
            spectrum_tick: None,
            library_open: true,
            playlists_open: true,
            playlist_rename: None,
            playlist_creation: None,
            playlist_creation_focus: cx.focus_handle(),
            tag_editor: None,
            tag_editor_album: None,
            album_deletion: None,
            album_deletion_focus: cx.focus_handle(),
            tag_notice: None,
            tag_jobs: Default::default(),
            metadata_folder,
            acoustid_key,
            status: "Opening library…".into(),
            connection: Connection::None,
            error: None,
            feedback: Default::default(),
            focus,
            artist_scroll: UniformListScrollHandle::default(),
            track_scroll: UniformListScrollHandle::default(),
            rating_hover: None,
            information_width: 300.,
            navigator_peek: None,
            history: History::default(),
            histories: HashMap::new(),
            blank: false,
            prefs_loaded: false,
            tab_save: None,
            closed_tabs: Vec::new(),
            restoring_location: false,
            _subscriptions: {
                subscriptions.push(cx.observe_self(|app, _| app.full_frame = true));
                subscriptions
            },
        }
    }
}
impl Render for MusicApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The cache keeps covers drawn in the last full frame. Animation frames
        // reuse cached parts without drawing their covers, so they must not
        // advance it, or visible covers could be evicted.
        if std::mem::take(&mut self.full_frame) {
            self.image_cache
                .update(cx, |cache, cx| cache.begin_frame(window, cx));
        }
        self.sync_active_tab(window, cx);
        let colors = ink(cx);
        if !self.settings {
            self.settings_page = 0;
        }
        let body = if self.smart.editor.is_some() && self.playlist_creation.is_none() {
            self.smart_editor_view(cx)
        } else if self.workspace == Workspace::NowPlaying && !self.settings && !self.blank {
            self.now_playing_view(window, cx)
        } else {
            let floating = self.navigator_floats();
            let docked = (self.preferences.layout.show_navigator || self.blank) && !floating;
            let (left_width, right_width) = panel_widths(
                f32::from(window.viewport_size().width),
                docked.then_some(self.preferences.layout.left_width),
                (self.preferences.layout.show_information && !self.settings)
                    .then_some(self.preferences.layout.right_width),
            );
            self.information_width = right_width;
            let preferred_width = if docked {
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
            let information = self.preferences.layout.show_information && !self.settings;
            let float_width = self.preferences.layout.left_width.clamp(140., 380.);
            let peeking = floating && self.navigator_peek.is_some();
            let slide = if floating {
                transition(
                    "navigator-slide",
                    if peeking { 1_f32 } else { 0_f32 },
                    Transition::new(Duration::from_millis(180)),
                    window,
                    cx,
                )
                .clamp(0., 1.)
            } else {
                0.
            };
            let left = (docked || slide > 0.001)
                .then(|| self.part(Part::Navigator, style().size_full(), cx));
            let center = self.part(Part::Center, style().size_full(), cx);
            let right = information.then(|| self.information_sidebar(cx));
            let (docked_left, floating_left) = if docked { (left, None) } else { (None, left) };
            // Hidden panels keep stale sizes in a group's state and block dragging,
            // so each arrangement gets its own group with only the panels it shows.
            let right_ix = usize::from(docked) + 1;
            let workspace = h_resizable(SharedString::from(format!(
                "musicbee-workspace-{}-{}-{}",
                u8::from(docked),
                u8::from(information),
                self.layout_generation
            )))
            .on_resize(cx.listener(
                move |app,
                      state: &Entity<gpui_kit::component::resizable::ResizableState>,
                      _,
                      cx| {
                    let sizes = state.read(cx).sizes();
                    if docked
                        && let Some(size) = sizes.first().filter(|size| f32::from(**size) >= 140.)
                    {
                        app.preferences.layout.left_width = f32::from(*size);
                    }
                    if information
                        && let Some(size) =
                            sizes.get(right_ix).filter(|size| f32::from(**size) >= 180.)
                    {
                        app.preferences.layout.right_width = f32::from(*size);
                    }
                    app.save_preferences();
                },
            ))
            .when_some(docked_left, |group, left| {
                group.child(
                    resizable_panel()
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
            })
            .child(
                resizable_panel()
                    .size_range(px(300.)..px(2400.))
                    .child(center),
            )
            .when_some(right, |group, right| {
                group.child(
                    resizable_panel()
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
            });
            div()
                .size_full()
                .relative()
                .child(workspace)
                .when(floating && !peeking, |area| {
                    area.child(
                        div()
                            .id("navigator-edge")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(px(6.))
                            .on_hover(cx.listener(move |app, hovered: &bool, window, cx| {
                                if *hovered {
                                    app.peek_navigator(float_width, window, cx);
                                }
                            })),
                    )
                })
                .when_some(floating_left, |area, navigator| {
                    area.child(
                        div()
                            .id("navigator-float")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(-float_width * (1. - slide)))
                            .w(px(float_width))
                            .opacity(0.6 + 0.4 * slide)
                            .occlude()
                            .border_r_1()
                            .border_color(colors.line)
                            .shadow_lg()
                            .child(navigator),
                    )
                })
                .into_any_element()
        };
        let playlist_creation = self.playlist_creation_view(cx);
        let tag_editor = self.tag_editor_dialog(cx);
        let album_deletion = self.album_delete_dialog(cx);
        let tabs = self.part(
            Part::Tabs,
            style()
                .w_full()
                .h(px(sections::TABS_HEIGHT))
                .flex_shrink_0(),
            cx,
        );
        let status = self.part(
            Part::Status,
            style().w_full().h(px(21.)).flex_shrink_0(),
            cx,
        );
        let player = self.part(
            Part::Player,
            style().w_full().h(px(56.)).flex_shrink_0(),
            cx,
        );
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .key_context("Sonora")
            .track_focus(&self.focus)
            .font_family(crate::platform::UI_FONT)
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
            .on_action(cx.listener(|app, _: &GoForward, window, cx| {
                app.go_forward(window, cx);
                app.focus.focus(window, cx);
            }))
            .on_action(cx.listener(|app, _: &NewTab, window, cx| app.new_tab(window, cx)))
            .on_action(cx.listener(|app, _: &CloseTab, window, cx| {
                let ix = app.preferences.tabs.active;
                app.close_tab(ix, window, cx)
            }))
            .on_action(cx.listener(|app, _: &ResetTabs, window, cx| app.reset_tabs(window, cx)))
            .on_action(
                cx.listener(|app, _: &ReopenClosedTab, window, cx| {
                    app.reopen_closed_tab(window, cx)
                }),
            )
            .on_action(cx.listener(|app, _: &SelectTab1, w, cx| app.select_tab_number(1, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab2, w, cx| app.select_tab_number(2, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab3, w, cx| app.select_tab_number(3, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab4, w, cx| app.select_tab_number(4, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab5, w, cx| app.select_tab_number(5, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab6, w, cx| app.select_tab_number(6, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab7, w, cx| app.select_tab_number(7, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab8, w, cx| app.select_tab_number(8, w, cx)))
            .on_action(cx.listener(|app, _: &SelectTab9, w, cx| app.select_tab_number(9, w, cx)))
            .on_action(cx.listener(|app, _: &GoBack, window, cx| {
                if app.settings {
                    app.settings = false;
                } else if app.history.can_go_back() {
                    app.go_back(window, cx);
                } else if app.blank {
                    // An empty tab has nowhere to go back to.
                } else if app.artist_name.is_some() {
                    let origin = app.artist_origin;
                    app.switch_workspace(origin, window, cx);
                } else if app.workspace == Workspace::Playlists {
                    app.playlists.close();
                } else {
                    app.query.selection = Default::default();
                    app.query.genre.clear();
                    app.query.artist.clear();
                    app.query.performer.clear();
                    app.query.year = None;
                    app.query.record_label.clear();
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
                app.restore_layout(cx);
                app.save_preferences();
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
            .child(tabs)
            .children(self.playlist_delete_confirmation(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .relative()
                    .overflow_hidden()
                    .child(body)
                    .children(self.settings_save_toast(cx)),
            )
            .child(status)
            .child(player)
            .children(playlist_creation)
            .children(tag_editor)
            .children(album_deletion)
    }
}
