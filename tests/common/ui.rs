//! Production-view interaction tests. Run with `scripts/test-ui.sh`.
//! Responses are controlled; the catalog and preference write are real SQLite.
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, ElementId, Entity, Role, SharedString, TestAppContext,
    WindowBounds, WindowOptions, px, size,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::ui::MusicApp;
use sonora::{
    catalog::Catalog,
    model::{Album, Preferences, Track},
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
        }
    }

    pub(crate) fn seed(&mut self) {
        self.catalog
            .put_tracks(
                &[
                    Track {
                        id: "track-1".into(),
                        title: "Fixture song".into(),
                        album_id: "album-1".into(),
                        album: "Fixture album".into(),
                        artist: ARTIST.into(),
                        album_artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        duration: 120.,
                        starred: Some("2026-01-01T00:00:00Z".into()),
                        user_rating: Some(5),
                        ..Default::default()
                    },
                    Track {
                        id: "track-2".into(),
                        title: "Second song".into(),
                        album_id: "album-2".into(),
                        album: "Second release".into(),
                        artist: ARTIST.into(),
                        album_artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        duration: 120.,
                        user_rating: Some(1),
                        ..Default::default()
                    },
                    Track {
                        id: "track-3".into(),
                        title: "Another song".into(),
                        album_id: "album-3".into(),
                        album: "Another album".into(),
                        artist: SECOND_ARTIST.into(),
                        album_artist: SECOND_ARTIST.into(),
                        artist_id: "artist-2".into(),
                        duration: 120.,
                        ..Default::default()
                    },
                ],
                1,
            )
            .unwrap();
        self.catalog
            .put_albums(
                &[
                    Album {
                        id: "album-1".into(),
                        name: "Fixture album".into(),
                        artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        user_rating: Some(2),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                    Album {
                        id: "album-2".into(),
                        name: "Second release".into(),
                        artist: ARTIST.into(),
                        artist_id: "artist-1".into(),
                        starred: Some("2026-01-01T00:00:00Z".into()),
                        user_rating: Some(4),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                    Album {
                        id: "album-3".into(),
                        name: "Another album".into(),
                        artist: SECOND_ARTIST.into(),
                        artist_id: "artist-2".into(),
                        user_rating: Some(3),
                        song_count: 1,
                        release_types: vec!["album".into()],
                        ..Default::default()
                    },
                ],
                1,
            )
            .unwrap();
        let mut prefs = Preferences::default();
        // Reproduce creation from a middle tab, with tabs to its right.
        prefs.tabs.active = 1;
        prefs.layout.show_spectrum = false;
        self.catalog.set_state("preferences", &prefs).unwrap();
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
                Command::Query(request, query) => {
                    self.trace.push(format!("Query({request}) -> View"));
                    self.backend
                        .emit(Event::View(request, self.catalog.view(&query).unwrap()));
                }
                Command::Artist(request, name, id) => {
                    self.trace.push(format!("Artist({request}) -> Artist"));
                    assert!(
                        [(ARTIST, "artist-1"), (SECOND_ARTIST, "artist-2")]
                            .contains(&(name.as_str(), id.as_str()))
                    );
                    self.backend.emit(Event::Artist(
                        request,
                        self.catalog.artist_page(&name, &id).unwrap(),
                    ));
                }
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
                    self.trace
                        .push(format!("GroupTracks({request}, {id}) -> GroupTracks"));
                    self.backend.emit(Event::GroupTracks(
                        request,
                        id.clone(),
                        self.catalog.album_tracks_matching(&query, &id).unwrap(),
                    ));
                }
                Command::Mutate(
                    mutation @ (Mutation::AlbumFavorite(..) | Mutation::AlbumRating(..)),
                ) => {
                    self.trace.push(format!("held {mutation:?}"));
                    self.mutations.push(mutation.clone());
                    self.pending_mutations.push_back(mutation);
                }
                Command::ArtistProfile(..) | Command::TopSongs(..) => {
                    self.trace
                        .push("optional artist enrichment deferred".into());
                }
                Command::SaveWindow(_) => self.trace.push("SaveWindow".into()),
                _ => panic!(
                    "unexpected service command in UI scenario; trace={:?}",
                    self.trace
                ),
            }
        }
        count
    }

    pub(crate) fn mutations(&self) -> &[Mutation] {
        &self.mutations
    }

    pub(crate) fn confirm_mutation(&mut self) {
        let mutation = self
            .pending_mutations
            .pop_front()
            .expect("held mutation to confirm");
        let album = match &mutation {
            Mutation::AlbumFavorite(id, favorite) => {
                self.catalog
                    .update_album_feedback(id, Some(*favorite), None)
            }
            Mutation::AlbumRating(id, rating) => {
                self.catalog.update_album_feedback(id, None, Some(*rating))
            }
            _ => unreachable!("only album feedback is held"),
        }
        .unwrap()
        .expect("fixture album");
        self.trace.push("AlbumUpdated -> MutationDone".into());
        self.backend.emit(Event::AlbumUpdated(album));
        self.backend.emit(Event::MutationDone(mutation));
    }

    pub(crate) fn fail_mutation(&mut self) {
        let mutation = self
            .pending_mutations
            .pop_front()
            .expect("held mutation to fail");
        let album = self
            .catalog
            .album(mutation.album_feedback_id().unwrap())
            .unwrap()
            .unwrap();
        self.trace
            .push("original AlbumUpdated -> MutationFailed".into());
        self.backend.emit(Event::AlbumUpdated(album));
        self.backend
            .emit(Event::MutationFailed(mutation, MUTATION_ERROR.into()));
    }

    pub(crate) fn click(
        &mut self,
        window: AnyWindowHandle,
        id: impl Into<ElementId>,
        cx: &mut TestAppContext,
    ) {
        cx.update_window(window, |_, window, cx| window.click(id, cx))
            .unwrap();
        self.settle(window, cx);
    }

    pub(crate) fn press(&mut self, window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        self.settle(window, cx);
    }

    pub(crate) fn open_album(&mut self, window: AnyWindowHandle, cx: &mut TestAppContext) {
        self.click(window, "new-tab", cx);
        self.click(window, SharedString::from(ARTIST_TARGET), cx);
        self.click(window, "artist-release-album-1", cx);
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
        })
        .unwrap();
    }
}

pub(crate) fn assert_active_tab(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    id: u64,
    expected_ids: &[u64],
    title: &str,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let tabs = app.read(cx).tab_state();
        assert_eq!(
            tabs.tabs.iter().map(|tab| tab.id).collect::<Vec<_>>(),
            expected_ids
        );
        assert_eq!(tabs.active().id, id);
        let last = window.find(("workspace-tab", id));
        assert_eq!(last.label(), Some(title));
        assert_eq!(last.selected(), Some(true));
        assert!(last.visible());
        for other in expected_ids.iter().filter(|other| **other != id) {
            assert_eq!(
                window.find(("workspace-tab", *other)).selected(),
                Some(false)
            );
        }
    })
    .unwrap();
}

pub(crate) fn assert_rightmost(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    id: u64,
    expected_ids: &[u64],
    title: &str,
    cx: &mut TestAppContext,
) {
    assert_active_tab(window, app, id, expected_ids, title, cx);
    cx.update_window(window, |_, window, cx| {
        assert_eq!(app.read(cx).tab_state().active, expected_ids.len() - 1);
        let last = window.find(("workspace-tab", id));
        for previous in &expected_ids[..expected_ids.len() - 1] {
            assert!(
                window.find(("workspace-tab", *previous)).bounds().right() <= last.bounds().left()
            );
        }
    })
    .unwrap();
}
