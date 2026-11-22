//! Music Explorer only: no scenario opens an artist page.
use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, ElementId, Entity, TestAppContext,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::{
    browser::Workspace,
    metadata::HealthIssue,
    model::{PlaybackSnapshot, Preferences},
    playback::PlayerCommand,
    service::Event,
    sorting::{Field, Scope},
    ui::MusicApp,
};
use std::{sync::Arc, time::Duration};

fn start_first_tab_cover(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Fixture,
    AnyWindowHandle,
    Entity<MusicApp>,
) {
    let (dir, mut fixture, old_window, old_app) = start(cx);
    cx.update_window(old_window, |_, window, _| window.remove_window())
        .unwrap();
    drop(old_app);
    let mut album = (*fixture.catalog.album("album-1").unwrap().unwrap()).clone();
    album.cover_art = "first-cover".into();
    fixture.catalog.put_albums(&[album], 1).unwrap();
    let mut prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    prefs.tabs.active = 0;
    fixture.catalog.set_state("preferences", &prefs).unwrap();
    fixture.expect("disk-only artwork lookup", |command, _| {
        assert!(
            matches!(command, sonora::service::Command::Artwork(id, None) if id == "first-cover")
        );
        vec![]
    });
    let (window, app) = fixture.mount(cx);
    fixture.settle(window, cx);
    fixture.assert_script_finished();
    (dir, fixture, window, app)
}

fn late_art_wake(cx: &mut TestAppContext) {
    // No input, playback, service event or animation advances the idle page.
    // Deliver the timer one frame late, as on a real event loop.
    cx.executor()
        .dispatcher()
        .as_test()
        .unwrap()
        .scheduler()
        .clock()
        .advance(Duration::from_millis(130));
}

#[gpui_kit::test]
fn first_tab_cover_downloads_after_its_own_settle_timer(cx: &mut TestAppContext) {
    let (dir, mut fixture, window, app) = start_first_tab_cover(cx);
    let path = dir.path().join("cover.png");
    image::RgbImage::from_pixel(32, 32, image::Rgb([70, 130, 180]))
        .save(&path)
        .unwrap();
    fixture.expect("visible artwork after idle wake", move |command, _| {
        assert!(matches!(command, sonora::service::Command::Artwork(id, Some(sonora::artwork::Priority::Visible)) if id == "first-cover"));
        vec![Event::Artwork("first-cover".into(), path, false)]
    });
    late_art_wake(cx);
    fixture.settle(window, cx);
    fixture.assert_script_finished();
    cx.update_window(window, |_, window, cx| {
        assert_eq!(app.read(cx).tab_state().active, 0);
        assert!(window.find("cover-image-first-cover").visible());
    })
    .unwrap();
}

#[gpui_kit::test]
fn failed_first_tab_cover_stays_placeholder_without_repeated_requests(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start_first_tab_cover(cx);
    fixture.expect("failed visible artwork", |command, _| {
        assert!(matches!(command, sonora::service::Command::Artwork(id, Some(sonora::artwork::Priority::Visible)) if id == "first-cover"));
        vec![Event::ArtworkFailed("first-cover".into())]
    });
    late_art_wake(cx);
    fixture.settle(window, cx);
    fixture.assert_script_finished();
    cx.executor().advance_clock(Duration::from_secs(2));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("explorer-album-album-1").visible());
        assert!(window.try_find("cover-image-first-cover").is_none());
    })
    .unwrap();
}

fn start_explorer(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Fixture,
    AnyWindowHandle,
    Entity<MusicApp>,
) {
    let (dir, mut fixture, window, app) = start(cx);
    fixture.seed_explorer();
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.diagnose("Explorer ready", window, &app, cx);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
    (dir, fixture, window, app)
}

fn assert_explorer(
    window: AnyWindowHandle,
    app: &Entity<MusicApp>,
    expected: &[&str],
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let page = app.read(cx).tab_state().active().page.as_ref().unwrap();
        assert_eq!(page.workspace, Workspace::Explorer);
        assert!(
            page.artist.is_none(),
            "scenario must remain in Music Explorer"
        );
        assert!(window.find("main-view-menu").visible());
        let mut cards = window
            .elements()
            .into_iter()
            .filter_map(|element| {
                let ElementId::Name(name) = element.path().last()? else {
                    return None;
                };
                let id = name.strip_prefix("explorer-album-")?.to_string();
                let label = match id.as_str() {
                    "album-1" => "Select album Fixture album by ONE OK ROCK",
                    "album-2" => "Select album Second release by ONE OK ROCK",
                    "album-3" => "Select album Another album by 宇多田ヒカル",
                    _ => panic!("unexpected fixture album card"),
                };
                assert_eq!(element.label(), Some(label));
                element.visible().then_some((element.bounds().origin, id))
            })
            .collect::<Vec<_>>();
        cards.sort_by(|a, b| {
            a.0.y
                .partial_cmp(&b.0.y)
                .unwrap()
                .then_with(|| a.0.x.partial_cmp(&b.0.x).unwrap())
        });
        let ids = cards.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, expected, "visible Explorer albums in layout order");
    })
    .unwrap();
}

#[gpui_kit::test]
fn unicode_search_empty_results_and_clear_recover_in_explorer(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start_explorer(cx);
    fixture.search(window, "宇多田", cx);
    assert_eq!(fixture.last_query().text, "宇多田");
    assert_explorer(window, &app, &["album-3"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("music-search").value(), Some("宇多田"))
    })
    .unwrap();
    fixture.search(window, "nothing-matches-this", cx);
    fixture.diagnose("empty Explorer search", window, &app, cx);
    assert_explorer(window, &app, &[], cx);
    cx.update_window(window, |_, window, _| {
        let empty = window.find("library-empty");
        assert!(empty.visible());
        assert_eq!(
            empty.label(),
            Some("No tracks match the current search or filters.")
        );
    })
    .unwrap();
    fixture.click(window, "empty-clear", cx);
    assert_eq!(fixture.last_query().text, "");
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("music-search").value(), Some(""));
        assert!(window.try_find("library-empty").is_none());
    })
    .unwrap();
}

fn assert_queue_unchanged(window: AnyWindowHandle, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, _| {
        let first = window.find("love-queue-side-0");
        let second = window.find("love-queue-side-1");
        assert_eq!(first.label(), Some("Favorite Fixture song"));
        assert_eq!(second.label(), Some("Favorite Another song"));
        assert!(first.visible() && second.visible());
        assert!(first.bounds().origin.y < second.bounds().origin.y);
        assert!(window.try_find("love-queue-side-2").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn genre_year_artist_filters_clear_without_changing_the_queue(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start_explorer(cx);
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        queue: Arc::new(
            fixture
                .catalog
                .by_ids(&["track-1".into(), "track-3".into()])
                .unwrap(),
        ),
        queue_ids: Arc::new(vec![41, 42]),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    assert_queue_unchanged(window, cx);

    fixture.click(window, "browser-value-Genre-Rock", cx);
    assert_eq!(fixture.last_query().selection.genres, ["Rock"]);
    assert_explorer(window, &app, &["album-1", "album-2"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("browser-value-Genre-Rock").selected(),
            Some(true)
        )
    })
    .unwrap();
    fixture.click(window, "browser-value-Album Artist-ONE OK ROCK", cx);
    assert_eq!(fixture.last_query().selection.artists, ["ONE OK ROCK"]);
    assert_explorer(window, &app, &["album-1", "album-2"], cx);
    fixture.click(window, "clear-filter", cx);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
    assert!(fixture.last_query().selection.is_empty());

    fixture.click(window, "navigator-mode", cx);
    fixture.menu_item(window, "Year", cx);
    fixture.click(window, "navigator-facet-Year-2024", cx);
    assert_eq!(fixture.last_query().year, Some(2024));
    assert_explorer(window, &app, &["album-1", "album-3"], cx);
    fixture.click(window, "browser-value-Album Artist-ONE OK ROCK", cx);
    assert_explorer(window, &app, &["album-1"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("navigator-facet-Year-2024").selected(),
            Some(true)
        );
        assert_eq!(
            window
                .find("browser-value-Album Artist-ONE OK ROCK")
                .selected(),
            Some(true)
        );
        assert!(window.find("clear-year-filter").visible());
    })
    .unwrap();
    fixture.click(window, "clear-year-filter", cx);
    assert_eq!(fixture.last_query().year, None);
    assert_explorer(window, &app, &["album-1", "album-2"], cx);
    fixture.click(window, "clear-filter", cx);
    fixture.diagnose("Explorer filters cleared", window, &app, cx);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("all-Genre").selected(), Some(true));
        assert_eq!(window.find("all-Album Artist").selected(), Some(true));
    })
    .unwrap();
    assert_queue_unchanged(window, cx);
    for command in fixture.backend.player_commands().try_iter() {
        assert!(
            matches!(command, PlayerCommand::Preferences(_)),
            "filters sent a playback command"
        );
    }
}

#[gpui_kit::test]
fn explorer_sort_and_direction_survive_catalog_reload(cx: &mut TestAppContext) {
    let (dir, mut fixture, window, app) = start_explorer(cx);
    fixture.click(window, "sort-menu", cx);
    fixture.menu_item(window, "Album title", cx);
    assert_explorer(window, &app, &["album-3", "album-1", "album-2"], cx);
    fixture.click(window, "sort-menu", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window
                .within("popup-menu")
                .find_by_label("Album title")
                .checked(),
            Some(true)
        )
    })
    .unwrap();
    fixture.menu_item(window, "Album title: Z–A", cx);
    assert_explorer(window, &app, &["album-2", "album-1", "album-3"], cx);
    assert_eq!(
        fixture.last_query().ordering.as_ref().unwrap().rules[0].field,
        Field::Album
    );
    assert!(fixture.last_query().ordering.as_ref().unwrap().rules[0].descending);
    cx.executor().advance_clock(Duration::from_millis(500));
    fixture.settle(window, cx);
    let saved: Preferences = fixture.catalog.state("preferences").unwrap();
    assert!(saved.sorting.get(Scope::Albums).rules[0].descending);
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(fixture);
    cx.run_until_parked();

    let mut restarted = Fixture::open(&dir.path().join("catalog.sqlite"));
    let (window, app) = restarted.mount(cx);
    restarted.settle(window, cx);
    restarted.diagnose("Explorer sort restored", window, &app, cx);
    assert_explorer(window, &app, &["album-2", "album-1", "album-3"], cx);
    restarted.click(window, "sort-menu", cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert_eq!(menu.find_by_label("Album title").checked(), Some(true));
        assert_eq!(menu.find_by_label("Album title: Z–A").checked(), Some(true));
    })
    .unwrap();
}

#[gpui_kit::test]
fn explorer_metadata_filters_and_back_restore_prior_queries(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start_explorer(cx);
    fixture.click(window, "explorer-album-album-1", cx);
    fixture.click(window, "album-genres-album-1-0", cx);
    assert_eq!(fixture.last_query().selection.genres, ["Rock"]);
    assert_explorer(window, &app, &["album-1", "album-2"], cx);
    fixture.click(window, "explorer-album-album-1", cx);
    fixture.click(window, "album-year-album-1", cx);
    assert_eq!(fixture.last_query().year, Some(2024));
    assert!(fixture.last_query().selection.is_empty());
    assert_explorer(window, &app, &["album-1", "album-3"], cx);
    fixture.click(window, "history-back", cx);
    assert_eq!(fixture.last_query().selection.genres, ["Rock"]);
    assert_explorer(window, &app, &["album-1", "album-2"], cx);
    fixture.click(window, "history-back", cx);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
    fixture.click(window, "explorer-album-album-1", cx);
    fixture.click(window, "album-label-album-1-0", cx);
    assert_eq!(fixture.last_query().record_label, "Label A");
    assert_explorer(window, &app, &["album-1", "album-3"], cx);
    fixture.click(window, "history-back", cx);
    fixture.diagnose("Explorer metadata Back restored", window, &app, cx);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
}

#[gpui_kit::test]
fn health_filters_narrow_explorer_and_clear_from_chip_or_menu(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start_explorer(cx);
    let mut undated = (*fixture.catalog.by_ids(&["track-3".into()]).unwrap()[0]).clone();
    undated.year = None;
    let mut guest = (*fixture.catalog.by_ids(&["track-2".into()]).unwrap()[0]).clone();
    guest.id = "track-4".into();
    guest.title = "Guest song".into();
    guest.album_artist = "Guest".into();
    fixture.catalog.put_tracks(&[undated, guest], 1).unwrap();

    fixture.click(window, "health-filter", cx);
    fixture.menu_item(window, "Missing year", cx);
    assert_eq!(fixture.last_query().health, Some(HealthIssue::MissingYear));
    assert_explorer(window, &app, &["album-3"], cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("clear-health-filter").label(),
            Some("Clear missing year filter")
        );
    })
    .unwrap();

    fixture.click(window, "health-filter", cx);
    fixture.menu_item(window, "Mixed album artist", cx);
    assert_eq!(
        fixture.last_query().health,
        Some(HealthIssue::MixedAlbumArtist)
    );
    assert_explorer(window, &app, &["album-2"], cx);

    fixture.click(window, "clear-health-filter", cx);
    assert_eq!(fixture.last_query().health, None);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);

    // A check nothing fails shows the empty state and recovers from the menu.
    fixture.click(window, "health-filter", cx);
    fixture.menu_item(window, "Missing genre", cx);
    fixture.diagnose("health filter with no matches", window, &app, cx);
    assert_explorer(window, &app, &[], cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("library-empty").visible());
    })
    .unwrap();
    fixture.click(window, "health-filter", cx);
    fixture.menu_item(window, "Any tags", cx);
    assert_eq!(fixture.last_query().health, None);
    assert_explorer(window, &app, &["album-1", "album-2", "album-3"], cx);
}

#[gpui_kit::test]
fn stale_search_results_cannot_replace_newer_explorer_results(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start_explorer(cx);
    fixture.hold_queries();
    fixture.search(window, "宇多田", cx);
    assert_eq!(fixture.last_query().text, "宇多田");
    fixture.search(window, "Second", cx);
    assert_eq!(fixture.last_query().text, "Second");
    fixture.deliver_query("Second");
    fixture.settle(window, cx);
    assert_explorer(window, &app, &["album-2"], cx);
    fixture.deliver_query("宇多田");
    fixture.settle(window, cx);
    fixture.diagnose("stale Explorer result ignored", window, &app, cx);
    assert_explorer(window, &app, &["album-2"], cx);
    cx.update_window(window, |_, window, cx| {
        assert_eq!(window.find("music-search").value(), Some("Second"));
        assert_eq!(
            app.read(cx)
                .tab_state()
                .active()
                .page
                .as_ref()
                .unwrap()
                .query
                .text,
            "Second"
        );
    })
    .unwrap();
}
