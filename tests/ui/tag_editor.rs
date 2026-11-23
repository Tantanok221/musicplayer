//! The opt-in tag editor: context menu entry, multi-edit form, failure and undo.
use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, ElementId, Modifiers, TestAppContext, test::TestWindowExt,
};
use sonora::{
    autotag::{LookupTrack, Release, ReleaseCandidate, ReleaseQuery},
    model::{MetadataPreferences, Preferences, Track},
    platform::{Account, shortcut},
    service::{Command, Event},
    tags::{EditValue, TagEdit, TagField, TagProgress, TagReport},
};
use std::{collections::BTreeMap, sync::Arc};

fn writes_tags(fixture: &mut Fixture, on: bool) {
    let mut prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    prefs.metadata = MetadataPreferences {
        write_tags: on,
        library_folder: "/Volumes/music".into(),
        ..Default::default()
    };
    fixture.backend.emit(Event::Ready(
        Some(Account {
            url: "https://fixture.invalid".into(),
            username: "fixture".into(),
        }),
        prefs,
    ));
}

fn open_album_editor(fixture: &mut Fixture, window: AnyWindowHandle, cx: &mut TestAppContext) {
    writes_tags(fixture, true);
    fixture.settle(window, cx);
    fixture.expect("album songs for the tag editor", |command, _| {
        assert!(matches!(command, Command::Album(id) if id == "album-1"));
        vec![Event::Album(
            "album-1".into(),
            vec![song("one", "First"), song("two", "Second")],
        )]
    });
    album_menu(fixture, window, cx);
    fixture.menu_item(window, "Edit Album Tags…", cx);
}

fn album_menu(fixture: &mut Fixture, window: AnyWindowHandle, cx: &mut TestAppContext) {
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("explorer-album-album-1", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
}

fn song(id: &str, title: &str) -> Arc<Track> {
    Arc::new(Track {
        id: id.into(),
        title: title.into(),
        album_id: "album-1".into(),
        album: "Fixture album".into(),
        artist: "Fixture Artist".into(),
        genre: "Rock".into(),
        ..Default::default()
    })
}

fn label(
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
    cx: &mut TestAppContext,
) -> Option<String> {
    let id = id.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id)
            .and_then(|element| element.label().map(str::to_string))
    })
    .unwrap()
}

#[gpui_kit::test]
fn edit_menu_is_hidden_until_tag_writing_is_turned_on(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    album_menu(&mut fixture, window, cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert!(menu.try_find_by_label("Edit Album Tags…").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn album_edit_sends_only_the_changed_field_then_retries_and_undoes(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    writes_tags(&mut fixture, true);
    fixture.settle(window, cx);
    fixture.expect("album songs for the tag editor", |command, _| {
        assert!(matches!(command, Command::Album(id) if id == "album-1"));
        vec![Event::Album(
            "album-1".into(),
            vec![song("one", "First"), song("two", "Second")],
        )]
    });
    album_menu(&mut fixture, window, cx);
    fixture.menu_item(window, "Edit Album Tags…", cx);
    assert_eq!(
        label(window, "tag-editor", cx).as_deref(),
        Some("Edit tags of 2 songs")
    );
    let failure = TagReport {
        job: 1,
        failures: vec![("Second".into(), "Couldn't find the file".into())],
        written: 1,
        ..Default::default()
    };
    fixture.expect("write genre only", move |command, _| {
        let Command::WriteTags(mut edits) = command else {
            panic!("expected a tag write");
        };
        assert_eq!(edits.len(), 1);
        let edit = edits.remove(0);
        assert_eq!(edit.tracks, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(
            edit.fields.into_iter().collect::<Vec<_>>(),
            vec![(TagField::Genre, EditValue::Set("Jazz".into()))]
        );
        vec![Event::TagsWritten(failure)]
    });
    cx.update_window(window, |_, window, cx| {
        window.click("tag-genre", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input("Jazz", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "save-tags", cx);
    assert_eq!(
        label(window, ("tag-change", 0_usize), cx).as_deref(),
        Some("First · Genre: Rock → Jazz")
    );
    fixture.click(window, "confirm-tags", cx);
    assert_eq!(
        label(window, "tag-failures", cx).as_deref(),
        Some("Second: Couldn't find the file")
    );
    assert_eq!(
        label(window, "tag-notice", cx).as_deref(),
        Some("Wrote tags to 1 file · 1 not changed")
    );

    fixture.expect("retry", |command, _| {
        assert!(matches!(command, Command::WriteTags(edits) if edits[0].fields.len() == 1));
        vec![Event::TagsWritten(TagReport {
            job: 2,
            written: 2,
            ..Default::default()
        })]
    });
    fixture.click(window, "save-tags", cx);
    assert!(label(window, "tag-editor", cx).is_none());
    assert_eq!(
        label(window, "tag-notice", cx).as_deref(),
        Some("Wrote tags to 2 files")
    );

    fixture.expect("undo", |command, _| {
        assert!(matches!(command, Command::UndoTags));
        vec![]
    });
    fixture.click(window, "undo-tags", cx);
    assert!(label(window, "tag-notice", cx).is_none());
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn command_click_selects_songs_and_the_table_edits_each_one(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    writes_tags(&mut fixture, true);
    fixture.settle(window, cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.click(window, "main-view-menu", cx);
    fixture.menu_item(window, "Tracks", cx);
    fixture.click(window, "row-flat-track-1", cx);
    for id in ["row-flat-track-2", "row-flat-track-3"] {
        cx.update_window(window, |_, window, cx| {
            window.click_with_modifiers(id, Modifiers::command(), cx)
        })
        .unwrap();
        fixture.settle(window, cx);
    }
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-flat-track-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Edit Tags of 3 Songs…", cx);
    assert_eq!(
        label(window, "tag-editor", cx).as_deref(),
        Some("Edit tags of 3 songs")
    );
    let row = ["track-1", "track-2", "track-3"]
        .iter()
        .position(|id| *id == "track-2")
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.click(format!("tag-cell-{row}-title"), cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input("Renamed", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "save-tags", cx);
    assert_eq!(
        label(window, ("tag-change", 0_usize), cx).as_deref(),
        Some("Second song · Title: Second song → Renamed")
    );
    assert!(label(window, ("tag-change", 1_usize), cx).is_none());
    fixture.click(window, "back-tags", cx);
    assert!(label(window, "tag-table", cx).is_some());
    fixture.expect("only the edited song is written", |command, _| {
        let Command::WriteTags(edits) = command else {
            panic!("expected a tag write");
        };
        assert_eq!(
            edits,
            vec![TagEdit {
                tracks: vec!["track-2".into()],
                fields: BTreeMap::from([(TagField::Title, EditValue::Set("Renamed".into()))]),
            }]
        );
        vec![]
    });
    fixture.click(window, "save-tags", cx);
    fixture.click(window, "confirm-tags", cx);
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn a_plain_click_ends_the_multi_selection(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    writes_tags(&mut fixture, true);
    fixture.settle(window, cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.click(window, "main-view-menu", cx);
    fixture.menu_item(window, "Tracks", cx);
    fixture.click(window, "row-flat-track-1", cx);
    cx.update_window(window, |_, window, cx| {
        window.click_with_modifiers("row-flat-track-2", Modifiers::command(), cx)
    })
    .unwrap();
    fixture.click(window, "row-flat-track-2", cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-flat-track-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Edit Tags…", cx);
    assert_eq!(
        label(window, "tag-editor", cx).as_deref(),
        Some("Edit tags of Second song")
    );
}

#[gpui_kit::test]
fn clearing_a_field_writes_it_empty_on_every_song(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    fixture.click(window, "clear-tag-genre", cx);
    fixture.click(window, "save-tags", cx);
    assert_eq!(
        label(window, ("tag-change", 0_usize), cx).as_deref(),
        Some("First · Genre: Rock → (empty)")
    );
    fixture.expect("clear genre", |command, _| {
        let Command::WriteTags(edits) = command else {
            panic!("expected a tag write");
        };
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].tracks.len(), 2);
        assert_eq!(
            edits[0].fields.get(&TagField::Genre),
            Some(&EditValue::Clear)
        );
        vec![]
    });
    fixture.click(window, "confirm-tags", cx);
    fixture.assert_script_finished();
}

fn candidate() -> ReleaseCandidate {
    ReleaseCandidate {
        id: "rel".into(),
        title: "Fixture album".into(),
        artist: "Fixture Artist".into(),
        date: "1997-05-21".into(),
        country: "GB".into(),
        format: "CD".into(),
        tracks: 2,
        score: 100,
        ..Default::default()
    }
}

fn release() -> Release {
    let track = |title: &str, number| LookupTrack {
        id: format!("t{number}"),
        recording_id: format!("r{number}"),
        title: title.into(),
        artists: vec![("Fixture Artist".into(), "a1".into())],
        number,
        disc: 1,
        length_ms: None,
    };
    Release {
        id: "rel".into(),
        title: "Fixture album".into(),
        artists: vec![("Fixture Artist".into(), "a1".into())],
        group_id: "rg".into(),
        date: "1997-05-21".into(),
        genres: vec!["Trip Hop".into()],
        tracks: vec![track("Second", 2), track("First Song", 1)],
    }
}

#[gpui_kit::test]
fn find_on_musicbrainz_fills_matched_songs_for_review(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    fixture.expect("search with the album's tags", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a release search");
        };
        assert_eq!(
            query,
            ReleaseQuery {
                album: "Fixture album".into(),
                artist: "Fixture Artist".into(),
                tracks: 2,
                ..Default::default()
            }
        );
        vec![
            Event::ReleaseCandidates(
                ReleaseQuery {
                    album: "An older search".into(),
                    ..query.clone()
                },
                Ok(vec![]),
            ),
            Event::ReleaseCandidates(query, Ok(vec![candidate()])),
        ]
    });
    fixture.click(window, "find-musicbrainz", cx);
    assert_eq!(
        label(window, ("release-candidate", 0_usize), cx).as_deref(),
        Some("Fixture album — Fixture Artist · 1997-05-21 · GB · CD · 2 tracks · 100% match")
    );
    fixture.expect("load the chosen edition", |command, _| {
        assert!(matches!(&command, Command::LoadRelease(id) if id == "rel"));
        vec![Event::ReleaseLoaded("rel".into(), Ok(Arc::new(release())))]
    });
    fixture.click(window, ("release-candidate", 0_usize), cx);
    assert_eq!(
        label(window, ("track-match", 0_usize), cx).as_deref(),
        Some("First → First Song · 65%")
    );
    assert_eq!(
        label(window, ("track-match", 1_usize), cx).as_deref(),
        Some("Second → Second · 100%")
    );
    fixture.click(window, "fill-genre", cx);
    fixture.click(window, "fill-tags", cx);
    fixture.click(window, "save-tags", cx);
    assert_eq!(
        label(window, ("tag-change", 0_usize), cx).as_deref(),
        Some("First · Title: First → First Song")
    );
    fixture.expect("write the filled values and IDs", |command, _| {
        let Command::WriteTags(edits) = command else {
            panic!("expected a tag write");
        };
        assert_eq!(edits.len(), 2);
        let first = &edits[0];
        assert_eq!(first.tracks, vec!["one".to_string()]);
        let set = |value: &str| Some(EditValue::Set(value.into()));
        assert_eq!(
            first.fields.get(&TagField::Title).cloned(),
            set("First Song")
        );
        assert_eq!(first.fields.get(&TagField::Track).cloned(), set("1"));
        assert_eq!(first.fields.get(&TagField::Year).cloned(), set("1997"));
        assert_eq!(
            first.fields.get(&TagField::MusicBrainzTrack).cloned(),
            set("r1")
        );
        assert_eq!(
            first.fields.get(&TagField::MusicBrainzRelease).cloned(),
            set("rel")
        );
        assert!(!first.fields.contains_key(&TagField::Genre));
        assert!(!first.fields.contains_key(&TagField::CoverArt));
        assert!(!first.fields.contains_key(&TagField::Album));
        assert_eq!(
            edits[1].fields.get(&TagField::MusicBrainzTrack).cloned(),
            set("r2")
        );
        vec![]
    });
    fixture.click(window, "confirm-tags", cx);
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn a_failed_search_says_why_and_keeps_the_form(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    fixture.expect("search fails", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a release search");
        };
        vec![Event::ReleaseCandidates(
            query,
            Err("MusicBrainz returned HTTP 503".into()),
        )]
    });
    fixture.click(window, "find-musicbrainz", cx);
    assert_eq!(
        label(window, "lookup-error", cx).as_deref(),
        Some("MusicBrainz returned HTTP 503")
    );
    assert!(label(window, "tag-table", cx).is_some());
}

fn present(window: AnyWindowHandle, id: &'static str, cx: &mut TestAppContext) -> bool {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn songs_view(fixture: &mut Fixture, window: AnyWindowHandle, cx: &mut TestAppContext) {
    writes_tags(fixture, true);
    fixture.settle(window, cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.click(window, "main-view-menu", cx);
    fixture.menu_item(window, "Tracks", cx);
}

#[gpui_kit::test]
fn one_song_searches_musicbrainz_by_its_recording(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    songs_view(&mut fixture, window, cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-flat-track-1", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Edit Tags…", cx);
    fixture.expect("search for the song's recording", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        assert!(query.is_song());
        assert_eq!(query.length_ms, Some(120_000));
        assert_eq!(query.tracks, 0);
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    assert_eq!(
        label(window, ("release-candidate", 0_usize), cx).as_deref(),
        Some("Fixture album — Fixture Artist · 1997-05-21 · GB · CD · 2 tracks · 100% match")
    );
    fixture.assert_script_finished();
}

fn from_album(id: &str, title: &str, album_id: &str, album: &str) -> Arc<Track> {
    let mut track = (*song(id, title)).clone();
    track.album_id = album_id.into();
    track.album = album.into();
    Arc::new(track)
}

#[gpui_kit::test]
fn songs_from_several_albums_are_looked_up_one_album_at_a_time(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    let other = from_album("three", "Third", "album-2", "Other album");
    open_editor_on(
        &mut fixture,
        window,
        vec![song("one", "First"), song("two", "Second"), other.clone()],
        cx,
    );
    assert_eq!(
        label(window, "find-musicbrainz", cx).as_deref(),
        Some("Find Each Album")
    );
    assert!(!present(window, "musicbrainz-link", cx));
    fixture.expect("search the first album alone", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a release search");
        };
        assert_eq!(query.album, "Fixture album");
        assert_eq!(query.tracks, 2);
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    assert_eq!(
        label(window, "lookup-album", cx).as_deref(),
        Some("Album 1 of 2: Fixture album")
    );
    fixture.expect("load the first album's edition", |_, _| {
        vec![Event::ReleaseLoaded("rel".into(), Ok(Arc::new(release())))]
    });
    fixture.click(window, ("release-candidate", 0_usize), cx);
    assert!(label(window, ("track-match", 1_usize), cx).is_some());
    assert!(label(window, ("track-match", 2_usize), cx).is_none());
    fixture.expect("then search the next album", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search for the next album");
        };
        assert_eq!(query.album, "Other album");
        vec![]
    });
    fixture.click(window, "fill-tags", cx);
    assert_eq!(
        label(window, "lookup-album", cx).as_deref(),
        Some("Album 2 of 2: Other album")
    );
    fixture.click(window, "skip-album", cx);
    assert!(!present(window, "lookup-album", cx));
    fixture.backend.emit(Event::ReleaseCandidates(
        ReleaseQuery::for_tracks(&[(*other).clone()]),
        Ok(vec![candidate()]),
    ));
    fixture.settle(window, cx);
    assert!(!present(window, "release-candidate", cx));
    fixture.click(window, "save-tags", cx);
    let changes = changes(window, cx);
    assert!(
        changes
            .iter()
            .any(|change| change == "First · Title: First → First Song"),
        "{changes:?}"
    );
    assert!(
        !changes.iter().any(|change| change.starts_with("Third")),
        "{changes:?}"
    );
    fixture.assert_script_finished();
}

fn paste_link(fixture: &mut Fixture, window: AnyWindowHandle, link: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.click("musicbrainz-link", cx);
        window.input(link, cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "open-musicbrainz-link", cx);
}

#[gpui_kit::test]
fn a_pasted_release_link_loads_that_release_without_searching(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    paste_link(&mut fixture, window, "not a link", cx);
    assert_eq!(
        label(window, "lookup-error", cx).as_deref(),
        Some("Paste a MusicBrainz release or recording link or ID")
    );
    let id = "b84ee12a-09ef-421b-82de-0441a926375b";
    fixture.expect("load the pasted release", move |command, _| {
        assert!(matches!(&command, Command::LoadRelease(asked) if asked == id));
        vec![Event::ReleaseLoaded(id.into(), Ok(Arc::new(release())))]
    });
    cx.update_window(window, |_, window, cx| {
        window.click("musicbrainz-link", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
    })
    .unwrap();
    paste_link(
        &mut fixture,
        window,
        &format!("https://musicbrainz.org/release/{id}"),
        cx,
    );
    assert!(present(window, "fill-tags", cx));
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn a_pasted_recording_link_offers_the_releases_it_is_on(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    let id = "4a2a2b31-0b8a-4a4d-b3d6-6a3b5e1f0d3c";
    fixture.expect("search by recording ID", move |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        assert_eq!(query.lucene(), format!("rid:{id}"));
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    paste_link(
        &mut fixture,
        window,
        &format!("https://musicbrainz.org/recording/{id}?tab=details"),
        cx,
    );
    assert!(label(window, ("release-candidate", 0_usize), cx).is_some());
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn ticking_cover_art_asks_for_the_release_front_cover(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    fixture.expect("search", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    fixture.expect("load", |_, _| {
        vec![Event::ReleaseLoaded("rel".into(), Ok(Arc::new(release())))]
    });
    fixture.click(window, ("release-candidate", 0_usize), cx);
    fixture.click(window, "fill-cover-art", cx);
    fixture.click(window, "fill-tags", cx);
    fixture.click(window, "save-tags", cx);
    let changes: Vec<String> = (0..40_usize)
        .filter_map(|row| label(window, ("tag-change", row), cx))
        .filter(|change| change.contains("Cover art"))
        .collect();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(changes[0].ends_with("→ Front cover from Cover Art Archive"));
    fixture.expect("write the cover with the tags", |command, _| {
        let Command::WriteTags(edits) = command else {
            panic!("expected a tag write");
        };
        assert!(edits.iter().all(|edit| edit.fields.get(&TagField::CoverArt)
            == Some(&EditValue::Set("coverart:rel".into()))));
        vec![]
    });
    fixture.click(window, "confirm-tags", cx);
    fixture.assert_script_finished();
}

fn open_editor_on(
    fixture: &mut Fixture,
    window: AnyWindowHandle,
    songs: Vec<Arc<Track>>,
    cx: &mut TestAppContext,
) {
    writes_tags(fixture, true);
    fixture.settle(window, cx);
    fixture.expect("album songs for the tag editor", move |_, _| {
        vec![Event::Album("album-1".into(), songs)]
    });
    album_menu(fixture, window, cx);
    fixture.menu_item(window, "Edit Album Tags…", cx);
}

fn changes(window: AnyWindowHandle, cx: &mut TestAppContext) -> Vec<String> {
    (0..40_usize)
        .filter_map(|row| label(window, ("tag-change", row), cx))
        .collect()
}

#[gpui_kit::test]
fn file_names_fill_the_table_through_a_pattern(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    let named = |id: &str, title: &str, path: &str| {
        let mut track = (*song(id, title)).clone();
        track.path = path.into();
        Arc::new(track)
    };
    open_editor_on(
        &mut fixture,
        window,
        vec![
            named(
                "one",
                "First",
                "Fixture Artist/Fixture album/01 - First Song.flac",
            ),
            named("two", "Second", "Fixture Artist/Fixture album/notes.flac"),
        ],
        cx,
    );
    cx.update_window(window, |_, window, cx| {
        window.click("filename-pattern", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input("%track% - %title% (Live)", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "fill-from-names", cx);
    assert_eq!(
        label(window, "lookup-error", cx).as_deref(),
        Some("No file name fits %track% - %title% (Live)")
    );
    cx.update_window(window, |_, window, cx| {
        window.click("filename-pattern", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input("%track% - %title%", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "fill-from-names", cx);
    fixture.click(window, "save-tags", cx);
    assert_eq!(
        changes(window, cx),
        [
            "First · Title: First → First Song",
            "First · Track: (empty) → 1"
        ]
    );
}

#[gpui_kit::test]
fn the_genre_limit_picks_how_many_genres_are_filled(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    fixture.expect("search", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    fixture.expect("load", |_, _| {
        let mut release = release();
        release.genres = ["Trip Hop", "Electronic", "Downtempo", "Ambient"]
            .map(String::from)
            .to_vec();
        vec![Event::ReleaseLoaded("rel".into(), Ok(Arc::new(release)))]
    });
    fixture.click(window, ("release-candidate", 0_usize), cx);
    fixture.click(window, "genre-limit-1", cx);
    fixture.click(window, "fill-tags", cx);
    fixture.click(window, "save-tags", cx);
    let genres: Vec<String> = changes(window, cx)
        .into_iter()
        .filter(|change| change.contains("Genre"))
        .collect();
    assert_eq!(
        genres,
        [
            "First · Genre: Rock → Trip Hop",
            "Second · Genre: Rock → Trip Hop"
        ]
    );
}

#[gpui_kit::test]
fn identify_by_sound_needs_a_key_then_searches_the_recording(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    songs_view(&mut fixture, window, cx);
    let open = |fixture: &mut Fixture, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.right_click("row-flat-track-1", cx)
        })
        .unwrap();
        fixture.settle(window, cx);
        fixture.menu_item(window, "Edit Tags…", cx);
    };
    open(&mut fixture, cx);
    assert!(!present(window, "identify-sound", cx));
    // The editor is modal: close it before opening it again with a key set.
    fixture.click(window, "cancel-tags", cx);
    let mut prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    prefs.metadata = MetadataPreferences {
        write_tags: true,
        library_folder: "/Volumes/music".into(),
        acoustid_key: "key".into(),
    };
    fixture.backend.emit(Event::Ready(
        Some(Account {
            url: "https://fixture.invalid".into(),
            username: "fixture".into(),
        }),
        prefs,
    ));
    fixture.settle(window, cx);
    open(&mut fixture, cx);
    fixture.expect("fingerprint the song", |command, _| {
        let Command::IdentifyBySound(track) = command else {
            panic!("expected an AcoustID lookup");
        };
        assert_eq!(track.id, "track-1");
        vec![
            Event::SoundIdentified("other".into(), Ok("stale".into())),
            Event::SoundIdentified(track.id, Ok("rec-1".into())),
        ]
    });
    fixture.expect("search the releases of the recording", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        assert_eq!(query.lucene(), "rid:rec-1");
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "identify-sound", cx);
    assert!(label(window, ("release-candidate", 0_usize), cx).is_some());
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn a_song_with_no_match_names_the_recording(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    writes_tags(&mut fixture, true);
    songs_view(&mut fixture, window, cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-flat-track-1", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Edit Tags…", cx);
    fixture.expect("search", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        vec![Event::ReleaseCandidates(query, Ok(Vec::new()))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    let error = label(window, "lookup-error", cx).unwrap();
    assert!(
        error.starts_with("MusicBrainz found no recording called "),
        "{error}"
    );
}

#[gpui_kit::test]
fn one_song_can_take_the_release_cover(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    writes_tags(&mut fixture, true);
    songs_view(&mut fixture, window, cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-flat-track-1", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.menu_item(window, "Edit Tags…", cx);
    fixture.expect("search", |command, _| {
        let Command::FindReleases(query) = command else {
            panic!("expected a search");
        };
        vec![Event::ReleaseCandidates(query, Ok(vec![candidate()]))]
    });
    fixture.click(window, "find-musicbrainz", cx);
    fixture.expect("load", |_, _| {
        let mut release = release();
        release.tracks[1].title = "Fixture song".into();
        vec![Event::ReleaseLoaded("rel".into(), Ok(Arc::new(release)))]
    });
    fixture.click(window, ("release-candidate", 0_usize), cx);
    fixture.click(window, "fill-cover-art", cx);
    fixture.click(window, "fill-tags", cx);
    fixture.click(window, "save-tags", cx);
    assert!(present(window, "confirm-tags", cx));
    let changes = changes(window, cx);
    assert!(
        changes.iter().any(|change| change
            .ends_with("Cover art: (empty) → Front cover from Cover Art Archive")
            || change.ends_with("→ Front cover from Cover Art Archive")),
        "{changes:?}"
    );
}

fn bounds(
    window: AnyWindowHandle,
    id: &'static str,
    cx: &mut TestAppContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.find(id).bounds()
    })
    .unwrap()
}

fn progress(done: usize, current: &str) -> Event {
    Event::TagProgress(TagProgress {
        done,
        total: 2,
        current: current.into(),
    })
}

#[gpui_kit::test]
fn the_editor_floats_over_the_library_and_shows_each_file_as_it_is_written(
    cx: &mut TestAppContext,
) {
    let (_dir, mut fixture, window, _app) = start(cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    let library = bounds(window, "library-content", cx);
    open_album_editor(&mut fixture, window, cx);
    assert_eq!(
        label(window, "tag-editor", cx).as_deref(),
        Some("Edit tags of 2 songs")
    );
    // A dialog over the window: the library keeps its whole area.
    assert_eq!(bounds(window, "library-content", cx), library);
    assert!(present(window, "tag-editor-scroll", cx));

    fixture.expect("write genre", |command, _| {
        assert!(matches!(command, Command::WriteTags(_)));
        vec![progress(0, "First")]
    });
    cx.update_window(window, |_, window, cx| {
        window.click("tag-genre", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input("Jazz", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.click(window, "save-tags", cx);
    fixture.click(window, "confirm-tags", cx);
    assert_eq!(
        label(window, "tag-progress", cx).as_deref(),
        Some("Writing 1 of 2 · First")
    );
    fixture.backend.emit(progress(1, "Second"));
    fixture.settle(window, cx);
    assert_eq!(
        label(window, "tag-progress", cx).as_deref(),
        Some("Writing 2 of 2 · Second")
    );
    // Escape does not close the editor while files are being written.
    fixture.press(window, "escape", cx);
    assert!(present(window, "tag-editor", cx));

    fixture.backend.emit(Event::TagsWritten(TagReport {
        job: 1,
        written: 2,
        ..Default::default()
    }));
    fixture.settle(window, cx);
    assert!(!present(window, "tag-editor", cx));
    assert_eq!(
        label(window, "tag-notice", cx).as_deref(),
        Some("Wrote tags to 2 files")
    );
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn escape_closes_the_editor_without_writing(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_album_editor(&mut fixture, window, cx);
    assert!(present(window, "tag-editor", cx));
    fixture.press(window, "escape", cx);
    assert!(!present(window, "tag-editor", cx));
    fixture.assert_script_finished();
}
