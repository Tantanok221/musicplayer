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
use sonora::sorting::{Grouping, Preset, Scope as SortScope, SortSpec};
use sonora::{
    artists::{
        ArtistPageData, ArtistProfile, ReleaseHit, TopSongs, cache_key, lastfm_url, profile_url,
        search_releases,
    },
    artwork::Priority,
    backdrop::Backdrop,
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
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};
mod album_feedback;
mod downloads;
mod queue_controls;
use queue_controls::QueueViewState;
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

const POPULAR_PER_ALBUM: usize = 3;

/// A page Back and Forward can return to.
#[derive(Clone, PartialEq)]
struct Location {
    workspace: Workspace,
    artist: Option<(String, String)>,
    query: LibraryQuery,
}
const HISTORY_LIMIT: usize = 50;

/// Clicking the current rating clears it; any other star sets that rating.
fn rating_after_click(rating: u8, clicked: u8) -> u8 {
    if clicked == rating { 0 } else { clicked }
}

const ARTIST_SONG_COLUMN_MIN: f32 = 300.;
const ALBUM_PREVIEW_ROWS: usize = 6;
const UP_NEXT_PREVIEW_ROWS: usize = 8;
const BACKDROP_FADE_SECONDS: f32 = 0.7;
const SPECTRUM_HEIGHT: f32 = 84.;
/// Fraction of the lyrics viewport height where the current line rests.
const LYRIC_ANCHOR: f32 = 0.4;
const LYRIC_SCROLL_SECONDS: f32 = 0.6;
/// How often a cover that is still on screen re-asks to move to the front of the download queue.
const ART_REPRIORITIZE: Duration = Duration::from_millis(750);
/// A cover must stay on screen this long before it is downloaded, so fast scrolling
/// doesn't fill the download slots with covers that are already gone.
const ART_SETTLE: Duration = Duration::from_millis(120);
/// Rows of covers above and below the viewport downloaded ahead of scrolling.
const PREFETCH_ROWS: usize = 2;
/// Albums above and below the top of the album-and-tracks list downloaded ahead.
const PREFETCH_ALBUMS: std::ops::Range<isize> = -3..8;
/// The spectrum redraws at 60 fps; display rate (up to 120 Hz) costs twice the CPU for no visible gain.
const SPECTRUM_FRAME: Duration = Duration::from_micros(16_667);

/// Low and high bar colors: the album's two leading colors, lifted so they
/// read against the scrim, or the theme accent before a palette exists.
/// Three breathing dots marking an instrumental break in timed lyrics.
fn lyric_gap(index: usize, text_size: f32, leading: f32, color: Hsla) -> impl IntoElement {
    let side = (text_size * 0.32).round();
    div()
        .h(px(leading))
        .flex()
        .items_center()
        .gap(px(side * 0.8))
        .children((0..3).map(move |dot| {
            div()
                .size(px(side))
                .rounded_full()
                .bg(color)
                .with_animation(
                    SharedString::from(format!("lyric-gap-{index}-{dot}")),
                    Animation::new(Duration::from_millis(1600)).repeat(),
                    move |element, t| {
                        let phase = (t - dot as f32 * 0.16).rem_euclid(1.);
                        let wave = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
                        element.opacity(0.25 + 0.75 * wave)
                    },
                )
        }))
}

fn spectrum_colors(palette: Option<[u32; 3]>, accent: Hsla, dark: bool) -> (Hsla, Hsla) {
    let vivid = |color: u32| {
        let mut color = Hsla::from(rgb(color));
        color.s = color.s.max(0.45);
        color.l = if dark {
            color.l.clamp(0.55, 0.72)
        } else {
            color.l.clamp(0.36, 0.5)
        };
        color
    };
    match palette {
        Some([first, second, _]) => (vivid(first), vivid(second)),
        None => (accent, accent),
    }
}

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

/// Play Next, Play After This Album, and Add to End of Queue, each with a
/// second line naming what the music will follow.
fn queue_options(
    menu: PopupMenu,
    places: &QueuePlaces,
    enqueue: impl Fn(Placement) + 'static,
) -> PopupMenu {
    let enqueue = Rc::new(enqueue);
    let mut options = vec![(
        Placement::Next,
        "Play Next",
        places
            .current_track
            .as_deref()
            .map_or_else(|| "At the start of the queue".to_string(), quoted),
    )];
    if let Some(album) = &places.current_album {
        options.push((
            Placement::AfterAlbum,
            "Play After This Album",
            quoted(album),
        ));
    }
    options.push((
        Placement::Last,
        "Add to End of Queue",
        places
            .last_album
            .as_deref()
            .map_or_else(|| "The queue is empty".to_string(), quoted),
    ));
    options
        .into_iter()
        .fold(menu, |menu, (placement, title, detail)| {
            let enqueue = enqueue.clone();
            let detail = SharedString::from(detail);
            menu.item(
                PopupMenuItem::element(move |_, cx| {
                    div().flex().flex_col().child(title).child(
                        div()
                            .text_size(px(11.))
                            .text_color(ink(cx).muted)
                            .child(detail.clone()),
                    )
                })
                .on_click(move |_, _, _| enqueue(placement)),
            )
        })
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

/// Wrapped line count of the artist biography at its 11px system font size.
fn biography_lines(text: &str, width: f32, cx: &App) -> usize {
    if width <= 0. {
        return 0;
    }
    let mut wrapper = cx
        .text_system()
        .line_wrapper(font(sonora::platform::UI_FONT), px(11.));
    text.lines()
        .map(|line| {
            1 + wrapper
                .wrap_line(&[LineFragment::text(line)], px(width), Default::default())
                .count()
        })
        .sum()
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
}
fn gain_index(mode: &GainMode) -> usize {
    match mode {
        GainMode::Smart => 0,
        GainMode::Track => 1,
        GainMode::Album => 2,
        GainMode::Off => 3,
    }
}
fn gain_from_index(index: usize) -> GainMode {
    match index {
        1 => GainMode::Track,
        2 => GainMode::Album,
        3 => GainMode::Off,
        _ => GainMode::Smart,
    }
}
fn gain_description(mode: &GainMode) -> &'static str {
    match mode {
        GainMode::Smart => {
            "Album gain while the queue is a single album; track gain once it mixes albums."
        }
        GainMode::Track => "Every track plays at the same loudness.",
        GainMode::Album => {
            "Albums play at the same loudness, keeping quiet and loud songs within them."
        }
        GainMode::Off => "Files play at their original level.",
    }
}
fn info_source_key(source: InfoSource) -> &'static str {
    match source {
        InfoSource::Selected => "selected",
        InfoSource::Playing => "playing",
    }
}
fn info_tab_key(tab: InfoTab) -> &'static str {
    match tab {
        InfoTab::Details => "details",
        InfoTab::Lyrics => "lyrics",
        InfoTab::Artwork => "artwork",
    }
}
fn credential_field(
    title: &'static str,
    input: Entity<InputState>,
    description: Option<&'static str>,
    keywords: &'static [&'static str],
) -> SettingItem {
    let item = SettingItem::new(
        title,
        SettingField::render(move |options, _, _| {
            let field = Input::new(&input).with_size(options.size());
            if options.layout() == Axis::Vertical {
                field.w_full()
            } else {
                field.w_64()
            }
        }),
    )
    .keywords(keywords.iter().copied());
    match description {
        Some(description) => item.description(description),
        None => item,
    }
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

fn layout_switch(
    app: WeakEntity<MusicApp>,
    title: &'static str,
    description: &'static str,
    default_value: bool,
    disabled: bool,
    get: fn(&LayoutPreferences) -> bool,
    set: fn(&mut LayoutPreferences, bool),
) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::switch(
            {
                let app = app.clone();
                move |cx: &App| {
                    app.read_with(cx, |app, _| get(&app.preferences.layout))
                        .unwrap_or(default_value)
                }
            },
            move |value, cx: &mut App| {
                let _ = app.update(cx, |app, cx| {
                    set(&mut app.preferences.layout, value);
                    app.settings_page = LAYOUT_SETTINGS_PAGE;
                    app.save_preferences();
                    cx.notify();
                });
            },
        )
        .default_value(default_value),
    )
    .description(description)
    .disabled(disabled)
}
fn layout_choice<const N: usize>(
    app: WeakEntity<MusicApp>,
    title: &'static str,
    description: &'static str,
    options: [(&'static str, &'static str); N],
    default_key: &'static str,
    current: &'static str,
    apply: fn(&mut LayoutPreferences, &str),
) -> SettingItem {
    let selected = options.iter().position(|(key, _)| *key == current);
    let keys: Vec<&'static str> = options.iter().map(|(key, _)| *key).collect();
    let labels: Vec<&'static str> = options.iter().map(|(_, label)| *label).collect();
    SettingItem::new(
        title,
        SettingField::render({
            let app = app.clone();
            move |_, _, _| {
                let app = app.clone();
                let keys = keys.clone();
                let mut group = RadioGroup::vertical(title).selected_index(selected);
                for (index, label) in labels.iter().copied().enumerate() {
                    group = group.child(Radio::new((title, index)).label(label));
                }
                group.on_change(move |index, _, cx| {
                    let Some(key) = keys.get(*index).copied() else {
                        return;
                    };
                    let _ = app.update(cx, |app, cx| {
                        apply(&mut app.preferences.layout, key);
                        app.settings_page = LAYOUT_SETTINGS_PAGE;
                        app.save_preferences();
                        cx.notify();
                    });
                })
            }
        })
        .on_reset(
            move |_: &App| current != default_key,
            move |_, cx| {
                let _ = app.update(cx, |app, cx| {
                    apply(&mut app.preferences.layout, default_key);
                    app.settings_page = LAYOUT_SETTINGS_PAGE;
                    app.save_preferences();
                    cx.notify();
                });
            },
        ),
    )
    .description(description)
    .layout(Axis::Vertical)
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
            favorite: rgb(0xe5484d).into(),
            flame: rgb(0xf76b15).into(),
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
            favorite: rgb(0xd1293d).into(),
            flame: rgb(0xdc4f0b).into(),
            selection: rgb(0xefe2cf).into(),
        }
    }
}
fn size_label(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1e9 {
        format!("{:.2} GB", bytes / 1e9)
    } else if bytes >= 1e6 {
        format!("{:.0} MB", bytes / 1e6)
    } else {
        format!("{:.0} kB", bytes / 1e3)
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
    playlist_search: String,
    queue_search: String,
    queue_ui: QueueViewState,
    titlebar_dragging: bool,
    workspace_scroll: ScrollHandle,
    server_url: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    playlist_name: Entity<InputState>,
    slskd_url: Entity<InputState>,
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
    settings: bool,
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
    playlists: Vec<Playlist>,
    playlist: Option<Playlist>,
    playback: PlaybackSnapshot,
    artwork: HashMap<String, PathBuf>,
    /// When each missing cover was last asked for; repeats keep visible covers first in line.
    art_requested: HashMap<String, Instant>,
    /// First and last time each missing cover was on screen, to skip downloads for fly-bys.
    art_seen: HashMap<String, (Instant, Instant)>,
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
    frame_timer: Option<Task<()>>,
    artist_name: Option<String>,
    artist_id: String,
    artist_request: u64,
    artist_data: Option<ArtistPageData>,
    artist_profiles: HashMap<String, Arc<ArtistProfile>>,
    profile_requested: HashSet<String>,
    popularity_requested: HashSet<String>,
    top_songs: HashMap<String, Arc<TopSongs>>,
    biography_expanded: bool,
    popular_expanded: bool,
    most_played_expanded: bool,
    artist_albums: Option<Vec<Arc<Album>>>,
    artist_origin: Workspace,
    highlight_album: Option<(String, Vec<Arc<Track>>)>,
    highlight_requested: Option<String>,
    album_tracks_expanded: bool,
    up_next_expanded: bool,
    backdrops: HashMap<String, Arc<Backdrop>>,
    backdrop_requested: HashSet<String>,
    backdrop_shown: Option<String>,
    backdrop_previous: Option<String>,
    backdrop_changed: Instant,
    spectrum: Spectrum,
    spectrum_window: Box<[f32; FFT_SIZE]>,
    spectrum_tick: Option<Instant>,
    library_open: bool,
    playlists_open: bool,
    show_playlist_editor: bool,
    playlist_editor_rename: bool,
    playlist_editor_tracks: Option<Vec<String>>,
    pending_delete: Option<String>,
    status: String,
    connection: Connection,
    error: Option<String>,
    pending_mutation: Option<Mutation>,
    album_pending: HashSet<String>,
    focus: FocusHandle,
    artist_scroll: UniformListScrollHandle,
    track_scroll: UniformListScrollHandle,
    rating_hover: Option<(String, u8)>,
    information_width: f32,
    navigator_peek: Option<Task<()>>,
    history_back: Vec<Location>,
    history_forward: Vec<Location>,
    restoring_location: bool,
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
        let slskd_url =
            cx.new(|cx| InputState::new(window, cx).placeholder("http://your-slskd-host:5030"));
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
        let subscriptions = vec![
            // Animation stops while hidden, so resume it when the window is shown again.
            window.observe_window_visibility(move |_, _, cx| {
                view.update(cx, |_, cx| cx.notify()).ok();
            }),
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
            playlist_search: String::new(),
            queue_search: String::new(),
            queue_ui: QueueViewState::new(cx),
            titlebar_dragging: false,
            workspace_scroll: ScrollHandle::new(),
            server_url,
            username,
            password,
            playlist_name,
            slskd_url,
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
            settings: false,
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
            playlists: vec![],
            playlist: None,
            playback: PlaybackSnapshot::default(),
            artwork: HashMap::new(),
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
            frame_timer: None,
            artist_name: None,
            artist_id: String::new(),
            artist_request: 0,
            artist_data: None,
            artist_profiles: HashMap::new(),
            profile_requested: HashSet::new(),
            popularity_requested: HashSet::new(),
            top_songs: HashMap::new(),
            biography_expanded: false,
            popular_expanded: false,
            most_played_expanded: false,
            artist_albums: None,
            artist_origin: Workspace::Explorer,
            highlight_album: None,
            highlight_requested: None,
            album_tracks_expanded: false,
            up_next_expanded: false,
            backdrops: HashMap::new(),
            backdrop_requested: HashSet::new(),
            backdrop_shown: None,
            backdrop_previous: None,
            backdrop_changed: Instant::now(),
            spectrum: Spectrum::default(),
            spectrum_window: Box::new([0.; FFT_SIZE]),
            spectrum_tick: None,
            library_open: true,
            playlists_open: true,
            show_playlist_editor: false,
            playlist_editor_rename: false,
            playlist_editor_tracks: None,
            pending_delete: None,
            status: "Opening library…".into(),
            connection: Connection::None,
            error: None,
            pending_mutation: None,
            album_pending: HashSet::new(),
            focus,
            artist_scroll: UniformListScrollHandle::default(),
            track_scroll: UniformListScrollHandle::default(),
            rating_hover: None,
            information_width: 300.,
            navigator_peek: None,
            history_back: vec![],
            history_forward: vec![],
            restoring_location: false,
            _subscriptions: subscriptions,
        }
    }
    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Ready(account, prefs) => {
                self.account = account.clone();
                self.settings = account.is_none();
                self.preferences = prefs;
                self.album_pending.clear();
                self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
                self.explorer_query.ordering = self.query.ordering.clone();
                self.request_query(cx);
                self.apply_theme(window, cx);
                self.volume.update(cx, |slider, cx| {
                    slider.set_value(self.preferences.volume, window, cx)
                });
                let slskd = self.preferences.downloads.url.clone();
                self.slskd_url
                    .update(cx, |input, cx| input.set_value(slskd, window, cx));
                self.discographies.clear();
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
                    self.backdrops.clear();
                    self.backdrop_requested.clear();
                    self.artist_profiles.clear();
                    self.profile_requested.clear();
                    self.popularity_requested.clear();
                    self.top_songs.clear();
                    self.artist_request += 1;
                    self.artist_name = None;
                    self.artist_albums = None;
                }
                self.account = Some(account);
                self.settings = false;
                self.status = label;
                self.error = None;
                self.art_requested.clear();
                self.art_seen.clear();
                self.art_prefetched.clear();
                self.art_thumbs.clear();
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
                    if let Some(name) = &self.artist_name {
                        self.service.send(Command::Artist(
                            self.artist_request,
                            name.clone(),
                            self.artist_id.clone(),
                        ));
                    }

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
                    let sections = self
                        .query
                        .ordering
                        .as_ref()
                        .filter(|s| s.grouping != Grouping::None)
                        .map(|spec| {
                            view.albums
                                .iter()
                                .enumerate()
                                .filter_map(|(i, a)| {
                                    (i == 0
                                        || spec.group_label(&view.albums[i - 1])
                                            != spec.group_label(a))
                                    .then_some(i)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let reset = ids != old || self.cover_sections != sections;
                    self.cover_sections = sections;
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
                    if let Some(id) = self.pending_explorer_album.take()
                        && let Some(album) = self
                            .view
                            .albums
                            .iter()
                            .find(|album| album.id == id)
                            .cloned()
                    {
                        self.select_album(album, cx);
                    }
                }
            }
            Event::Page(request, revision, offset, tracks) => {
                if request == self.request
                    && revision == self.view.revision
                    && offset == self.tracks.len()
                {
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
            Event::Artist(request, data) => {
                if self.artist_name.is_some() && self.artist_request == request {
                    self.artist_albums = Some(data.albums.clone());
                    self.artist_data = Some(data);
                }
            }
            Event::ArtistProfile(key, profile) => {
                self.artist_profiles.insert(key, profile);
            }
            Event::TopSongs(key, top) => {
                self.top_songs.insert(key, top);
            }
            Event::Discography(key, view) => {
                self.discographies.insert(key, view);
            }
            Event::SoulseekSearch(release, update) => {
                if self.selected_missing.as_ref() == Some(&release) {
                    self.soulseek = Some((release, update));
                    self.soulseek_open = None;
                }
            }
            Event::ReleaseDetails(group, details) => {
                self.release_details.insert(group, details);
            }
            Event::Downloads(jobs) => {
                if self.download_start.as_ref().is_some_and(|start| {
                    start.failure.is_none() && jobs.iter().any(|job| job.release == start.release)
                }) {
                    self.download_start = None;
                }
                self.downloads = jobs;
            }
            Event::DownloadNotStarted(release, reason) => {
                if let Some(start) = self
                    .download_start
                    .as_mut()
                    .filter(|start| start.release == release)
                {
                    start.failure = Some(reason);
                }
            }
            Event::SlskdStatus(status) => self.slskd_status = Some(status),
            Event::Album(id, tracks) => {
                if self.highlight_requested.as_ref() == Some(&id) {
                    self.highlight_album = Some((id, tracks));
                }
            }
            Event::Playlists(playlists) => {
                if let Some(current) = &mut self.playlist {
                    if let Some(summary) = playlists.iter().find(|p| p.id == current.id) {
                        current.name.clone_from(&summary.name);
                        current.song_count = summary.song_count;
                    } else {
                        self.playlist = None;
                    }
                }
                self.playlists = playlists
                    .into_iter()
                    .map(|mut playlist| {
                        playlist.entry = Arc::default();
                        playlist
                    })
                    .collect();
            }
            Event::Playlist(playlist) => {
                if self.playlist.as_ref().is_some_and(|p| p.id == playlist.id) {
                    self.playlist = Some(playlist);
                }
            }
            Event::TrackUpdated(track) => self.apply_track_update(track),
            Event::AlbumUpdated(album) => self.apply_album_update(album),
            Event::Artwork(id, path, thumbs) => {
                self.art_failures.remove(&id);
                if thumbs {
                    self.art_thumbs.insert(id.clone());
                }
                self.artwork.insert(id, path);
            }
            Event::Backdrop(id, backdrop) => {
                self.backdrops.insert(id, backdrop);
            }
            Event::ArtworkFailed(id) => {
                self.art_failures.insert(id);
            }
            Event::ArtworkRetrying => {
                self.art_failures.clear();
                self.profile_requested.clear();
                self.popularity_requested.clear();
                if let Some(name) = &self.artist_name {
                    self.service.send(Command::ArtistProfile(
                        name.clone(),
                        self.artist_id.clone(),
                        true,
                    ));
                }
            }
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
                self.playback_at = Instant::now();
                self.queue_ui.retain_selection(&self.playback.queue_ids);
            }
            Event::Status(status) => self.status = status,
            Event::Connection(connection) => self.connection = connection,
            Event::Error(error) => {
                self.error = Some(error);
                self.loading_more = false;
            }
            Event::MutationDone(mutation) => {
                if let Some(id) = mutation.album_feedback_id() {
                    self.album_pending.remove(id);
                }
                if self.pending_mutation.as_ref() == Some(&mutation) {
                    self.pending_mutation = None;
                    self.error = None;
                }
                if matches!(
                    mutation,
                    Mutation::CreatePlaylist(..)
                        | Mutation::RenamePlaylist(..)
                        | Mutation::ReplacePlaylist(..)
                ) {
                    self.show_playlist_editor = false;
                }
                self.status = "Saved to Navidrome".into();
            }
            Event::MutationFailed(mutation, error) => {
                if let Some(id) = mutation.album_feedback_id() {
                    self.album_pending.remove(id);
                }
                self.pending_mutation = Some(mutation);
                self.error = Some(error);
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
        // `Theme::update` copies edited colors into the token layer. The settings
        // sidebar and links read those roles; a `global_mut` edit would leave them
        // on the stock theme.
        Theme::update(cx, |theme| {
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
            theme.colors.sidebar = colors.panel;
            theme.colors.sidebar_foreground = colors.text;
            theme.colors.sidebar_border = colors.line;
            theme.colors.sidebar_accent = colors.selection;
            theme.colors.sidebar_accent_foreground = colors.text;
            theme.colors.sidebar_primary = colors.accent;
            theme.colors.sidebar_primary_foreground = colors.background;
            theme.colors.link = colors.accent;
            theme.colors.link_hover = colors.accent;
            theme.colors.link_active = colors.accent;
        });
    }
    fn restore_layout(&mut self, cx: &mut Context<Self>) {
        self.preferences.layout = LayoutPreferences::default();
        self.save_preferences();
        self.album_list.reset(self.album_row_count());
        cx.notify();
    }
    fn save_preferences(&self) {
        self.service
            .send(Command::Preferences(self.preferences.clone()));
    }
    fn request_query(&mut self, cx: &mut Context<Self>) {
        self.pending_explorer_album = None;
        self.query.favorite_filter = self.preferences.favorites.clone();
        if self.is_library() {
            self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
            self.explorer_query = self.query.clone();
        }
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
        CoverRows::grouped(
            self.view.albums.len(),
            self.cover_columns,
            self.alphabet_jump,
            &self.cover_sections,
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
        self.query.ordering = Some(self.preferences.sorting.get(self.sort_scope()));
        self.explorer_query.ordering = self.query.ordering.clone();
        self.request_query(cx);
        self.album_list.reset(self.album_row_count());
        self.cover_list
            .reset(self.cover_rows().row_count() + usize::from(self.alphabet_jump.is_some()));
        self.save_preferences();
        cx.notify();
    }
    fn location(&self) -> Location {
        Location {
            workspace: self.workspace,
            artist: self
                .artist_name
                .clone()
                .map(|name| (name, self.artist_id.clone())),
            query: if self.workspace == Workspace::Explorer {
                self.query.clone()
            } else {
                LibraryQuery::default()
            },
        }
    }
    /// Record the current page before navigating away from it.
    fn remember_location(&mut self) {
        if self.restoring_location {
            return;
        }
        let location = self.location();
        if self.history_back.last() != Some(&location) {
            self.history_back.push(location);
            if self.history_back.len() > HISTORY_LIMIT {
                self.history_back.remove(0);
            }
        }
        self.history_forward.clear();
    }
    fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(location) = self.history_back.pop() {
            self.history_forward.push(self.location());
            self.restore_location(location, window, cx);
        }
    }
    fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(location) = self.history_forward.pop() {
            self.history_back.push(self.location());
            self.restore_location(location, window, cx);
        }
    }
    fn restore_location(
        &mut self,
        location: Location,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restoring_location = true;
        if let Some((name, id)) = location.artist {
            self.open_artist(name, id, window, cx);
        } else {
            self.switch_workspace(location.workspace, window, cx);
            if location.workspace == Workspace::Explorer {
                self.search_text = location.query.text.clone();
                self.search.update(cx, |input, cx| {
                    input.set_value(self.search_text.clone(), window, cx)
                });
                self.explorer_query = location.query.clone();
                self.query = location.query;
                self.request_query(cx);
            }
        }
        self.restoring_location = false;
        cx.notify();
    }
    fn switch_workspace(
        &mut self,
        workspace: Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if workspace != self.workspace || self.artist_name.is_some() {
            self.remember_location();
        }
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
        if workspace == Workspace::Queue {
            self.queue_ui.focus.focus(window, cx);
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
        self.selected_missing = None;
        self.soulseek = None;
        self.request_group(&album.id);
        self.request_popularity(&album.artist_credits());
        for index in [old, current].into_iter().flatten() {
            let row = self.cover_rows().row_for_album(index);
            self.cover_list.splice(row..row + 1, 1);
        }
        cx.notify();
    }
    /// Popularity arrives later through `Event::TopSongs`; selection never waits for it.
    fn request_popularity(&mut self, artists: &[NamedId]) {
        for artist in artists {
            if self
                .popularity_requested
                .insert(cache_key(&artist.name, &artist.id))
            {
                self.service
                    .send(Command::TopSongs(artist.name.clone(), artist.id.clone()));
            }
        }
    }
    fn popular_tracks(&self, album_id: &str, artists: &[NamedId]) -> HashSet<String> {
        let mut popular = HashSet::new();
        for top in artists
            .iter()
            .filter_map(|artist| self.top_songs.get(&cache_key(&artist.name, &artist.id)))
        {
            for id in top.for_album(album_id, POPULAR_PER_ALBUM) {
                if popular.len() < POPULAR_PER_ALBUM {
                    popular.insert(id);
                }
            }
        }
        popular
    }
    fn navigator_floats(&self) -> bool {
        self.preferences.layout.show_navigator && self.preferences.layout.navigator_auto_hide
    }
    /// Hide by pointer position rather than hover, so the navigator's own menus,
    /// which render outside its element tree, don't dismiss it.
    fn peek_navigator(&mut self, width: f32, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigator_peek.is_some() {
            return;
        }
        self.navigator_peek = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let away = this
                    .update_in(cx, |app, window, cx| {
                        let away = !app.navigator_floats()
                            || f32::from(window.mouse_position().x) > width + 12.;
                        if away {
                            app.navigator_peek = None;
                            cx.notify();
                        }
                        away
                    })
                    .unwrap_or(true);
                if away {
                    break;
                }
            }
        }));
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
    fn request_release_details(&mut self, group: &str) {
        if self.release_details_requested.insert(group.into()) {
            self.service.send(Command::ReleaseDetails(group.into()));
        }
    }
    /// Replaces a track everywhere it is shown, without reloading any list.
    fn apply_track_update(&mut self, track: Arc<Track>) {
        if let Some(data) = &mut self.artist_data {
            for value in &mut data.most_played {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        for profile in self.artist_profiles.values_mut() {
            if profile.popular.iter().any(|value| value.id == track.id) {
                for value in &mut Arc::make_mut(profile).popular {
                    if value.id == track.id {
                        *value = track.clone();
                    }
                }
            }
        }
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
            for value in Arc::make_mut(&mut playlist.entry) {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        if self.playback.queue.iter().any(|value| value.id == track.id) {
            for value in Arc::make_mut(&mut self.playback.queue) {
                if value.id == track.id {
                    *value = track.clone();
                }
            }
        }
        if self
            .playback
            .track
            .as_ref()
            .is_some_and(|t| t.id == track.id)
        {
            self.playback.track = Some(track);
        }
    }
    fn find_track(&self, id: &str) -> Option<Arc<Track>> {
        self.selected
            .iter()
            .chain(&self.tracks)
            .chain(self.group_tracks.values().flatten())
            .chain(self.highlight_album.iter().flat_map(|(_, tracks)| tracks))
            .chain(self.playback.queue.iter())
            .chain(
                self.playlist
                    .iter()
                    .flat_map(|playlist| playlist.entry.iter()),
            )
            .chain(self.artist_data.iter().flat_map(|data| &data.most_played))
            .chain(
                self.artist_profiles
                    .values()
                    .flat_map(|profile| &profile.popular),
            )
            .find(|track| track.id == id)
            .cloned()
    }
    fn find_album(&self, id: &str) -> Option<Arc<Album>> {
        self.selected_album
            .iter()
            .chain(&self.view.albums)
            .chain(self.artist_albums.iter().flatten())
            .chain(self.artist_data.iter().flat_map(|data| &data.albums))
            .find(|album| album.id == id)
            .cloned()
    }
    fn apply_album_update(&mut self, album: Arc<Album>) {
        if self
            .selected_album
            .as_ref()
            .is_some_and(|selected| selected.id == album.id)
        {
            self.selected_album = Some(album.clone());
        }
        for value in self
            .view
            .albums
            .iter_mut()
            .chain(self.artist_albums.iter_mut().flatten())
            .chain(
                self.artist_data
                    .iter_mut()
                    .flat_map(|data| &mut data.albums),
            )
        {
            if value.id == album.id {
                *value = album.clone();
            }
        }
    }
    fn mutate(&mut self, mutation: Mutation, cx: &mut Context<Self>) {
        if let Some(id) = mutation.album_feedback_id() {
            if !self.album_pending.insert(id.into()) {
                return;
            }
            if let Some(album) = self.find_album(id) {
                let mut edited = (*album).clone();
                match &mutation {
                    Mutation::AlbumFavorite(_, favorite) => {
                        edited.starred = favorite.then(|| chrono::Utc::now().to_rfc3339())
                    }
                    Mutation::AlbumRating(_, rating) => edited.user_rating = Some(*rating),
                    _ => (),
                }
                self.apply_album_update(Arc::new(edited));
            }
        }
        // Show ratings and favorites at once; a failed save restores the
        // catalog value through the corresponding track/album update event.
        let edited = match &mutation {
            Mutation::Favorite(id, value) => self.find_track(id).map(|track| {
                let mut track = (*track).clone();
                track.starred = value.then(|| chrono::Utc::now().to_rfc3339());
                track
            }),
            Mutation::Rating(id, rating) => self.find_track(id).map(|track| {
                let mut track = (*track).clone();
                track.user_rating = Some(*rating);
                track
            }),
            _ => None,
        };
        if let Some(track) = edited.map(Arc::new) {
            self.service
                .player
                .send(PlayerCommand::UpdateTrack(track.clone()));
            self.apply_track_update(track);
        }
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
        self.remember_location();
        let origin = if self.artist_name.is_some() {
            self.artist_origin
        } else {
            self.workspace
        };
        self.switch_workspace(Workspace::Explorer, window, cx);
        self.artist_origin = origin;
        self.artist_name = Some(name.clone());
        self.artist_id = id.clone();
        self.artist_request += 1;
        self.artist_data = None;
        self.biography_expanded = false;
        self.popular_expanded = false;
        self.most_played_expanded = false;
        self.artist_albums = None;
        self.selected_album = None;
        self.selected_missing = None;
        self.missing_expanded = false;
        self.soulseek = None;
        self.group_tracks.clear();
        self.group_requested.clear();
        self.search_text.clear();
        self.search
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.service
            .send(Command::Artist(self.artist_request, name, id));
        cx.notify();
    }
    fn open_explorer_filter(
        &mut self,
        filter: ExplorerFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_location();
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
            ExplorerFilter::RecordLabel(label) => label.clone(),
        };
        let kind = match &filter {
            ExplorerFilter::Genre(_) => "genre",
            ExplorerFilter::Year(_) => "year",
            ExplorerFilter::RecordLabel(_) => "record label",
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
        if !self.query.record_label.is_empty() {
            bar = bar.child(
                Button::new("clear-record-label-filter")
                    .ghost()
                    .compact()
                    .label(format!("Label: {} ×", self.query.record_label))
                    .accessibility_label(format!(
                        "Clear record label {} filter",
                        self.query.record_label
                    ))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.query.record_label.clear();
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
        // Fixed-height rows must stay on one line; wrapped credits would spill
        // into the next row.
        if ["footer", "queue-artist", "card-artist"]
            .iter()
            .any(|prefix| key.starts_with(prefix))
        {
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
    fn artist_portrait(
        &mut self,
        name: &str,
        id: &str,
        fallback: &str,
        side: f32,
        cx: &App,
    ) -> AnyElement {
        let key = cache_key(name, id);
        if self.profile_requested.insert(key.clone()) {
            self.service
                .send(Command::ArtistProfile(name.into(), id.into(), false));
        }
        let portrait = self
            .artist_profiles
            .get(&key)
            .map(|profile| profile.portrait.clone())
            .filter(|portrait| !portrait.is_empty() && !self.art_failures.contains(portrait))
            .unwrap_or_else(|| fallback.into());
        self.art(&portrait, side, cx)
    }
    /// Queues covers just outside the viewport once scrolling pauses, so they are
    /// already downloaded when they scroll in. Covers on screen still go first.
    fn prefetch_art(&mut self, list: &'static str, top: usize, covers: Vec<String>) {
        let now = Instant::now();
        match self.prefetch_anchor {
            Some((anchor, row, since)) if anchor == list && row == top => {
                if now.duration_since(since) < ART_SETTLE {
                    self.art_wake = true;
                    return;
                }
            }
            _ => {
                self.prefetch_anchor = Some((list, top, now));
                self.art_wake = true;
                return;
            }
        }
        for id in covers {
            if id.is_empty() || self.artwork.contains_key(&id) || self.art_failures.contains(&id) {
                continue;
            }
            if self
                .art_prefetched
                .get(&id)
                .is_none_or(|sent| now.duration_since(*sent) >= ART_REPRIORITIZE)
            {
                self.art_prefetched.insert(id.clone(), now);
                self.service
                    .send(Command::Artwork(id, Some(Priority::Nearby)));
            }
        }
    }
    fn art(&mut self, id: &str, side: f32, cx: &App) -> AnyElement {
        if let Some(path) = self.artwork.get(id) {
            let path = match thumbnail::size_for(side).filter(|_| self.art_thumbs.contains(id)) {
                Some(size) => thumbnail::thumb_path(path, size),
                None => path.clone(),
            };
            return div()
                .size(px(side))
                .flex_shrink_0()
                .overflow_hidden()
                .child(
                    img(path)
                        .image_cache(&self.image_cache)
                        .size_full()
                        .aspect_square()
                        .object_fit(ObjectFit::Cover),
                )
                .into_any_element();
        }
        if !id.is_empty() && !self.art_failures.contains(id) {
            let now = Instant::now();
            // Covers already on disk show at once; only downloads wait for the tile to settle.
            let (first, last) = self.art_seen.entry(id.to_string()).or_insert((now, now));
            let fresh = *first == now;
            if now.duration_since(*last) > ART_SETTLE {
                *first = now;
            }
            *last = now;
            let settled = now.duration_since(*first) >= ART_SETTLE;
            if fresh {
                self.service.send(Command::Artwork(id.into(), None));
            }
            if !settled {
                self.art_wake = true;
            } else if self
                .art_requested
                .get(id)
                .is_none_or(|sent| now.duration_since(*sent) >= ART_REPRIORITIZE)
            {
                self.art_requested.insert(id.into(), now);
                self.service
                    .send(Command::Artwork(id.into(), Some(Priority::Visible)));
            }
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
        let controls_inset = sonora::platform::TITLEBAR_CONTROLS_INSET;
        let tabs_width = (width - search_width - controls_inset - 32. - 24.).clamp(100., 440.);
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
        let navigator = navigator_key(&self.preferences.layout);
        let weak = cx.weak_entity();
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
            .when(controls_inset > 0., |bar| {
                bar.child(
                    // The transparent macOS title bar places traffic lights in this area.
                    div().w(px(controls_inset)).h_full().flex_shrink_0(),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(2.))
                    .pr_2()
                    .child(
                        short_button("history-back", Glyph::ChevronLeft, "Back")
                            .disabled(self.history_back.is_empty())
                            .on_click(cx.listener(|app, _, window, cx| app.go_back(window, cx))),
                    )
                    .child(
                        short_button("history-forward", Glyph::ChevronRight, "Forward")
                            .disabled(self.history_forward.is_empty())
                            .on_click(cx.listener(|app, _, window, cx| app.go_forward(window, cx))),
                    ),
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
                    .dropdown_menu(move |mut menu, _, _| {
                        menu = menu.label("Navigator and artist browser");
                        for (key, label) in [
                            ("show", "Show"),
                            ("auto", "Auto-hide"),
                            ("disabled", "Disabled"),
                        ] {
                            let weak = weak.clone();
                            menu = menu.item(
                                PopupMenuItem::new(label)
                                    .checked(navigator == key)
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |app, cx| {
                                            set_navigator(&mut app.preferences.layout, key);
                                            app.navigator_peek = None;
                                            app.save_preferences();
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                        menu.separator()
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
                        app.playlist_editor_tracks = None;
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
                        let art =
                            if matches!(mode, NavigatorMode::Artist | NavigatorMode::AlbumArtist) {
                                app.artist_portrait(
                                    &item.label,
                                    &item.detail,
                                    &item.artwork,
                                    34.,
                                    cx,
                                )
                            } else {
                                app.art(&item.artwork, 34., cx)
                            };
                        let id = item.id.clone();
                        let artist_id = item.detail.clone();
                        div()
                            .id(("navigator-facet", i))
                            .role(Role::Button)
                            .aria_label(format!("Browse {}: {}", mode.label(), item.label))
                            // List rows shrink to their text unless told to fill.
                            .w_full()
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
    /// What the Favorites page lists: hearts, a minimum rating, or both. Kept in
    /// preferences so it survives restarts and new builds.
    fn favorites_filter_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let filter = self.preferences.favorites.clone();
        let weak = cx.weak_entity();
        let update =
            move |weak: &WeakEntity<Self>, cx: &mut App, change: &dyn Fn(&mut FavoritesFilter)| {
                let _ = weak.update(cx, |app, cx| {
                    change(&mut app.preferences.favorites);
                    app.save_preferences();
                    app.request_query(cx);
                });
            };
        let update = std::rc::Rc::new(update);
        Button::new("favorites-filter")
            .ghost()
            .small()
            .icon(Glyph::SlidersHorizontal)
            .label(filter.summary())
            .text_color(ink(cx).muted)
            .tooltip("Choose what counts as a favorite")
            .dropdown_menu(move |menu, _, _| {
                let item =
                    |label: &str, checked: bool, change: Box<dyn Fn(&mut FavoritesFilter)>| {
                        let (weak, update) = (weak.clone(), update.clone());
                        PopupMenuItem::new(label.to_string())
                            .checked(checked)
                            .on_click(move |_, _, cx| update(&weak, cx, &*change))
                    };
                let mut menu = menu
                    .label("Include")
                    .item(item(
                        "Hearted",
                        filter.hearted || filter.min_rating == 0,
                        Box::new(|filter| filter.hearted = !filter.hearted),
                    ))
                    .separator()
                    .label("Minimum rating")
                    .item(item(
                        "Any rating",
                        filter.min_rating == 0,
                        Box::new(|filter| {
                            filter.min_rating = 0;
                            filter.hearted = true;
                        }),
                    ));
                for stars in 1..=5u8 {
                    let label = format!(
                        "{}{}",
                        "★".repeat(stars as usize),
                        if stars < 5 { " and up" } else { "" }
                    );
                    menu = menu.item(item(
                        &label,
                        filter.min_rating == stars,
                        Box::new(move |filter| filter.min_rating = stars),
                    ));
                }
                menu.separator()
                    .label("When both are on")
                    .item(item(
                        "Match either",
                        !filter.match_all,
                        Box::new(|filter| filter.match_all = false),
                    ))
                    .item(item(
                        "Match both",
                        filter.match_all,
                        Box::new(|filter| filter.match_all = true),
                    ))
            })
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
        if self.is_library() && self.query.favorites {
            left = left.child(self.favorites_filter_menu(cx));
        }
        if self.workspace == Workspace::Queue
            && self.artist_name.is_none()
            && self.library_width >= 620.
        {
            left = left.child(
                div()
                    .text_xs()
                    .text_color(colors.muted)
                    .child(self.queue_summary()),
            );
        }
        let mut alphabet = div().flex().items_center().gap(px(1.));
        if self.is_library()
            && self.main_view() != MainView::Tracks
            && self.library_width >= 900.
            && self
                .query
                .ordering
                .as_ref()
                .is_some_and(|spec| spec.alphabetical_field().is_some())
        {
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
                                app.query
                                    .ordering
                                    .as_ref()
                                    .is_some_and(|spec| spec.album_letter(album) == letter)
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
                                    app.query.record_label.clear();
                                    app.request_query(cx);
                                })),
                        )
                        .child(self.sort_button(cx))
                    })
                    .when(
                        self.workspace == Workspace::Queue && self.artist_name.is_none(),
                        |s| s.child(self.queue_toolbar(cx)),
                    )
                    .children(self.downloads_button(cx))
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
                                    let id = item.id;
                                    div()
                                        .id(("browser-value", i))
                                        .role(Role::Button)
                                        .aria_label(format!("Filter {label}: {}", item.label))
                                        // List rows shrink to their text unless told to fill.
                                        .w_full()
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
                                            move |app, event: &ClickEvent, _, cx| {
                                                app.select_browser(
                                                    column,
                                                    Some(id.clone()),
                                                    event.modifiers().platform
                                                        || event.modifiers().control,
                                                    cx,
                                                );
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
        div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .gap(px(2.))
            .child(self.favorite_button(&track, &key, cx))
            .child(self.rating_stars(&track, &key, cx))
            .into_any_element()
    }
    fn rating_stars(&self, track: &Track, key: &str, cx: &mut Context<Self>) -> Div {
        let colors = ink(cx);
        let rating = track.user_rating.unwrap_or(0);
        let preview = self
            .rating_hover
            .as_ref()
            .filter(|(hovered, _)| hovered == key)
            .map(|(_, value)| rating_after_click(rating, *value));
        let mut stars = div().flex().items_center().flex_shrink_0();
        for value in 1..=5u8 {
            let id = track.id.clone();
            let filled = value <= preview.unwrap_or(rating);
            let color = if filled { colors.accent } else { colors.muted };
            let hover_key = key.to_string();
            stars = stars.child(
                div()
                    .id(SharedString::from(format!("rating-hover-{key}-{value}")))
                    .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                        let entry = (hover_key.clone(), value);
                        if *hovered {
                            app.rating_hover = Some(entry);
                        } else if app.rating_hover.as_ref() == Some(&entry) {
                            app.rating_hover = None;
                        } else {
                            return;
                        }
                        cx.notify();
                    }))
                    .child(
                        Button::new(SharedString::from(format!("rating-{key}-{value}")))
                            .ghost()
                            .compact()
                            .w(px(15.))
                            .h(px(23.))
                            .label(if filled { "★" } else { "☆" })
                            .text_color(color)
                            .tooltip(if value == rating {
                                "Clear rating".to_string()
                            } else {
                                format!("Rate {value} / 5")
                            })
                            .accessibility_label(format!("Rate {} {value} stars", track.title))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.rating_hover = None;
                                app.mutate(
                                    Mutation::Rating(id.clone(), rating_after_click(rating, value)),
                                    cx,
                                );
                            })),
                    ),
            );
        }
        stars
    }
    /// One button that opens a rating menu, for widths where five stars don't fit.
    fn rating_menu(&self, track: &Track, key: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = ink(cx);
        let rating = track.user_rating.unwrap_or(0);
        let id = track.id.clone();
        let weak = cx.weak_entity();
        Button::new(SharedString::from(format!("rating-menu-{key}")))
            .ghost()
            .compact()
            .h(px(24.))
            .label(if rating == 0 {
                "☆".to_string()
            } else {
                format!("★ {rating}")
            })
            .text_color(if rating == 0 {
                colors.muted
            } else {
                colors.accent
            })
            .tooltip("Rating")
            .accessibility_label(format!("Rate {}", track.title))
            .dropdown_menu(move |mut menu, _, _| {
                for value in (0..=5u8).rev() {
                    let id = id.clone();
                    let weak = weak.clone();
                    let label = if value == 0 {
                        "No rating".to_string()
                    } else {
                        "★".repeat(value.into())
                    };
                    menu = menu.item(PopupMenuItem::new(label).checked(value == rating).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |app, cx| {
                                app.mutate(Mutation::Rating(id.clone(), value), cx)
                            });
                        },
                    ));
                }
                menu
            })
    }
    fn favorite_button(&self, track: &Track, key: &str, cx: &mut Context<Self>) -> Button {
        let colors = ink(cx);
        let starred = track.starred.is_some();
        let id = track.id.clone();
        Button::new(SharedString::from(format!("love-{key}")))
            .ghost()
            .compact()
            .w(px(23.))
            .h(px(24.))
            .label(if starred { "♥" } else { "♡" })
            .text_color(if starred {
                colors.favorite
            } else {
                colors.muted
            })
            .tooltip(if starred {
                "Remove favorite"
            } else {
                "Favorite"
            })
            .accessibility_label(format!("Favorite {}", track.title))
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                app.mutate(Mutation::Favorite(id.clone(), !starred), cx);
            }))
    }
    fn queue_places(&self) -> Rc<QueuePlaces> {
        let current = self.playback.track.as_ref();
        Rc::new(QueuePlaces {
            current_track: current.map(|track| track.title.clone()),
            current_album: current
                .filter(|track| !track.album_id.is_empty())
                .map(|track| track.album.clone()),
            last_album: self.playback.queue.last().map(|track| {
                if track.album.is_empty() {
                    track.title.clone()
                } else {
                    track.album.clone()
                }
            }),
        })
    }
    /// Right-click menu for an album: play it, or queue it in one of three places.
    fn album_menu(
        &self,
        album_id: String,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let places = self.queue_places();
        let service = self.service.clone();
        move |menu, _, _| {
            let (play, queue) = (service.clone(), service.clone());
            let (play_id, queue_id) = (album_id.clone(), album_id.clone());
            let menu = menu
                .item(
                    PopupMenuItem::new("Play Album")
                        .on_click(move |_, _, _| play.send(Command::PlayAlbum(play_id.clone()))),
                )
                .separator();
            queue_options(menu, &places, move |placement| {
                queue.send(Command::EnqueueAlbum(queue_id.clone(), placement))
            })
        }
    }
    fn track_row(
        &mut self,
        track: Arc<Track>,
        number: usize,
        key: String,
        queue_index: Option<usize>,
        popular: Option<bool>,
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
        let queued = track.id.clone();
        let service = self.service.clone();
        let places = self.queue_places();
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
            .when(!selected, |s| s.hover(|s| s.bg(colors.panel)))
            .when_some(popular, |row, popular| {
                row.child(
                    div()
                        .id(SharedString::from(format!("popular-{key}")))
                        .w(px(12.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .when(popular, |slot| {
                            slot.child(
                                Icon::new(Glyph::Flame)
                                    .size(px(11.))
                                    .text_color(colors.flame),
                            )
                            .tooltip(|window, cx| {
                                tooltip::Tooltip::new("Popular on Last.fm").build(window, cx)
                            })
                        }),
                )
            })
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
                let queued = queued.clone();
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
                    .separator();
                let menu = queue_options(menu, &places, move |placement| {
                    service.send(Command::Enqueue(vec![queued.clone()], placement))
                });
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
        let new_artist = !selected_panel
            && self.query.ordering.as_ref().is_some_and(|spec| {
                spec.grouping != Grouping::None
                    && (index == 0
                        || spec.group_label(&self.view.albums[index - 1])
                            != spec.group_label(&album))
            });
        self.album_panel(album, new_artist, selected_panel, cx)
    }
    fn album_metadata(
        &self,
        album: &Album,
        show_artist: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let row = |label: &'static str, value: AnyElement| {
            div()
                .flex()
                .items_start()
                .gap_2()
                .min_w_0()
                .child(
                    div()
                        .w(px(38.))
                        .flex_shrink_0()
                        .text_color(colors.muted)
                        .child(label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(colors.text)
                        .child(value),
                )
        };
        let mut metadata = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .pb_2()
            .min_w_0()
            .text_size(px(10.));
        let credits = album.artist_credits();
        if show_artist && !credits.is_empty() {
            metadata = metadata.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.text)
                    .child(self.artist_links(credits, format!("album-artist-{}", album.id), cx)),
            );
        }
        if album.year.is_some() {
            metadata = metadata.child(row(
                "Year",
                self.year_link(album.year, format!("album-year-{}", album.id), cx),
            ));
        }
        if !album.genre.trim().is_empty() {
            metadata = metadata.child(row(
                "Genre",
                self.genre_links(&album.genre, format!("album-genres-{}", album.id), cx),
            ));
        }
        let labels = album.record_label_names();
        if !labels.is_empty() {
            let mut links = div().flex().flex_wrap().items_center().min_w_0().gap_1();
            for (index, label) in labels.into_iter().enumerate() {
                if index > 0 {
                    links = links.child(div().text_color(colors.muted).child("·"));
                }
                links = links.child(self.metadata_link(
                    ExplorerFilter::RecordLabel(label),
                    format!("album-label-{}-{index}", album.id),
                    cx,
                ));
            }
            metadata = metadata.child(row("Label", links.into_any_element()));
        }
        metadata.into_any_element()
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
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(album.name.clone()),
                    )
                    .child(self.album_favorite(&album, cx)),
            )
            .child(
                self.album_metadata(
                    &album,
                    selected_panel
                        || self
                            .query
                            .ordering
                            .as_ref()
                            .is_none_or(|spec| spec.grouping != Grouping::AlbumArtist),
                    cx,
                ),
            );
        if let Some(job) = self.album_download(&album) {
            let arrived = match job.expected {
                Some(expected) if album.song_count < expected => format!(
                    " · {} of {expected} tracks are in the library so far",
                    album.song_count
                ),
                _ => String::new(),
            };
            right = right.child(div().text_size(px(10.)).text_color(colors.accent).child(
                match job.state {
                    JobState::Failed => format!(
                        "{}{arrived} · open the artist page to pick another folder",
                        download_caption(job)
                    ),
                    _ => format!("{} from Soulseek{arrived}", download_caption(job)),
                },
            ));
        }
        if let Some(tracks) = tracks {
            let popular = self.popular_tracks(&album.id, &album.artist_credits());
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
                let flame = Some(popular.contains(&track.id));
                right = right.child(self.track_row(track, number, key, None, flame, cx));
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
                        .child(
                            if self
                                .query
                                .ordering
                                .as_ref()
                                .is_some_and(|spec| spec.grouping == Grouping::AlbumArtist)
                            {
                                self.artist_links(
                                    album.artist_credits(),
                                    format!("group-artist-{}", album.id),
                                    cx,
                                )
                            } else {
                                div()
                                    .child(
                                        self.query
                                            .ordering
                                            .as_ref()
                                            .map(|spec| spec.group_label(&album))
                                            .unwrap_or_default(),
                                    )
                                    .into_any_element()
                            },
                        ),
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
                                    }))
                                    .context_menu(self.album_menu(album.id.clone())),
                            )
                            .child(self.album_rating(&album, cx))
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
        let top = self.album_list.logical_scroll_top().item_ix;
        let nearby = PREFETCH_ALBUMS
            .filter_map(|delta| top.checked_add_signed(delta))
            .filter_map(|index| self.view.albums.get(index))
            .map(|album| album.cover_art.clone())
            .collect();
        self.prefetch_art("albums", top, nearby);
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
    fn artist_song_section(
        &mut self,
        title: &'static str,
        tracks: Vec<Arc<Track>>,
        personal: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let expanded = if personal {
            self.most_played_expanded
        } else {
            self.popular_expanded
        };
        let mut section = div().w_full().min_w_0().flex().flex_col().gap_1().child(
            div()
                .mb_2()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        );
        if tracks.is_empty() {
            section = section.child(
                div()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("No plays recorded yet."),
            );
        }
        for (i, track) in tracks
            .iter()
            .take(if expanded { 10 } else { 5 })
            .enumerate()
        {
            let row = self.track_row(
                track.clone(),
                i + 1,
                format!("artist-{}-{}", personal, track.id),
                None,
                None,
                cx,
            );
            let art = self.art(&track.cover_art, 36., cx);
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(36.))
                            .flex_shrink_0()
                            .rounded_md()
                            .overflow_hidden()
                            .child(art),
                    )
                    .child(div().flex_1().min_w_0().child(row))
                    .when(personal, |row| {
                        row.child(
                            div()
                                .w(px(48.))
                                .flex_shrink_0()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!("{} plays", track.play_count.unwrap_or(0))),
                        )
                    }),
            );
        }
        if tracks.len() > 5 {
            section = section.child(
                Button::new(if personal {
                    "more-personal"
                } else {
                    "more-popular"
                })
                .ghost()
                .small()
                .label(if expanded { "Show less" } else { "Show more" })
                .on_click(cx.listener(move |app, _, _, cx| {
                    if personal {
                        app.most_played_expanded = !app.most_played_expanded;
                    } else {
                        app.popular_expanded = !app.popular_expanded;
                    }
                    cx.notify();
                })),
            );
        }
        section.into_any_element()
    }
    fn similar_artists(
        &mut self,
        profile: Option<Arc<ArtistProfile>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(profile) = profile.filter(|profile| !profile.similar.is_empty()) else {
            return div().into_any_element();
        };
        let colors = ink(cx);
        let mut cards = div().flex().flex_wrap().gap_3();
        for similar in profile.similar.iter().take(12) {
            let item = similar.clone();
            let fallback = self
                .view
                .navigator
                .artists
                .iter()
                .chain(&self.view.navigator.album_artists)
                .find(|artist| normalize(&artist.label) == normalize(&item.name))
                .map(|artist| artist.artwork.clone())
                .unwrap_or_default();
            let art = if item.in_library {
                self.artist_portrait(&item.name, &item.id, &fallback, 88., cx)
            } else {
                self.art(&item.portrait, 88., cx)
            };
            cards = cards.child(
                div()
                    .id(SharedString::from(format!("similar-{}", item.name)))
                    .role(Role::Button)
                    .aria_label(format!(
                        "{} {}",
                        if item.in_library {
                            "Open artist"
                        } else {
                            "View on Last.fm:"
                        },
                        item.name
                    ))
                    .w(px(112.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(if item.in_library {
                        colors.text
                    } else {
                        colors.muted
                    })
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .size(px(88.))
                            .flex_shrink_0()
                            .rounded_full()
                            .overflow_hidden()
                            .when(!item.in_library, |image| image.opacity(0.4))
                            .child(art),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_size(px(11.))
                            .truncate()
                            .child(item.name.clone()),
                    )
                    .when(!item.in_library, |card| {
                        card.child(div().text_size(px(9.)).child("Not in your library"))
                    })
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if item.in_library {
                            app.open_artist(item.name.clone(), item.id.clone(), window, cx);
                        } else {
                            cx.open_url(&lastfm_url(&item.name));
                        }
                    })),
            );
        }
        div()
            .mt_5()
            .child(
                div()
                    .mb_3()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("Similar artists to {}", profile.name)),
            )
            .child(cards)
            .into_any_element()
    }
    fn artist_search_section(
        &mut self,
        category: ReleaseCategory,
        hits: Vec<ReleaseHit>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let count = hits.len();
        let mut section = div().mt_5().min_w_0().child(
            div()
                .mb_3()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(format!("{} ({count})", category.label())),
        );
        for hit in hits {
            let album = hit.album;
            let selected = self
                .selected_album
                .as_ref()
                .is_some_and(|selected| selected.id == album.id);
            let open = album.clone();
            let year = if let Some(year) = album.year {
                self.metadata_link(
                    ExplorerFilter::Year(year),
                    format!("search-release-year-{}", album.id),
                    cx,
                )
            } else {
                div().child("Year unknown").into_any_element()
            };
            let mut block = div()
                .id(SharedString::from(format!("artist-search-{}", album.id)))
                .mb_4()
                .min_w_0()
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "artist-search-release-{}",
                            album.id
                        )))
                        .role(Role::Button)
                        .aria_label(format!("Select release {}", album.name))
                        .flex()
                        .items_center()
                        .gap_3()
                        .cursor_pointer()
                        .pb_2()
                        .border_b_2()
                        .border_color(if selected {
                            colors.accent
                        } else {
                            colors.background
                        })
                        .child(self.art(&album.cover_art, 56., cx))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .truncate()
                                        .child(album.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(colors.muted)
                                        .child(year),
                                ),
                        )
                        .on_click(cx.listener(move |app, event: &ClickEvent, _, cx| {
                            app.select_album(open.clone(), cx);
                            if event.click_count() == 2 {
                                app.service.send(Command::PlayAlbum(open.id.clone()));
                            }
                        }))
                        .context_menu(self.album_menu(album.id.clone())),
                );
            for (index, track) in hit.songs.iter().cloned().enumerate() {
                let number = track.track.map_or(index + 1, |number| number as usize);
                block = block.child(self.track_row(
                    track,
                    number,
                    format!("artist-search-{}-{index}", album.id),
                    None,
                    None,
                    cx,
                ));
            }
            if selected {
                block = block.child(
                    div()
                        .mt_2()
                        .border_y_1()
                        .border_color(colors.line)
                        .child(self.album_panel(album, false, true, cx)),
                );
            }
            section = section.child(block);
        }
        section.into_any_element()
    }
    fn artist_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let name = self.artist_name.clone().unwrap_or_default();
        let profile = self
            .artist_profiles
            .get(&cache_key(&name, &self.artist_id))
            .cloned();
        let data = self.artist_data.clone().unwrap_or_default();
        let mut page = div()
            .id("artist-page")
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .p_4();
        let Some(albums) = self.artist_albums.clone() else {
            return page
                .child(div().child("Loading artist releases…"))
                .into_any_element();
        };
        let mut hero = div().mb_4().flex().items_center().gap_4();
        let fallback = albums
            .first()
            .map(|album| album.cover_art.as_str())
            .unwrap_or_default();
        hero = hero.child(self.artist_portrait(&name, &self.artist_id.clone(), fallback, 80., cx));
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
                            .child(name.clone()),
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
        if let Some(profile) = &profile {
            if !profile.biography.is_empty() {
                let overflows = self.biography_expanded
                    || biography_lines(&profile.biography, self.library_width - 32., cx) > 3;
                page = page
                    .child(
                        // A distinct id per state: text keeps its measured layout across
                        // frames, so removing the clamp alone leaves three lines.
                        div()
                            .id(if self.biography_expanded {
                                "artist-bio-full"
                            } else {
                                "artist-bio-clamped"
                            })
                            .mb_1()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .when(!self.biography_expanded, |text| {
                                text.line_clamp(3).text_ellipsis()
                            })
                            .child(profile.biography.clone()),
                    )
                    .when(overflows, |page| {
                        page.child(
                            Button::new("artist-bio-more")
                                .ghost()
                                .small()
                                .label(if self.biography_expanded {
                                    "Show less"
                                } else {
                                    "Read more"
                                })
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.biography_expanded = !app.biography_expanded;
                                    cx.notify();
                                })),
                        )
                    });
            }
            if let Some(url) = profile_url(&profile.url) {
                page = page.child(
                    Button::new("artist-source")
                        .ghost()
                        .small()
                        .label("Artist profile ↗")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
        }
        if !data.genres.is_empty() {
            let mut chips = div()
                .mt_2()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child("From your library:");
            for (index, genre) in data.genres.iter().enumerate() {
                chips = chips.child(self.metadata_link(
                    ExplorerFilter::Genre(genre.clone()),
                    format!("artist-genre-{index}"),
                    cx,
                ));
            }
            page = page.child(chips);
        }
        if !self.search_text.trim().is_empty() {
            let sections = search_releases(&albums, &data.tracks, &self.search_text);
            if sections.is_empty() {
                return page
                    .child(
                        div()
                            .mt_5()
                            .text_color(colors.muted)
                            .child("No releases match your search."),
                    )
                    .into_any_element();
            }
            for (category, hits) in sections {
                page = page.child(self.artist_search_section(category, hits, cx));
            }
            return page.into_any_element();
        }
        let popular = profile
            .as_ref()
            .map(|profile| profile.popular.clone())
            .unwrap_or_default();
        let column = |section: AnyElement| {
            div()
                .flex_1()
                .min_w(px(ARTIST_SONG_COLUMN_MIN))
                .child(section)
        };
        let mut songs = div().mt_4().flex().flex_wrap().gap_4().min_w_0();
        if !popular.is_empty() {
            songs = songs.child(column(self.artist_song_section(
                "Popular songs",
                popular,
                false,
                cx,
            )));
        }
        songs = songs.child(column(self.artist_song_section(
            "Your most played",
            data.most_played,
            true,
            cx,
        )));
        page = page.child(songs);
        if albums.is_empty() {
            let similar = self.similar_artists(profile.clone(), cx);
            let missing = self.missing_releases(&name, cx);
            return page
                .children(missing)
                .child(similar)
                .child(
                    div()
                        .text_color(colors.muted)
                        .child("No releases for this artist in your library."),
                )
                .into_any_element();
        }
        let (columns, side) = cover_layout((self.library_width - 8.).max(0.));
        let columns = columns.max(1);
        for category in [
            ReleaseCategory::Album,
            ReleaseCategory::Ep,
            ReleaseCategory::Single,
            ReleaseCategory::Other,
        ] {
            let releases = albums
                .iter()
                .filter(|album| album.category() == category)
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
            let mut cards = Vec::with_capacity(releases.len());
            for album in &releases {
                let open = album.clone();
                let selected = self
                    .selected_album
                    .as_ref()
                    .is_some_and(|selected| selected.id == album.id);
                cards.push(
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
                        }))
                        .context_menu(self.album_menu(album.id.clone()))
                        .into_any_element(),
                );
            }
            // The album panel opens right under the row holding the picked cover.
            let selected = self.selected_album.clone().and_then(|selected| {
                releases
                    .iter()
                    .position(|album| album.id == selected.id)
                    .map(|index| (index / columns, selected))
            });
            let mut panel = selected.as_ref().map(|(_, album)| {
                div()
                    .border_y_1()
                    .border_color(colors.line)
                    .child(self.album_panel(album.clone(), false, true, cx))
                    .into_any_element()
            });
            let panel_row = selected.map(|(row, _)| row);
            let mut grid = div().flex().flex_col().gap_4().min_w_0();
            let mut cards = cards.into_iter();
            for row in 0..releases.len().div_ceil(columns) {
                grid = grid.child(
                    div()
                        .flex()
                        .gap_4()
                        .min_w_0()
                        .children(cards.by_ref().take(columns)),
                );
                if panel_row == Some(row) {
                    grid = grid.children(panel.take());
                }
            }
            section = section.child(grid);
            page = page.child(section);
        }
        let missing = self.missing_releases(&name, cx);
        page.children(missing)
            .child(self.similar_artists(profile, cx))
            .into_any_element()
    }
    /// Releases the artist has published that the library lacks, dimmed like
    /// similar artists that are not in the library.
    fn missing_releases(&mut self, name: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let colors = ink(cx);
        let view = self
            .discographies
            .get(&cache_key(name, &self.artist_id))
            .cloned()?;
        let all = self.preferences.all_missing_releases;
        let downloads = self.downloads.clone();
        let job = |release: &MissingRelease| downloads.iter().any(|job| job.release == release.id);
        // A release being downloaded is listed first, whatever its type.
        let mut shown = view
            .missing
            .iter()
            .filter(|release| all || release.core || job(release))
            .cloned()
            .collect::<Vec<_>>();
        shown.sort_by_key(|release| !job(release));
        let others = view.missing.len() - shown.len();
        if shown.is_empty() && others == 0 {
            return view.loading.then(|| {
                div()
                    .mt_5()
                    .text_size(px(11.))
                    .text_color(colors.muted)
                    .child("Checking discography…")
                    .into_any_element()
            });
        }
        let (columns, side) = cover_layout((self.library_width - 8.).max(0.));
        let limit = if self.missing_expanded {
            shown.len()
        } else {
            columns * 2
        };
        let mut section = div().mt_5().min_w_0().child(
            div()
                .mb_3()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("Missing from your library ({})", shown.len())),
                )
                .when(all || others > 0, |header| {
                    header.child(
                        Button::new("missing-release-types")
                            .ghost()
                            .small()
                            .label(if all {
                                "Albums and EPs only".to_string()
                            } else {
                                format!("Show singles, live, compilations… ({others})")
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.preferences.all_missing_releases =
                                    !app.preferences.all_missing_releases;
                                app.save_preferences();
                                cx.notify();
                            })),
                    )
                }),
        );
        let mut tiles: Vec<(String, AnyElement)> = Vec::new();
        for release in shown.iter().take(limit) {
            let selected = self.selected_missing.as_ref() == Some(&release.id);
            let job = self
                .downloads
                .iter()
                .find(|job| job.release == release.id)
                .cloned();
            let caption = match &job {
                Some(job) => download_caption(job),
                None => "Not in your library".into(),
            };
            let year = release
                .year
                .map_or("Year unknown".to_string(), |year| year.to_string());
            let open = release.clone();
            let artist = name.to_string();
            let art = self.art(&release.cover, side, cx);
            tiles.push((
                release.id.clone(),
                div()
                    .id(SharedString::from(format!(
                        "missing-release-{}",
                        release.id
                    )))
                    .role(Role::Button)
                    .aria_label(format!("Missing release {}", release.title))
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
                    .text_color(colors.muted)
                    .child(div().opacity(0.4).child(art))
                    .child(
                        div()
                            .mt_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(release.title.clone()),
                    )
                    .child(div().text_size(px(10.)).truncate().child(format!(
                        "{} · {}",
                        if release.upcoming {
                            "Upcoming".to_string()
                        } else {
                            year
                        },
                        release.kind
                    )))
                    .child(div().text_size(px(9.)).truncate().child(caption))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        if app.selected_missing.as_ref() == Some(&open.id) {
                            app.selected_missing = None;
                        } else {
                            // A lookup that failed earlier is tried again on reopening.
                            if matches!(app.release_details.get(&open.id), Some(Err(_))) {
                                app.release_details.remove(&open.id);
                                app.release_details_requested.remove(&open.id);
                            }
                            app.selected_missing = Some(open.id.clone());
                            app.selected_album = None;
                            let query = format!("{artist} {}", open.title);
                            app.soulseek_query
                                .update(cx, |input, cx| input.set_value(query, window, cx));
                        }
                        app.soulseek = None;
                        app.soulseek_open = None;
                        cx.notify();
                    }))
                    .into_any_element(),
            ));
        }
        // The panel opens under the row holding the picked cover, or after the
        // last row when that cover is folded away.
        let selected = self
            .selected_missing
            .as_ref()
            .and_then(|id| shown.iter().find(|release| &release.id == id))
            .cloned();
        let rows = tiles.len().div_ceil(columns.max(1));
        let panel_row = selected.as_ref().map(|release| {
            tiles
                .iter()
                .position(|(id, _)| id == &release.id)
                .map_or(rows.saturating_sub(1), |index| index / columns.max(1))
        });
        let mut panel = selected.map(|release| {
            div()
                .py_3()
                .border_y_1()
                .border_color(colors.line)
                .child(self.missing_panel(name, release, cx))
                .into_any_element()
        });
        let mut grid = div().flex().flex_col().gap_4().min_w_0();
        let mut tiles = tiles.into_iter();
        for row in 0..rows {
            grid = grid.child(
                div()
                    .flex()
                    .gap_4()
                    .min_w_0()
                    .children(tiles.by_ref().take(columns.max(1)).map(|(_, tile)| tile)),
            );
            if panel_row == Some(row) {
                grid = grid.children(panel.take());
            }
        }
        section = section.child(grid);
        if shown.len() > columns * 2 {
            section = section.child(
                Button::new("missing-release-more")
                    .ghost()
                    .small()
                    .mt_2()
                    .label(if self.missing_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {}", shown.len())
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.missing_expanded = !app.missing_expanded;
                        cx.notify();
                    })),
            );
        }
        Some(section.into_any_element())
    }
    /// One missing release laid out like a library album: cover, details and
    /// track list, places to listen to it, then the Soulseek download.
    fn missing_panel(
        &mut self,
        artist: &str,
        release: MissingRelease,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request_release_details(&release.id);
        let colors = ink(cx);
        let details = self.release_details.get(&release.id).cloned();
        let loaded = details
            .as_ref()
            .and_then(|details| details.as_ref().ok())
            .cloned();
        let hide = (
            artist.to_string(),
            self.artist_id.clone(),
            release.id.clone(),
        );
        let page = format!("https://musicbrainz.org/release-group/{}", release.id);
        let note = |text: String| {
            div()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(text)
        };
        let mut facts = vec![
            release.kind.clone(),
            match (release.upcoming, release.year) {
                (true, Some(year)) => format!("Upcoming, {year}"),
                (_, Some(year)) => year.to_string(),
                _ => "Year unknown".into(),
            },
        ];
        if let Some(details) = &loaded {
            match (details.label.is_empty(), details.catalog_number.is_empty()) {
                (false, false) => {
                    facts.push(format!("{} ({})", details.label, details.catalog_number))
                }
                (false, true) => facts.push(details.label.clone()),
                _ => {}
            }
            if !details.country.is_empty() && details.country != "XW" {
                facts.push(details.country.clone());
            }
            if !details.tracks.is_empty() {
                let count = details.tracks.len();
                facts.push(format!(
                    "{count} {}",
                    if count == 1 { "track" } else { "tracks" }
                ));
            }
            if details.seconds() > 0 {
                facts.push(duration_label(details.seconds() as f64));
            }
        }
        let mut right = div()
            .flex_1()
            .min_w_0()
            .when(self.library_width < 520., |column| column.w_full())
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .min_h(px(24.))
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
                            .child(release.title.clone()),
                    )
                    .child(
                        Button::new("missing-open-musicbrainz")
                            .ghost()
                            .small()
                            .label("Open on MusicBrainz")
                            .on_click(move |_, _, cx| cx.open_url(&page)),
                    )
                    .child(
                        Button::new("missing-hide")
                            .ghost()
                            .small()
                            .label("Hide")
                            .tooltip("Stop listing this release as missing")
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.selected_missing = None;
                                app.service.send(Command::HideRelease(
                                    hide.0.clone(),
                                    hide.1.clone(),
                                    hide.2.clone(),
                                ));
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(facts.join(" · ")),
            )
            .when_some(
                loaded.as_ref().filter(|details| !details.genres.is_empty()),
                |column, details| {
                    column.child(
                        div()
                            .text_size(px(10.))
                            .text_color(colors.muted)
                            .truncate()
                            .child(details.genres.join(", ")),
                    )
                },
            );
        match &details {
            None => {
                right = right.child(
                    div()
                        .h(px(60.))
                        .pt_2()
                        .child(note("Loading album details…".into())),
                );
            }
            Some(Err(error)) => {
                right = right.child(
                    div()
                        .pt_2()
                        .child(note(format!("Could not load the track list: {error}"))),
                );
            }
            Some(Ok(details)) if details.tracks.is_empty() => {
                right = right.child(div().pt_2().child(note(
                    "MusicBrainz has no track list for this release.".into(),
                )));
            }
            Some(Ok(details)) => {
                let discs = details
                    .tracks
                    .iter()
                    .map(|track| track.disc)
                    .collect::<HashSet<_>>();
                let mut last_disc = None;
                for track in &details.tracks {
                    if discs.len() > 1 && last_disc != Some(track.disc) {
                        last_disc = Some(track.disc);
                        right = right.child(
                            div()
                                .h(px(23.))
                                .pt_1()
                                .text_size(px(10.))
                                .text_color(colors.muted)
                                .child(format!("Disc {}", track.disc)),
                        );
                    }
                    right = right.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .h(px(27.))
                            .px_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_size(px(11.))
                            .child(
                                div()
                                    .w(px(36.))
                                    .flex_shrink_0()
                                    .text_color(colors.muted)
                                    .truncate()
                                    .child(track.number.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .flex()
                                    .gap_1()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_shrink(1.)
                                            .truncate()
                                            .child(track.title.clone()),
                                    )
                                    .when(!track.artist.is_empty(), |title| {
                                        title.child(
                                            div()
                                                .min_w_0()
                                                .flex_1()
                                                .truncate()
                                                .text_color(colors.muted)
                                                .child(format!("· {}", track.artist)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(38.))
                                    .flex_shrink_0()
                                    .text_color(colors.muted)
                                    .text_right()
                                    .child(
                                        track
                                            .seconds
                                            .map(|seconds| duration_label(seconds as f64))
                                            .unwrap_or_default(),
                                    ),
                            ),
                    );
                }
            }
        }
        if !release.upcoming {
            let mut links = div().flex().flex_wrap().items_center().gap_1();
            for (index, link) in listen_links(loaded.as_deref(), artist, &release.title)
                .into_iter()
                .enumerate()
            {
                let url = link.url.clone();
                links = links.child(
                    Button::new(("missing-listen", index))
                        .ghost()
                        .small()
                        .label(if link.direct {
                            link.service.clone()
                        } else {
                            format!("Search {}", link.service)
                        })
                        .tooltip(if link.direct {
                            format!("Open this release on {}", link.service)
                        } else {
                            format!("Search {} for this release", link.service)
                        })
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            right = right.child(
                div()
                    .mt_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.muted)
                            .child("Listen on"),
                    )
                    .child(links),
            );
        }
        right = right.child(
            div()
                .mt_3()
                .pt_2()
                .border_t_1()
                .border_color(colors.line)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.muted)
                        .child("Download from Soulseek"),
                )
                .child(self.soulseek_block(artist, release.clone(), cx)),
        );
        div()
            .w_full()
            .min_w_0()
            .px_3()
            .flex()
            .gap_3()
            .items_start()
            .when(self.library_width < 520., |row| row.flex_col())
            .child(
                div()
                    .w(px(124.))
                    .flex_shrink_0()
                    .child(self.art(&release.cover, 124., cx)),
            )
            .child(right)
            .into_any_element()
    }
    /// The Soulseek half of a missing release: its download, or a search for
    /// the folders peers offer when slskd is linked.
    fn soulseek_block(
        &mut self,
        artist: &str,
        release: MissingRelease,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let job = self
            .downloads
            .iter()
            .find(|job| job.release == release.id)
            .cloned();
        let mut block = div().flex().flex_col().gap_2().min_w_0();
        let note = |text: String| {
            div()
                .text_size(px(11.))
                .text_color(colors.muted)
                .child(text)
        };
        if release.upcoming {
            return block
                .child(note("This release is not out yet.".into()))
                .into_any_element();
        }
        if !self.preferences.downloads.enabled {
            return block
                .child(note(
                    "Link slskd in Settings → Downloads to look for this release on Soulseek."
                        .into(),
                ))
                .into_any_element();
        }
        if let Some(job) = job {
            let dismiss = job.release.clone();
            return block
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(note(format!(
                            "{} · from {}",
                            download_caption(&job),
                            job.username
                        )))
                        .when(job.state != JobState::Active, |row| {
                            row.child(
                                Button::new("missing-download-dismiss")
                                    .ghost()
                                    .small()
                                    .label(if job.state == JobState::Failed {
                                        "Pick another folder"
                                    } else {
                                        "Clear"
                                    })
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.service.send(Command::DismissDownload(dismiss.clone()));
                                        cx.notify();
                                    })),
                            )
                        }),
                )
                .into_any_element();
        }
        let search = self
            .soulseek
            .as_ref()
            .filter(|(id, _)| id == &release.id)
            .map(|(_, update)| update.clone());
        let searching = matches!(search, Some(SearchUpdate::Searching(_)));
        let start = self
            .download_start
            .as_ref()
            .filter(|start| start.release == release.id);
        let starting = start.is_some_and(|start| start.failure.is_none());
        let id = release.id.clone();
        block = block.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .max_w(px(420.))
                        .child(Input::new(&self.soulseek_query).small()),
                )
                .child(
                    Button::new("missing-search")
                        .primary()
                        .small()
                        .label(if search.is_some() {
                            "Search again"
                        } else {
                            "Find on Soulseek"
                        })
                        .disabled(searching || starting)
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let query = app.soulseek_query.read(cx).value().to_string();
                            app.soulseek = Some((id.clone(), SearchUpdate::Searching(0)));
                            app.soulseek_open = None;
                            app.download_start = None;
                            app.service.send(Command::SoulseekSearch(id.clone(), query));
                            cx.notify();
                        })),
                ),
        );
        if let Some(start) = start {
            let failed = start.failure.is_some();
            let tint = colors.accent;
            let message = match &start.failure {
                Some(reason) => format!(
                    "Download not started. {reason} Pick another folder below, or try again."
                ),
                None => format!(
                    "Starting download from {}… asking for the full folder and adding it to the queue. This can take up to a minute.",
                    start.username
                ),
            };
            block = block.child(
                div()
                    .id("download-start")
                    .role(if failed { Role::Alert } else { Role::Status })
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_l_2()
                    .border_color(tint)
                    .bg(colors.panel)
                    .text_size(px(11.))
                    .child(
                        Icon::new(if failed {
                            Glyph::CircleAlert
                        } else {
                            Glyph::LoaderCircle
                        })
                        .size(px(13.))
                        .flex_shrink_0()
                        .text_color(tint),
                    )
                    .child(div().flex_1().min_w_0().child(message)),
            );
        }
        match search {
            None => {}
            Some(SearchUpdate::Searching(count)) => {
                block = block.child(note(format!(
                    "Searching Soulseek… {count} {} so far",
                    if count == 1 { "response" } else { "responses" }
                )));
            }
            Some(SearchUpdate::Failed(message)) => block = block.child(note(message)),
            Some(SearchUpdate::Results(candidates, expected)) => {
                if candidates.is_empty() {
                    block = block.child(note(
                        "No folders found. The Soulseek server hides some names; try different search text."
                            .into(),
                    ));
                } else {
                    block = block.child(note(format!(
                        "{} {} · best matches first{}",
                        candidates.len(),
                        if candidates.len() == 1 {
                            "folder"
                        } else {
                            "folders"
                        },
                        expected.map_or(String::new(), |tracks| format!(
                            " · the release has {tracks} tracks"
                        ))
                    )));
                    for (index, candidate) in candidates.iter().take(40).enumerate() {
                        block = block.child(
                            self.candidate_row(artist, &release, index, candidate, expected, cx),
                        );
                    }
                }
            }
        }
        block.into_any_element()
    }
    fn candidate_row(
        &self,
        artist: &str,
        release: &MissingRelease,
        index: usize,
        candidate: &Candidate,
        expected: Option<u32>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let open = self.soulseek_open == Some(index);
        let tracks = match (candidate.completeness, expected) {
            (Completeness::Unknown, _) | (_, None) => format!("{} tracks", candidate.tracks),
            (_, Some(expected)) => format!("{}/{expected} tracks", candidate.tracks),
        };
        let mut facts = vec![
            candidate.username.clone(),
            tracks,
            candidate.format.clone(),
            size_label(candidate.size),
        ];
        if candidate.seconds > 0 {
            facts.push(duration_label(candidate.seconds as f64));
        }
        facts.push(if candidate.free_slot {
            "free slot".into()
        } else {
            format!("queue {}", candidate.queue)
        });
        if candidate.speed > 0 {
            facts.push(format!("{}/s", size_label(candidate.speed)));
        }
        if candidate.locked > 0 {
            facts.push(format!("{} locked files left out", candidate.locked));
        }
        let start = self
            .download_start
            .as_ref()
            .filter(|start| start.release == release.id && start.failure.is_none());
        let starting = start.is_some();
        let clicked = start.is_some_and(|start| start.candidate == index);
        let request = DownloadRequest {
            artist: artist.to_string(),
            artist_id: self.artist_id.clone(),
            release: release.id.clone(),
            title: release.title.clone(),
            expected,
            candidate: candidate.clone(),
        };
        let mut row = div()
            .id(("soulseek-candidate", index))
            .px_2()
            .py_1()
            .min_w_0()
            .rounded_sm()
            .hover(|row| row.bg(colors.panel))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(("soulseek-candidate-files", index))
                            .role(Role::Button)
                            .aria_label(format!("Show files in {}", candidate.folder()))
                            .flex_1()
                            .min_w_0()
                            .cursor_pointer()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .truncate()
                                    .when(candidate.completeness == Completeness::Partial, |name| {
                                        name.text_color(colors.muted)
                                    })
                                    .child(candidate.folder().to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.muted)
                                    .truncate()
                                    .child(facts.join(" · ")),
                            )
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.soulseek_open =
                                    (app.soulseek_open != Some(index)).then_some(index);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("soulseek-download", index))
                            .small()
                            .label(if clicked { "Starting…" } else { "Download" })
                            .loading(clicked)
                            .disabled(starting && !clicked)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app
                                    .download_start
                                    .as_ref()
                                    .is_some_and(|start| start.failure.is_none())
                                {
                                    return;
                                }
                                // Keep the results until slskd accepts, so a refusal can pick another.
                                app.download_start = Some(DownloadStart {
                                    release: request.release.clone(),
                                    candidate: index,
                                    username: request.candidate.username.clone(),
                                    title: request.title.clone(),
                                    artist_key: cache_key(&request.artist, &request.artist_id),
                                    artist_name: request.artist.clone(),
                                    failure: None,
                                });
                                app.service
                                    .send(Command::Download(Box::new(request.clone())));
                                cx.notify();
                            })),
                    ),
            );
        if open {
            let mut files = div()
                .mt_1()
                .pl_3()
                .text_size(px(10.))
                .text_color(colors.muted);
            for file in &candidate.files {
                files = files.child(div().truncate().child(format!(
                    "{} · {}",
                    file.name(),
                    size_label(file.size)
                )));
            }
            row = row.child(files);
        }
        row.into_any_element()
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
        let rows = self.cover_rows().row_count();
        let top = self.cover_list.logical_scroll_top().item_ix.min(rows);
        // Card height is the square cover plus its two text lines and padding.
        let visible = (self.library_height / (side + 56.)).ceil() as usize + 1;
        let below = top + visible;
        let nearby = (top.saturating_sub(PREFETCH_ROWS)..top)
            .chain(below..(below + PREFETCH_ROWS).min(rows))
            .flat_map(|row| self.cover_rows().albums_in_row(row))
            .filter_map(|index| self.view.albums.get(index))
            .map(|album| album.cover_art.clone())
            .collect();
        self.prefetch_art("covers", top, nearby);
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
                            .context_menu(app.album_menu(id.clone())),
                    );
                }
                let first = app.cover_rows().albums_in_row(row).start;
                let mut item = div().w_full().min_w_0().flex().flex_col();
                if app.cover_sections.contains(&first)
                    && let Some(spec) = &app.query.ordering
                {
                    item = item.child(
                        div()
                            .mx_3()
                            .pt_3()
                            .pb_1()
                            .border_b_1()
                            .border_color(ink(cx).line)
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(spec.group_label(&app.view.albums[first])),
                    );
                }
                item = item.child(cards);
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
                        app.view.revision,
                        app.query.clone(),
                        app.tracks.len(),
                    ));
                }
                range
                    .map(|index| {
                        if let Some(track) = app.tracks.get(index).cloned() {
                            app.track_row(track, index + 1, format!("flat-{index}"), None, None, cx)
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
                                app.playlist_editor_tracks =
                                    Some(app.playback.queue.iter().map(|t| t.id.clone()).collect());
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
                                let key = format!("queue-side-{index}");
                                let favorite = app.favorite_button(&track, &key, cx);
                                let rating = if app.information_width < 300. {
                                    app.rating_menu(&track, &key, cx).into_any_element()
                                } else {
                                    app.rating_stars(&track, &key, cx).into_any_element()
                                };
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
                                    .overflow_hidden()
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
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .child(favorite)
                                            .child(rating),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
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
    /// Playback position extrapolated from the last snapshot, which arrives every ~200ms.
    fn live_position(&self) -> f64 {
        let mut position = self.playback.position;
        if self.playback.playing && !self.playback.buffering {
            position += self.playback_at.elapsed().as_secs_f64().min(0.5);
        }
        if self.playback.duration > 0. {
            position = position.min(self.playback.duration);
        }
        position
    }
    /// Redraws once the next timed line is due, so lyrics stay on the beat without a frame loop.
    fn schedule_lyric_wake(
        &mut self,
        lyrics: &Lyrics,
        following: bool,
        now: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = following
            .then(|| {
                (0..lyrics.line.len())
                    .filter_map(|index| Some((index, lyrics.line_position(index)?)))
                    .filter(|(_, start)| *start > now)
                    .min_by(|a, b| a.1.total_cmp(&b.1))
            })
            .flatten()
            .filter(|(_, start)| start - now < 0.4);
        let Some((index, start)) = next else {
            self.lyrics_wake = None;
            return;
        };
        if self
            .lyrics_wake
            .as_ref()
            .is_some_and(|(due, _)| *due == index)
        {
            return;
        }
        let delay = Duration::from_secs_f64(start - now + 0.004);
        let task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        });
        self.lyrics_wake = Some((index, task));
    }
    /// Redraws after `interval`, coalescing requests, for animations that don't need display rate.
    fn request_frame_after(
        &mut self,
        interval: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.frame_timer.is_some() {
            return;
        }
        self.frame_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(interval).await;
            this.update(cx, |app, cx| {
                app.frame_timer = None;
                cx.notify();
            })
            .ok();
        }));
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
        let dark = cx.theme().is_dark();
        let reduce_motion = cx.reduce_motion();
        let scroll = self.lyrics_scroll.clone();
        let viewport = scroll.bounds();
        let height = f32::from(viewport.size.height);
        let (text_size, leading, row_y, row_x, fade, weight) = if highlight {
            (28., 36., 9., 14., 120., FontWeight::BOLD)
        } else {
            (15., 22., 6., 8., 56., FontWeight::SEMIBOLD)
        };
        // The large view sits on a tinted backdrop where pure white reads best.
        let ink_color = if highlight && dark {
            gpui_kit::white()
        } else {
            colors.text
        };
        let mut content = div()
            .id("lyrics-scroll")
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&scroll);
        match self.lyrics.get(&track.id).cloned() {
            Some(Ok(lyrics)) if !lyrics.is_empty() => {
                let is_playing = self
                    .playback
                    .track
                    .as_ref()
                    .is_some_and(|playing| playing.id == track.id);
                let following = is_playing && lyrics.is_timed();
                let now = self.live_position();
                self.schedule_lyric_wake(&lyrics, following, now, window, cx);
                let active = is_playing.then(|| lyrics.active_line(now)).flatten();
                // Padding lets the first and last lines reach the anchor.
                content = if following && height > 0. {
                    content
                        .px_3()
                        .pt(px(height * LYRIC_ANCHOR))
                        .pb(px(height * (1. - LYRIC_ANCHOR)))
                } else {
                    content.p_3()
                };
                let target = |index: usize| -> Option<f32> {
                    if !following {
                        return Some(0.);
                    }
                    let item = scroll.bounds_for_item(index)?;
                    let anchor = viewport.top() + viewport.size.height * LYRIC_ANCHOR;
                    Some(
                        f32::from(anchor - item.center().y)
                            .clamp(-f32::from(scroll.max_offset().y), 0.),
                    )
                };
                let focus_index = active.unwrap_or(0);
                let follow = (track.id.clone(), active);
                if self.lyrics_follow.as_ref() != Some(&follow) {
                    let same_track = self
                        .lyrics_follow
                        .as_ref()
                        .is_some_and(|(id, _)| id == &track.id);
                    if same_track && following && !reduce_motion {
                        self.lyrics_scroll_motion =
                            Some((Instant::now(), f32::from(scroll.offset().y), focus_index));
                    } else {
                        // Layout for new lyrics lands a frame later, so settle over a few.
                        self.lyrics_scroll_motion = None;
                        self.lyrics_snap = 3;
                    }
                    self.lyrics_follow = Some(follow);
                }
                if let Some((started, from, index)) = self.lyrics_scroll_motion {
                    let progress = (started.elapsed().as_secs_f32() / LYRIC_SCROLL_SECONDS).min(1.);
                    let eased = 1. - (1. - progress).powi(4);
                    // Re-read the target each frame so late layout changes are absorbed.
                    if let Some(target) = target(index) {
                        scroll.set_offset(point(
                            scroll.offset().x,
                            px(from + (target - from) * eased),
                        ));
                    }
                    if progress < 1. {
                        window.request_animation_frame();
                    } else {
                        self.lyrics_scroll_motion = None;
                    }
                } else if self.lyrics_snap > 0 {
                    self.lyrics_snap -= 1;
                    match target(focus_index) {
                        Some(target) => scroll.set_offset(point(scroll.offset().x, px(target))),
                        None => scroll.scroll_to_top_of_item(focus_index),
                    }
                    window.request_animation_frame();
                }
                let offset_y = scroll.offset().y;
                for (index, line) in lyrics.line.iter().enumerate() {
                    let highlighted = lyrics.line_is_active(index, active);
                    // Sung lines recede most; upcoming lines dim with distance.
                    let rest = match active {
                        _ if !following => 0.86,
                        None => 0.55,
                        Some(current) if index < current => 0.3,
                        Some(current) => (0.6 - 0.06 * (index - current) as f32).max(0.34),
                    };
                    let alpha = transition(
                        SharedString::from(format!("lyric-alpha-{}-{highlight}-{index}", track.id)),
                        if highlighted { 1_f32 } else { rest },
                        Transition::new(Duration::from_millis(420)),
                        window,
                        cx,
                    )
                    .clamp(0., 1.);
                    // Lines dissolve toward the viewport edges instead of being clipped.
                    let edge = if following && height > 0. {
                        scroll.bounds_for_item(index).map_or(1., |item| {
                            let center = f32::from(item.center().y + offset_y - viewport.top());
                            let t = (center.min(height - center) / fade).clamp(0., 1.);
                            t * t * (3. - 2. * t)
                        })
                    } else {
                        1.
                    };
                    let shade = alpha * edge;
                    let empty = line.value.trim().is_empty();
                    let seek = is_playing.then(|| lyrics.line_position(index)).flatten();
                    let player = self.service.player.clone();
                    let mut row = div()
                        .id(SharedString::from(format!("lyric-{index}")))
                        .role(Role::Group)
                        .aria_label(format!(
                            "{}{}",
                            if highlighted { "Current lyric: " } else { "" },
                            if empty && highlighted && following {
                                "Instrumental"
                            } else {
                                line.value.as_str()
                            }
                        ))
                        .max_w(px(if highlight { 880. } else { 640. }))
                        .px(px(row_x))
                        .py(px(row_y))
                        .rounded_lg()
                        .text_size(px(text_size))
                        .line_height(px(leading))
                        .font_weight(weight)
                        .text_color(ink_color.opacity(shade));
                    row = if empty && highlighted && following {
                        row.child(lyric_gap(
                            index,
                            text_size,
                            leading,
                            ink_color.opacity(edge),
                        ))
                    } else if empty {
                        row.child(" ")
                    } else {
                        row.child(line.value.clone())
                    };
                    content = content.child(row.when_some(seek, |row, position| {
                        row.cursor_pointer()
                            .hover(|style| {
                                style
                                    .bg(ink_color.opacity(0.07 * edge))
                                    .text_color(ink_color.opacity(shade.max(0.8 * edge)))
                            })
                            .on_click(move |_, _, _| {
                                player.send(PlayerCommand::Seek(position));
                            })
                    }));
                }
                if !lyrics.is_timed() {
                    content = content.child(
                        div()
                            .mt_3()
                            .px(px(row_x))
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
                content = content.p_3().child(
                    div()
                        .px(px(row_x))
                        .text_size(px(if highlight { 15. } else { 12. }))
                        .text_color(colors.muted)
                        .child(message),
                );
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
    fn now_playing_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = self.playback.track.clone() else {
            return self.empty(
                "Play an album or track to see its artwork, details, and lyrics here.",
                cx,
            );
        };
        let colors = ink(cx);
        let dark = cx.theme().is_dark();
        if !track.album_id.is_empty() && self.highlight_requested.as_ref() != Some(&track.album_id)
        {
            self.highlight_requested = Some(track.album_id.clone());
            self.highlight_album = None;
            self.album_tracks_expanded = false;
            self.service.send(Command::Album(track.album_id.clone()));
        }
        let album_artists = track.album_artist_credits();
        self.request_popularity(&album_artists);
        let popular = self.popular_tracks(&track.album_id, &album_artists);
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

        let backdrop = self.backdrop(&track.cover_art, window, cx);
        let palette = self
            .backdrops
            .get(&track.cover_art)
            .map(|backdrop| backdrop.palette);

        let cover_side = (f32::from(window.viewport_size().width) * 0.3).clamp(260., 360.);
        let cover = match self.artwork.get(&track.cover_art) {
            Some(path) => img(path.clone())
                .image_cache(&self.image_cache)
                .size(px(cover_side))
                .rounded_xl()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            None => self.art(&track.cover_art, cover_side, cx),
        };

        let current = album_tracks.iter().position(|item| item.id == track.id);
        let collapsed = !self.album_tracks_expanded && album_tracks.len() > ALBUM_PREVIEW_ROWS;
        let visible = if collapsed {
            let start = current
                .unwrap_or(0)
                .saturating_sub(1)
                .min(album_tracks.len() - ALBUM_PREVIEW_ROWS);
            start..start + ALBUM_PREVIEW_ROWS
        } else {
            0..album_tracks.len()
        };
        let mut album_list = div().flex().flex_col().gap_1();
        for index in visible {
            let item = album_tracks[index].clone();
            album_list = album_list.child(self.track_row(
                item.clone(),
                item.track.unwrap_or(index as u32 + 1) as usize,
                format!("highlight-album-{index}"),
                None,
                Some(popular.contains(&item.id)),
                cx,
            ));
        }
        if album_tracks.len() > ALBUM_PREVIEW_ROWS {
            album_list = album_list.child(
                Button::new("album-tracks-toggle")
                    .ghost()
                    .small()
                    .label(if self.album_tracks_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {} tracks", album_tracks.len())
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.album_tracks_expanded = !app.album_tracks_expanded;
                        cx.notify();
                    })),
            );
        }

        let up_next = self.up_next(&album_tracks, cx);
        let hero = div()
            .id("now-playing-album")
            .w(px(cover_side + 64.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .overflow_y_scroll()
            .px_8()
            .pt_8()
            .pb_4()
            .child(
                div()
                    .size(px(cover_side))
                    .rounded_xl()
                    .shadow_2xl()
                    .child(cover),
            )
            .child(
                div()
                    .mt_6()
                    .text_size(px(24.))
                    .line_height(px(30.))
                    .font_weight(FontWeight::BOLD)
                    .child(track.title.clone()),
            )
            .child(
                div()
                    .mt_1()
                    .text_size(px(15.))
                    .text_color(colors.text.opacity(0.85))
                    .child(self.artist_links(
                        track.artist_credits(),
                        format!("now-playing-artist-{}", track.id),
                        cx,
                    )),
            )
            .child(
                div()
                    .mt_3()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(colors.muted)
                    .child(
                        div()
                            .text_color(colors.text.opacity(0.8))
                            .child(track.album.clone()),
                    )
                    .child("·")
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
                    .mt_1()
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
                    .mt_1()
                    .text_size(px(10.))
                    .text_color(colors.muted)
                    .child(technical(&track)),
            )
            .child(
                div()
                    .mt_6()
                    .mb_2()
                    .text_size(px(10.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.muted)
                    .child("ON THIS ALBUM"),
            )
            .child(album_list)
            .children(up_next);
        let lyrics = self.lyrics_content(track.clone(), true, window, cx);
        let lyrics = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .pl_4()
            .pr_8()
            .pt_6()
            .child(lyrics);

        let spectrum = self
            .preferences
            .layout
            .show_spectrum
            .then(|| self.spectrum_band(palette, dark, window, cx));
        div()
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(colors.background)
            .child(backdrop)
            .child(
                div()
                    .relative()
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .when_some(
                        self.error.clone().or_else(|| self.playback.error.clone()),
                        |page, error| {
                            page.child(
                                div()
                                    .px_8()
                                    .py_2()
                                    .text_size(px(11.))
                                    .text_color(colors.accent)
                                    .child(error),
                            )
                        },
                    )
                    .child(div().flex_1().min_h_0().flex().child(hero).child(lyrics))
                    .children(spectrum),
            )
            .into_any_element()
    }
    /// Queued tracks after the current one, unless the queue is just this album.
    fn up_next(
        &mut self,
        album_tracks: &[Arc<Track>],
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let queue = self.playback.queue.clone();
        let same_as_album = queue.len() == album_tracks.len()
            && queue.iter().zip(album_tracks).all(|(a, b)| a.id == b.id);
        let start = self.playback.index.map_or(0, |index| index + 1);
        if same_as_album || start >= queue.len() {
            return None;
        }
        let colors = ink(cx);
        let upcoming = queue.len() - start;
        let shown = if self.up_next_expanded {
            upcoming
        } else {
            upcoming.min(UP_NEXT_PREVIEW_ROWS)
        };
        let mut list = div().flex().flex_col().gap_1();
        for index in start..start + shown {
            list = list.child(self.track_row(
                queue[index].clone(),
                index + 1,
                format!("now-up-next-{index}"),
                Some(index),
                None,
                cx,
            ));
        }
        if upcoming > UP_NEXT_PREVIEW_ROWS {
            list = list.child(
                Button::new("up-next-toggle")
                    .ghost()
                    .small()
                    .label(if self.up_next_expanded {
                        "Show fewer".to_string()
                    } else {
                        format!("Show all {upcoming} upcoming")
                    })
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.up_next_expanded = !app.up_next_expanded;
                        cx.notify();
                    })),
            );
        }
        Some(
            div()
                .child(
                    div()
                        .mt_6()
                        .mb_2()
                        .flex()
                        .justify_between()
                        .text_size(px(10.))
                        .text_color(colors.muted)
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("UP NEXT"))
                        .child(format!("{upcoming} in queue")),
                )
                .child(list)
                .into_any_element(),
        )
    }
    /// Blurred cover layers under a theme-colored scrim, crossfading on change.
    fn backdrop(&mut self, cover: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let dark = cx.theme().is_dark();
        if !self.backdrops.contains_key(cover)
            && self.artwork.contains_key(cover)
            && self.backdrop_requested.insert(cover.to_string())
        {
            self.service.send(Command::Backdrop(cover.to_string()));
        }
        let ready = self
            .backdrops
            .contains_key(cover)
            .then(|| cover.to_string());
        if ready != self.backdrop_shown {
            self.backdrop_previous = self.backdrop_shown.take();
            self.backdrop_shown = ready;
            self.backdrop_changed = Instant::now();
        }
        let progress = if cx.reduce_motion() {
            1.
        } else {
            (self.backdrop_changed.elapsed().as_secs_f32() / BACKDROP_FADE_SECONDS).min(1.)
        };
        if progress < 1. {
            window.request_animation_frame();
        } else {
            self.backdrop_previous = None;
        }
        let layer = |id: &Option<String>, opacity: f32| {
            id.as_ref()
                .and_then(|id| self.backdrops.get(id))
                .map(|backdrop| {
                    img(backdrop.path.clone())
                        .image_cache(&self.image_cache)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .opacity(opacity)
                })
        };
        let eased = 1. - (1. - progress).powi(3);
        div()
            .absolute()
            .inset_0()
            .children(layer(&self.backdrop_previous, 1.))
            .children(layer(&self.backdrop_shown, eased))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(colors.background.opacity(if dark { 0.55 } else { 0.62 })),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h(px(220.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(colors.background.opacity(0.), 0.),
                        linear_color_stop(colors.background.opacity(0.7), 1.),
                    )),
            )
            .into_any_element()
    }
    fn spectrum_band(
        &mut self,
        palette: Option<[u32; 3]>,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = ink(cx);
        let now = Instant::now();
        let dt = self
            .spectrum_tick
            .map_or(0., |tick| now.duration_since(tick).as_secs_f32());
        self.spectrum_tick = Some(now);
        let rate = self
            .service
            .player
            .spectrum_input(&mut self.spectrum_window);
        let moving = self
            .spectrum
            .update(rate.map(|rate| (&*self.spectrum_window, rate)), dt);
        if moving && window.is_visible() {
            self.request_frame_after(SPECTRUM_FRAME, window, cx);
        } else if !moving {
            self.spectrum_tick = None;
        }
        let (low, high) = spectrum_colors(palette, colors.accent, dark);
        let height = SPECTRUM_HEIGHT;
        let levels = *self.spectrum.levels();
        let peaks = *self.spectrum.peaks();
        // One canvas instead of ~100 positioned divs keeps per-frame layout cheap.
        let bars = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let gap = 3.;
                let width = f32::from(bounds.size.width);
                let bar =
                    ((width - gap * (SPECTRUM_BANDS - 1) as f32) / SPECTRUM_BANDS as f32).max(1.);
                let bottom = f32::from(bounds.bottom());
                for band in 0..SPECTRUM_BANDS {
                    let color = mix(low, high, band as f32 / (SPECTRUM_BANDS - 1) as f32);
                    let left = f32::from(bounds.left()) + band as f32 * (bar + gap);
                    let level = (levels[band] * height).max(2.);
                    window.paint_quad(
                        fill(
                            Bounds::new(
                                point(px(left), px(bottom - level)),
                                size(px(bar), px(level)),
                            ),
                            linear_gradient(
                                0.,
                                linear_color_stop(color.opacity(0.25), 0.),
                                linear_color_stop(color.opacity(0.95), 1.),
                            ),
                        )
                        .corner_radii(Corners {
                            top_left: px(2.),
                            top_right: px(2.),
                            bottom_left: px(0.),
                            bottom_right: px(0.),
                        }),
                    );
                    let peak = peaks[band];
                    if peak > 0.02 {
                        let top = bottom - peak * height - 4.;
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(left), px(top)), size(px(bar), px(2.))),
                                color.opacity(0.9),
                            )
                            .corner_radii(px(1.)),
                        );
                    }
                }
            },
        )
        .w_full()
        .h(px(height));
        let mut scale = div().relative().h(px(14.)).mt_1();
        for (hz, label) in [
            (60., "60"),
            (250., "250"),
            (1_000., "1k"),
            (4_000., "4k"),
            (10_000., "10k"),
        ] {
            scale = scale.child(
                div()
                    .absolute()
                    .left(relative(frequency_position(hz)))
                    .ml(px(-8.))
                    .w(px(16.))
                    .flex()
                    .justify_center()
                    .text_size(px(9.))
                    .text_color(colors.muted.opacity(0.8))
                    .child(label),
            );
        }
        div()
            .id("output-visualizer")
            .role(Role::Group)
            .aria_label("Audio spectrum")
            .flex_shrink_0()
            .relative()
            .px_8()
            .pt_2()
            .pb_3()
            .when(self.playback.buffering, |band| {
                band.child(
                    div()
                        .absolute()
                        .top_0()
                        .right(px(32.))
                        .text_size(px(10.))
                        .text_color(colors.muted)
                        .child("Buffering…"),
                )
            })
            .child(bars)
            .child(scale)
            .into_any_element()
    }
    fn playlists_view(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        if let Some(playlist) = self.playlist.clone() {
            let tracks = playlist.entry.clone();
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
                                        .send(PlayerCommand::Replace(play.as_ref().clone(), 0))
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
        let app = cx.entity().downgrade();
        div()
            .id("settings")
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                Settings::new("sonora-settings")
                    .small()
                    .default_selected_index(SelectIndex {
                        page_ix: self.settings_page,
                        group_ix: None,
                    })
                    .page(self.library_settings(app.clone()))
                    .page(self.playback_settings(app.clone()))
                    .page(self.download_settings(app.clone()))
                    .page(self.appearance_settings(app.clone()))
                    // Keep the Layout page at LAYOUT_SETTINGS_PAGE.
                    .page(self.layout_settings(app)),
            )
            .into_any_element()
    }
    fn library_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let signed_in = self
            .account
            .as_ref()
            .map(|account| format!("Signed in as {} · {}", account.username, account.url));
        let server = self.account.as_ref().map(|account| account.url.clone());
        let connecting = self.status == "Connecting…";
        SettingPage::new("Library")
            .icon(Icon::new(Glyph::Library))
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Connection")
                    .description(signed_in.unwrap_or_else(|| "Not connected".into()))
                    .item(credential_field(
                        "Server address",
                        self.server_url.clone(),
                        None,
                        &["navidrome", "url", "host"],
                    ))
                    .item(credential_field(
                        "Username",
                        self.username.clone(),
                        None,
                        &["account"],
                    ))
                    .item(credential_field(
                        "Password",
                        self.password.clone(),
                        None,
                        &["token", "sign in"],
                    ))
                    .item(
                        SettingItem::render(move |_, _, _| {
                            let app = app.clone();
                            Button::new("connect-library")
                                .primary()
                                .label("Connect")
                                .disabled(connecting)
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        app.status = "Connecting…".into();
                                        app.error = None;
                                        app.service.send(Command::Connect(
                                            Account {
                                                url: app.server_url.read(cx).value().to_string(),
                                                username: app.username.read(cx).value().to_string(),
                                            },
                                            app.password.read(cx).value().to_string(),
                                        ));
                                        cx.notify();
                                    });
                                })
                        })
                        .keywords(["connect", "sign in", "library"]),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Scrobbling")
                    .description("Sonora reports plays only to Navidrome. Link Last.fm in your server's personal settings to forward them. Artist information follows the server's metadata-agent order. Uncertain submissions are not resent, to avoid duplicates.")
                    .item(SettingItem::render(move |_, _, cx| {
                        server.as_ref().map_or_else(
                            || {
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Connect to open your server")
                                    .into_any_element()
                            },
                            |url| {
                                Link::new("open-navidrome")
                                    .href(url.clone())
                                    .child("Open Navidrome")
                                    .into_any_element()
                            },
                        )
                    }).keywords(["last.fm", "scrobble", "navidrome"])),
            )
    }
    fn download_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let status = match &self.slskd_status {
            Some(Ok(message)) => message.clone(),
            Some(Err(message)) => format!("Not linked · {message}"),
            None => "Sonora searches Soulseek only when you ask it to, one release at a time, and slskd does the downloading.".into(),
        };
        SettingPage::new("Downloads")
            .icon(Icon::new(Glyph::Save))
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("slskd")
                    .description(status)
                    .item(
                        SettingItem::new(
                            "Offer missing releases for download",
                            SettingField::switch(
                                {
                                    let app = app.clone();
                                    move |cx: &App| {
                                        app.read_with(cx, |app, _| {
                                            app.preferences.downloads.enabled
                                        })
                                        .unwrap_or(false)
                                    }
                                },
                                {
                                    let app = app.clone();
                                    move |value, cx: &mut App| {
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.downloads.enabled = value;
                                            app.save_preferences();
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value(false),
                        )
                        .description("Adds Find on Soulseek to missing releases on artist pages."),
                    )
                    .item(credential_field(
                        "slskd address",
                        self.slskd_url.clone(),
                        None,
                        &["soulseek", "slskd", "url", "host"],
                    ))
                    .item(credential_field(
                        "API key",
                        self.slskd_key.clone(),
                        Some("A key with the readwrite role from slskd's web.authentication.api_keys, if you have set one up."),
                        &["soulseek", "slskd", "token"],
                    ))
                    .item(credential_field(
                        "Username",
                        self.slskd_username.clone(),
                        Some("Without an API key, sign in with the username and password of slskd's web page."),
                        &["soulseek", "slskd", "login"],
                    ))
                    .item(credential_field(
                        "Password",
                        self.slskd_password.clone(),
                        Some("Leave all three empty to keep the saved login, or if your slskd needs none. Saved in a private file on this Mac."),
                        &["soulseek", "slskd", "login"],
                    ))
                    .item(
                        SettingItem::render(move |_, _, _| {
                            let app = app.clone();
                            Button::new("link-slskd")
                                .primary()
                                .label("Save and test")
                                .on_click(move |_, window, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        app.preferences.downloads.url =
                                            app.slskd_url.read(cx).value().trim().to_string();
                                        app.preferences.downloads.enabled = true;
                                        app.save_preferences();
                                        let fields = [
                                            app.slskd_key.clone(),
                                            app.slskd_username.clone(),
                                            app.slskd_password.clone(),
                                        ];
                                        let [key, username, password] = fields
                                            .each_ref()
                                            .map(|field| field.read(cx).value().to_string());
                                        app.service
                                            .send(Command::SlskdAuth(key, username, password));
                                        for field in fields {
                                            field.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                        }
                                        app.slskd_status = Some(Ok("Testing…".into()));
                                        app.service.send(Command::SlskdTest);
                                        cx.notify();
                                    });
                                })
                        })
                        .keywords(["soulseek", "slskd", "download", "test"]),
                    ),
            )
    }
    fn playback_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let gain = self.preferences.gain.clone();
        let selected = gain_index(&gain);
        SettingPage::new("Playback")
            .icon(Icon::new(Glyph::AudioLines))
            .group(
                SettingGroup::new().title("ReplayGain").item(
                    SettingItem::new(
                        "Loudness",
                        SettingField::render({
                            let app = app.clone();
                            move |_, _, _| {
                                let app = app.clone();
                                RadioGroup::vertical("replay-gain")
                                    .selected_index(Some(selected))
                                    .child(Radio::new("gain-smart").label("Smart"))
                                    .child(Radio::new("gain-track").label("Track"))
                                    .child(Radio::new("gain-album").label("Album"))
                                    .child(Radio::new("gain-off").label("Off"))
                                    .on_change(move |index, _, cx| {
                                        let mode = gain_from_index(*index);
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.gain = mode;
                                            app.save_preferences();
                                            cx.notify();
                                        });
                                    })
                            }
                        })
                        .on_reset(
                            {
                                let app = app.clone();
                                move |cx| {
                                    app.read_with(cx, |app, _| {
                                        app.preferences.gain != GainMode::Smart
                                    })
                                    .unwrap_or(false)
                                }
                            },
                            move |_, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.preferences.gain = GainMode::Smart;
                                    app.save_preferences();
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .description(gain_description(&gain))
                    .layout(Axis::Vertical)
                    .keywords([
                        "replaygain",
                        "volume",
                        "loudness",
                        "normalize",
                    ]),
                ),
            )
    }
    fn appearance_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let dark = self.preferences.dark;
        let selected = match dark {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        };
        SettingPage::new("Appearance")
            .icon(Icon::new(Glyph::SunMoon))
            .group(
                SettingGroup::new().item(
                    SettingItem::new(
                        "Theme",
                        SettingField::render({
                            let app = app.clone();
                            move |_, _, _| {
                                let app = app.clone();
                                RadioGroup::vertical("appearance")
                                    .selected_index(Some(selected))
                                    .child(Radio::new("appearance-system").label("System"))
                                    .child(Radio::new("appearance-light").label("Light"))
                                    .child(Radio::new("appearance-dark").label("Dark"))
                                    .on_change(move |index, window, cx| {
                                        let dark = match *index {
                                            1 => Some(false),
                                            2 => Some(true),
                                            _ => None,
                                        };
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.dark = dark;
                                            app.apply_theme(window, cx);
                                            app.save_preferences();
                                            cx.notify();
                                        });
                                    })
                            }
                        })
                        .on_reset(
                            {
                                let app = app.clone();
                                move |cx| {
                                    app.read_with(cx, |app, _| app.preferences.dark.is_some())
                                        .unwrap_or(false)
                                }
                            },
                            move |window, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.preferences.dark = None;
                                    app.apply_theme(window, cx);
                                    app.save_preferences();
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .description("System follows the appearance of this computer.")
                    .layout(Axis::Vertical)
                    .keywords(["dark", "light", "system", "theme"]),
                ),
            )
    }
    fn layout_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let layout = self.preferences.layout.clone();
        SettingPage::new("Layout")
            .icon(Icon::new(Glyph::PanelsTopLeft))
            .description(
                "These choices are also in the layout menu. Drag the panel edges to resize them.",
            )
            .group(
                SettingGroup::new()
                    .title("Panels")
                    .item(
                        layout_choice(
                            app.clone(),
                            "Navigator and artist browser",
                            "The library list on the left. Auto-hide keeps it out of the way until the pointer reaches the left edge.",
                            [
                                ("show", "Show"),
                                ("auto", "Auto-hide"),
                                ("disabled", "Disabled"),
                            ],
                            "show",
                            navigator_key(&layout),
                            set_navigator,
                        )
                        .keywords(["sidebar", "hover", "auto-hide", "hide"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Queue and information",
                            "The panel on the right.",
                            true,
                            false,
                            |layout| layout.show_information,
                            |layout, value| layout.show_information = value,
                        )
                        .keywords(["sidebar", "lyrics"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Column browser",
                            "Genre, artist, and album columns in the music explorer.",
                            true,
                            false,
                            |layout| layout.explorer_browser,
                            |layout, value| layout.explorer_browser = value,
                        )
                        .keywords(["filter", "genre"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Spectrum visualizer",
                            "Frequency bars in Now Playing. Turn off to save energy.",
                            true,
                            false,
                            |layout| layout.show_spectrum,
                            |layout, value| layout.show_spectrum = value,
                        )
                        .keywords([
                            "visualizer",
                            "spectrum",
                            "cpu",
                            "battery",
                            "now playing",
                        ]),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Track information")
                    .item(
                        layout_choice(
                            app.clone(),
                            "Shows",
                            "The track the information panel follows.",
                            [("selected", "Selected track"), ("playing", "Playing track")],
                            "selected",
                            info_source_key(layout.info_source),
                            |layout, key| {
                                layout.info_source = if key == "playing" {
                                    InfoSource::Playing
                                } else {
                                    InfoSource::Selected
                                };
                            },
                        )
                        .keywords(["selected", "playing", "now playing"]),
                    )
                    .item(
                        layout_choice(
                            app.clone(),
                            "Tab",
                            "Details, lyrics, or artwork.",
                            [
                                ("details", "Details"),
                                ("lyrics", "Lyrics"),
                                ("artwork", "Artwork"),
                            ],
                            "details",
                            info_tab_key(layout.info_tab),
                            |layout, key| {
                                layout.info_tab = match key {
                                    "lyrics" => InfoTab::Lyrics,
                                    "artwork" => InfoTab::Artwork,
                                    _ => InfoTab::Details,
                                };
                            },
                        )
                        .keywords(["lyrics", "artwork", "details"]),
                    ),
            )
            .group(
                SettingGroup::new().item(
                    SettingItem::render({
                        let app = app.clone();
                        move |_, _, _| {
                            let app = app.clone();
                            Button::new("restore-layout-settings")
                                .small()
                                .label("Restore initial panel layout")
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, |app, cx| app.restore_layout(cx));
                                })
                        }
                    })
                    .keywords(["reset", "default", "panels", "width"])
                    .on_reset(
                        {
                            let app = app.clone();
                            move |cx| {
                                app.read_with(cx, |app, _| {
                                    app.preferences.layout != LayoutPreferences::default()
                                })
                                .unwrap_or(false)
                            }
                        },
                        move |_, cx| {
                            let _ = app.update(cx, |app, cx| app.restore_layout(cx));
                        },
                    ),
                ),
            )
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
        let cover = track.as_ref().map(|track| {
            let side = if compact { 32. } else { 40. };
            div()
                .id("footer-cover")
                .group("footer-cover")
                .relative()
                .flex_shrink_0()
                .size(px(side))
                .cursor_pointer()
                .role(Role::Button)
                .aria_label("Open Now Playing")
                .tooltip(|window, cx| tooltip::Tooltip::new("Open Now Playing").build(window, cx))
                .child(self.art(&track.cover_art, side, cx))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.5))
                        .text_color(gpui_kit::white())
                        .opacity(0.)
                        .group_hover("footer-cover", |overlay| overlay.opacity(1.))
                        .child(Icon::new(Glyph::Maximize).size(px(side * 0.45))),
                )
                .on_click(cx.listener(|app, _, window, cx| {
                    app.switch_workspace(Workspace::NowPlaying, window, cx);
                    cx.notify();
                }))
        });
        let favorite = track
            .as_ref()
            .map(|track| self.favorite_button(track, "footer", cx));
        let rating = track.as_ref().map(|track| {
            if compact {
                self.rating_menu(track, "footer", cx).into_any_element()
            } else {
                self.rating_stars(track, "footer", cx).into_any_element()
            }
        });
        let time = |seconds| {
            div()
                .w(px(34.))
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(colors.muted)
                .child(duration_label(seconds))
        };
        let muted = self.preferences.volume <= 0.;
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
            .gap(px(if compact { 8. } else { 16. }))
            .child(
                div()
                    .w(px((width * 0.2).clamp(120., 260.)))
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
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .child(
                                        div().min_w_0().text_size(px(11.)).truncate().child(title),
                                    )
                                    .when_some(favorite, |row, favorite| {
                                        row.child(favorite.h(px(18.)))
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(colors.muted)
                                    .min_w_0()
                                    .child(artist_caption),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_shrink_0()
                            .gap(px(2.))
                            .child(
                                short_button(
                                    "shuffle",
                                    Glyph::Shuffle,
                                    if self.preferences.shuffle {
                                        "Shuffle: on"
                                    } else {
                                        "Shuffle: off"
                                    },
                                )
                                .selected(self.preferences.shuffle)
                                .on_click(cx.listener(
                                    |app, _, _, cx| {
                                        app.preferences.shuffle = !app.preferences.shuffle;
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            )
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
                                if self.playback.playing {
                                    "Pause"
                                } else {
                                    "Play"
                                },
                                PlayerCommand::Toggle,
                            ))
                            .child(self.player_button(
                                "next",
                                Glyph::SkipForward,
                                "Next",
                                PlayerCommand::Next,
                            ))
                            .child(
                                short_button(
                                    "repeat",
                                    if self.preferences.repeat == RepeatMode::One {
                                        Glyph::Repeat1
                                    } else {
                                        Glyph::Repeat
                                    },
                                    match self.preferences.repeat {
                                        RepeatMode::Off => "Repeat: off",
                                        RepeatMode::All => "Repeat: all",
                                        RepeatMode::One => "Repeat: one",
                                    },
                                )
                                .selected(self.preferences.repeat != RepeatMode::Off)
                                .on_click(cx.listener(
                                    |app, _, _, cx| {
                                        app.preferences.repeat = match app.preferences.repeat {
                                            RepeatMode::Off => RepeatMode::All,
                                            RepeatMode::All => RepeatMode::One,
                                            RepeatMode::One => RepeatMode::Off,
                                        };
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(time(self.playback.position).text_right())
                    .child(
                        Slider::new(&self.seek)
                            .flex_1()
                            .min_w(px(40.))
                            .accessibility_label("Playback position"),
                    )
                    .child(time(self.playback.duration)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap(px(if compact { 4. } else { 10. }))
                    .when_some(rating, |row, rating| row.child(rating))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                short_button(
                                    "mute",
                                    if muted {
                                        Glyph::VolumeX
                                    } else {
                                        Glyph::Volume2
                                    },
                                    if muted { "Unmute" } else { "Mute" },
                                )
                                .on_click(cx.listener(
                                    |app, _, window, cx| {
                                        let volume = match app.volume_before_mute.take() {
                                            Some(previous) => previous,
                                            None if app.preferences.volume > 0. => {
                                                app.volume_before_mute =
                                                    Some(app.preferences.volume);
                                                0.
                                            }
                                            None => Preferences::default().volume,
                                        };
                                        app.preferences.volume = volume;
                                        app.volume.update(cx, |slider, cx| {
                                            slider.set_value(volume, window, cx)
                                        });
                                        app.save_preferences();
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                Slider::new(&self.volume)
                                    .w(px(if compact { 44. } else { 76. }))
                                    .accessibility_label("Volume"),
                            ),
                    ),
            )
            .into_any_element()
    }
    /// The unfinished Soulseek download that this library album is part of, if any.
    fn album_download(&self, album: &Album) -> Option<&DownloadJob> {
        let title = sonora::discography::fold(&album.name);
        let artists = album
            .artist_credits()
            .iter()
            .map(|credit| format!("{}\n", normalize(&credit.name)))
            .collect::<Vec<_>>();
        self.downloads.iter().find(|job| {
            job.state != JobState::Downloaded
                && sonora::discography::fold(&job.title) == title
                && artists.iter().any(|artist| job.artist.starts_with(artist))
        })
    }
    /// Server reachability at the start of the status bar, in the same quiet dot style
    /// for every state; offline adds amber text and retries on click.
    fn connection_indicator(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dot = |color: u32| {
            div()
                .size(px(7.))
                .rounded_full()
                .flex_shrink_0()
                .bg(rgb(color))
        };
        let quiet = |id: &'static str, color: u32, label: &'static str, hint: &'static str| {
            div()
                .id(id)
                .mr_2()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(dot(color))
                .child(label)
                .tooltip(move |window, cx| tooltip::Tooltip::new(hint).build(window, cx))
                .into_any_element()
        };
        match &self.connection {
            Connection::None => None,
            Connection::Online => Some(quiet(
                "server-online",
                0x3fa45b,
                "Online",
                "Connected to your Navidrome server",
            )),
            Connection::Connecting => Some(quiet(
                "server-connecting",
                0xd99a2b,
                "Connecting…",
                "Contacting your Navidrome server",
            )),
            Connection::Offline(reason) => {
                let reason: SharedString =
                    format!("{reason}\n\nRetrying automatically. Click to retry now.").into();
                Some(
                    div()
                        .id("server-offline")
                        .mr_2()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .text_color(ink(cx).accent)
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .hover(|label| label.underline())
                        .role(Role::Alert)
                        .aria_label("Server offline. Click to retry now.")
                        .child(dot(0xd99a2b))
                        .child("Server offline · Retry")
                        .tooltip(move |window, cx| {
                            tooltip::Tooltip::new(reason.clone()).build(window, cx)
                        })
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.connection = Connection::Connecting;
                            app.service.send(Command::CheckConnection(true));
                            cx.notify();
                        }))
                        .into_any_element(),
                )
            }
        }
    }
    fn status_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let colors = ink(cx);
        let queue_time = self.playback.queue.iter().map(|t| t.duration).sum::<f64>();
        let library = format!(
            "{} tracks · {} albums",
            self.view.track_count,
            self.view.albums.len()
        );
        // Many releases simply have no cover on the Cover Art Archive; that is not a failure.
        let failed_covers = self
            .art_failures
            .iter()
            .filter(|id| !id.starts_with("remote:https://coverartarchive.org/"))
            .count();
        let artwork_status = if failed_covers == 0 {
            String::new()
        } else {
            format!(" · {failed_covers} covers failed; Refresh to retry")
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
            .children(self.connection_indicator(cx))
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
                    || !self.query.record_label.is_empty()
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
                        .id("error-banner")
                        .role(Role::Alert)
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
                                    let ids =
                                        app.playlist_editor_tracks.clone().unwrap_or_default();
                                    app.mutate(Mutation::CreatePlaylist(name, ids), cx);
                                }
                            })),
                    )
                    .child(
                        short_button("cancel-playlist-editor", Glyph::X, "Cancel").on_click(
                            cx.listener(|app, _, _, cx| {
                                app.show_playlist_editor = false;
                                app.playlist_editor_tracks = None;
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
        self.image_cache
            .update(cx, |cache, cx| cache.begin_frame(window, cx));
        let colors = ink(cx);
        if !self.settings {
            self.settings_page = 0;
        }
        let body = if self.workspace == Workspace::NowPlaying && !self.settings {
            self.now_playing_view(window, cx)
        } else {
            let floating = self.navigator_floats();
            let docked = self.preferences.layout.show_navigator && !floating;
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
            let left = (docked || slide > 0.001).then(|| self.navigator(cx));
            let center = self.center_panel(cx);
            let right = information.then(|| self.information_sidebar(window, cx));
            let (docked_left, floating_left) = if docked { (left, None) } else { (None, left) };
            // Hidden panels keep stale sizes in a group's state and block dragging,
            // so each arrangement gets its own group with only the panels it shows.
            let right_ix = usize::from(docked) + 1;
            let workspace = h_resizable(SharedString::from(format!(
                "musicbee-workspace-{}-{}",
                u8::from(docked),
                u8::from(information)
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
        if std::mem::take(&mut self.art_wake) {
            self.request_frame_after(ART_SETTLE, window, cx);
        }
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .key_context("Sonora")
            .track_focus(&self.focus)
            .font_family(sonora::platform::UI_FONT)
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
            .on_action(cx.listener(|app, _: &GoBack, window, cx| {
                if app.settings {
                    app.settings = false;
                } else if !app.history_back.is_empty() {
                    app.go_back(window, cx);
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
            .on_action(cx.listener(|app, _: &RestoreLayout, _, cx| app.restore_layout(cx)))
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

#[cfg(test)]
mod tests {
    use super::{gain_from_index, gain_index};
    use sonora::model::GainMode;

    #[test]
    fn replaygain_choices_round_trip() {
        for mode in [
            GainMode::Smart,
            GainMode::Track,
            GainMode::Album,
            GainMode::Off,
        ] {
            assert_eq!(gain_from_index(gain_index(&mode)), mode);
        }
        assert_eq!(gain_from_index(9), GainMode::Smart);
    }
}
