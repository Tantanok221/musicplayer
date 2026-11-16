//! Production-view interaction tests. Run with `scripts/test-ui.sh`.
//! Responses are controlled; the catalog and preference write are real SQLite.
mod feedback;
mod input;
mod library;
mod playback;
mod seed;
mod tabs;

pub(crate) use playback::PlaybackIntent;
pub(crate) use tabs::{assert_active_tab, assert_rightmost};

use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Entity, Role, TestAppContext, WindowBounds, WindowOptions,
    px, size,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::ui::MusicApp;
use sonora::{
    catalog::Catalog,
    model::LibraryQuery,
    platform::Account,
    service::{Command, Event, Mutation, test_support::ControlledService},
};
use std::{collections::VecDeque, path::Path};

pub(crate) const ARTIST: &str = "ONE OK ROCK";
pub(crate) const ARTIST_TARGET: &str = "navigator-facet-Album Artist-ONE OK ROCK";
pub(crate) const SECOND_ARTIST: &str = "宇多田ヒカル";
pub(crate) const SECOND_ARTIST_TARGET: &str = "navigator-facet-Album Artist-宇多田ヒカル";
pub(crate) const MUTATION_ERROR: &str =
    "Fixture server rejected album update. Retry when connected.";

pub(crate) fn start(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Fixture,
    AnyWindowHandle,
    Entity<MusicApp>,
) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(sonora::ui::tab_bindings());
        cx.bind_keys(sonora::ui::playback_bindings());
        cx.bind_keys(sonora::ui::queue_bindings());
    });
    let dir = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::open(&dir.path().join("catalog.sqlite"));
    fixture.seed();
    let (window, app) = fixture.mount(cx);
    fixture.settle(window, cx);
    fixture.diagnose("ready", window, &app, cx);
    (dir, fixture, window, app)
}

pub(crate) struct Fixture {
    pub(crate) backend: ControlledService,
    pub(crate) catalog: Catalog,
    trace: Vec<String>,
    pub(crate) saves: usize,
    mutations: Vec<Mutation>,
    pending_mutations: VecDeque<Mutation>,
    hold_queries: bool,
    query_requests: Vec<LibraryQuery>,
    held_views: VecDeque<(String, Event)>,
    playback_intents: Option<Vec<PlaybackIntent>>,
    expected: VecDeque<ExpectedCommand>,
}

type Response = Box<dyn FnOnce(Command, &Catalog) -> Vec<Event>>;

struct ExpectedCommand {
    label: &'static str,
    respond: Response,
}

impl Fixture {
    pub(crate) fn open(path: &Path) -> Self {
        Self {
            backend: ControlledService::new(),
            catalog: Catalog::open(path).expect("isolated catalog"),
            trace: vec![],
            saves: 0,
            mutations: vec![],
            pending_mutations: VecDeque::new(),
            hold_queries: false,
            query_requests: vec![],
            held_views: VecDeque::new(),
            playback_intents: None,
            expected: VecDeque::new(),
        }
    }

    /// Scenario-local commands run after the small shared catalog defaults.
    /// Closures assert intent and choose replies; labels are safe diagnostics.
    pub(crate) fn expect(
        &mut self,
        label: &'static str,
        respond: impl FnOnce(Command, &Catalog) -> Vec<Event> + 'static,
    ) {
        self.expected.push_back(ExpectedCommand {
            label,
            respond: Box::new(respond),
        });
    }

    pub(crate) fn assert_script_finished(&self) {
        assert!(
            self.expected.is_empty(),
            "unconsumed scenario commands: {:?}",
            self.expected
                .iter()
                .map(|expected| expected.label)
                .collect::<Vec<_>>()
        );
    }

    pub(crate) fn mount(&self, cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<MusicApp>) {
        cx.update(|cx| {
            let (window, app) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Default::default(),
                        size: size(px(1440.), px(900.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        MusicApp::new(self.backend.service(), self.backend.events(), window, cx)
                    })
                },
            )
            .expect("production view window");
            (window, app)
        })
    }

    fn pump(&mut self) -> usize {
        let mut count = 0;
        while let Ok(command) = self.backend.commands().try_recv() {
            count += 1;
            // Allowlisted summaries only: Command includes credentials.
            match command {
                Command::Initialize => {
                    self.trace.push("Initialize -> Ready".into());
                    self.backend.emit(Event::Ready(
                        Some(Account {
                            url: "https://fixture.invalid".into(),
                            username: "fixture".into(),
                        }),
                        self.catalog.state("preferences").unwrap(),
                    ));
                }
                Command::Query(request, query) => self.respond_query(request, query),
                Command::Artist(request, name, id) => self.respond_artist(request, name, id),
                Command::Preferences(prefs) => {
                    self.trace.push(format!(
                        "Preferences tabs={} active={}",
                        prefs.tabs.tabs.len(),
                        prefs.tabs.active
                    ));
                    self.backend
                        .persist_preferences(&self.catalog, prefs)
                        .unwrap();
                    self.saves += 1;
                }
                Command::GroupTracks(request, query, id) => {
                    self.respond_group_tracks(request, query, id)
                }
                Command::Mutate(
                    mutation @ (Mutation::AlbumFavorite(..) | Mutation::AlbumRating(..)),
                ) => self.hold_mutation(mutation),
                Command::ArtistProfile(..) | Command::TopSongs(..) => {
                    self.trace
                        .push("optional artist enrichment deferred".into());
                }
                Command::SaveWindow(_) => self.trace.push("SaveWindow".into()),
                Command::PlayAlbum(id) if self.capturing_playback() => {
                    self.record_playback(PlaybackIntent::PlayAlbum(id))
                }
                Command::EnqueueAlbum(id, placement) if self.capturing_playback() => {
                    self.record_playback(PlaybackIntent::EnqueueAlbum(id, placement))
                }
                command => {
                    let expected = self.expected.pop_front().unwrap_or_else(|| {
                        panic!(
                            "unexpected service command in UI scenario; trace={:?}",
                            self.trace
                        )
                    });
                    self.trace
                        .push(format!("scenario response: {}", expected.label));
                    for event in (expected.respond)(command, &self.catalog) {
                        self.backend.emit(event);
                    }
                }
            }
        }
        count
    }

    pub(crate) fn settle(&mut self, window: AnyWindowHandle, cx: &mut TestAppContext) {
        for _ in 0..32 {
            cx.run_until_parked();
            let count = self.pump();
            cx.run_until_parked();
            cx.update_window(window, |_, window, cx| window.render_frame(cx))
                .unwrap();
            if count == 0 && self.backend.commands().is_empty() {
                return;
            }
        }
        panic!("UI scenario did not settle; trace={:?}", self.trace);
    }

    pub(crate) fn diagnose(
        &self,
        stage: &str,
        window: AnyWindowHandle,
        app: &Entity<MusicApp>,
        cx: &mut TestAppContext,
    ) {
        let scenario = cx.test_function_name().unwrap_or("ui");
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let tabs = app.read(cx).tab_state();
            eprintln!(
                "scenario={scenario} stage={stage} saves={} active={} tab_ids={:?}",
                self.saves,
                tabs.active,
                tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>()
            );
            for element in window
                .elements()
                .into_iter()
                .filter(|element| matches!(element.role(), Some(Role::Tab | Role::Alert)) || element.label().is_some_and(|label| label.contains("album") || label == "Retry"))
            {
                eprintln!(
                    "scenario={scenario} element path={:?} label={:?} selected={:?} disabled={:?} bounds={:?} visible={}",
                    element.path(),
                    element.label(),
                    element.selected(),
                    element.disabled(),
                    element.bounds(),
                    element.visible()
                );
            }
            eprintln!("scenario={scenario} commands={:?}", self.trace);
            eprintln!("scenario={scenario} held album mutations={}", self.pending_mutations.len());
            if let Some(query) = self.query_requests.last() {
                eprintln!("scenario={scenario} query text={:?} genres={:?} artists={:?} year={:?} record_label={:?} sort={:?} held_views={}",
                    query.text, query.selection.genres, query.selection.artists, query.year,
                    query.record_label, query.ordering.as_ref().map(|sort| sort.summary()), self.held_views.len());
            }
        })
        .unwrap();
    }
}
