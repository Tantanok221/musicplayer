//! Playlist interactions with scenario-local expected commands and replies.
use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, ScrollDelta, TestAppContext, point, px, size, test::TestWindowExt,
};
use sonora::{
    model::{PlaybackSnapshot, Playlist},
    platform::shortcut,
    playback::{PlayerCommand, test_support::PausedPlayer},
    service::{Command, Event, Mutation, PlaylistTarget},
};
use std::{sync::Arc, time::Duration};

#[gpui_kit::test]
fn metadata_columns_can_be_changed_from_the_header_and_settings(cx: &mut TestAppContext) {
    let (dir, mut f, window, app) = start(cx);
    open_playlist(&mut f, window, cx);
    let mut loaded = playlist("Roadtrip");
    loaded.entry = f
        .catalog
        .by_ids(&["track-1".into(), "track-1".into()])
        .unwrap()
        .into();
    loaded.song_count = 2;
    for entry in Arc::make_mut(&mut loaded.entry) {
        Arc::make_mut(entry).genre = "Jazz".into();
    }
    f.backend.emit(Event::Playlist(loaded));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(
            window.try_find("playlist-track-header").is_some(),
            "playlist needs a metadata column header"
        );
        assert!(window.find("playlist-column-artist").visible());
        assert!(window.find("playlist-cell-playlist-0-artist").visible());
        assert!(window.try_find("playlist-column-genre").is_none());
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.right_click("playlist-track-header", cx)
    })
    .unwrap();
    f.settle(window, cx);
    f.menu_item(window, "Genre", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-column-genre").visible());
        assert!(window.find("playlist-cell-playlist-1-genre").visible());
        assert_eq!(
            window.find("playlist-cell-playlist-1-genre").label(),
            Some("Jazz")
        );
        assert_eq!(
            window.find("playlist-column-genre").bounds().left(),
            window
                .find("playlist-cell-playlist-1-genre")
                .bounds()
                .left()
        );
    })
    .unwrap();
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    cx.update_window(window, |_, window, cx| {
        window.within("settings-sidebar").click("0-6", cx)
    })
    .unwrap();
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playlist-show-genre").checked(), Some(true));
    })
    .unwrap();
    f.click(window, "playlist-show-artist", cx);
    f.press(window, shortcut("cmd-,", "ctrl-,"), cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-column-artist").is_none());
        assert!(window.try_find("playlist-cell-playlist-0-artist").is_none());
        assert!(window.find("playlist-column-genre").visible());
    })
    .unwrap();
    // Metadata controls cannot change the original occurrence used by playback.
    f.backend.player_commands().try_iter().for_each(drop);
    cx.update_window(window, |_, window, cx| {
        window.double_click("row-playlist-1", cx)
    })
    .unwrap();
    f.settle(window, cx);
    let PlayerCommand::Replace(tracks, index) = f.backend.player_commands().try_recv().unwrap()
    else {
        panic!("expected playlist playback")
    };
    assert_eq!(index, 1);
    assert_eq!(tracks.len(), 2);
    f.assert_script_finished();
    let saved: sonora::model::Preferences = f.catalog.state("preferences").unwrap();
    assert!(
        !saved
            .playlist_columns
            .contains(sonora::playlists::PlaylistColumn::Artist)
    );
    assert!(
        saved
            .playlist_columns
            .contains(sonora::playlists::PlaylistColumn::Genre)
    );
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    drop(app);
    drop(f);
    cx.run_until_parked();
    let mut restarted = Fixture::open(&dir.path().join("catalog.sqlite"));
    restarted.expect("restore open playlist", |command, _| {
        assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
        vec![Event::Playlist(playlist("Roadtrip"))]
    });
    let (window, _app) = restarted.mount(cx);
    restarted.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-column-artist").is_none());
        assert!(window.find("playlist-column-genre").visible());
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.right_click("playlist-track-header", cx)
    })
    .unwrap();
    restarted.settle(window, cx);
    restarted.menu_item(window, "Restore default columns", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-column-artist").visible());
        assert!(window.try_find("playlist-column-genre").is_none());
    })
    .unwrap();
    restarted.assert_script_finished();
}

#[gpui_kit::test]
fn metadata_columns_scroll_together_and_support_readonly_and_empty_playlists(
    cx: &mut TestAppContext,
) {
    let (_dir, mut f, window, _app) = start(cx);
    open_playlist(&mut f, window, cx);
    let mut loaded = playlist("Automatic");
    loaded.readonly = true;
    loaded.entry = f.catalog.by_ids(&["track-1".into()]).unwrap().into();
    f.backend.emit(Event::Playlist(loaded));
    f.settle(window, cx);
    for label in ["Genre", "Year", "Format", "Play count"] {
        cx.update_window(window, |_, window, cx| {
            window.right_click("playlist-track-header", cx)
        })
        .unwrap();
        f.settle(window, cx);
        f.menu_item(window, label, cx);
    }
    cx.simulate_window_resize(window, size(px(900.), px(650.)));
    f.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.scroll(
            "playlist-track-table",
            ScrollDelta::Pixels(point(px(-1500.), px(0.))),
            cx,
        );
    })
    .unwrap();
    f.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let header = window.find("playlist-column-play-count");
        let cell = window.find("playlist-cell-playlist-0-play-count");
        assert!(header.visible());
        assert!(cell.visible());
        assert_eq!(cell.label(), Some("—"));
        assert_eq!(header.bounds().left(), cell.bounds().left());
        window.right_click("playlist-cell-playlist-0-play-count", cx);
    })
    .unwrap();
    f.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert!(menu.try_find_by_label("Remove from Playlist").is_none());
        menu.press("escape", cx);
    })
    .unwrap();
    f.settle(window, cx);
    f.search(window, "no match", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-track-header").visible());
        assert!(window.try_find("row-playlist-0").is_none());
    })
    .unwrap();
    f.search(window, "", cx);
    cx.simulate_window_resize(window, size(px(1440.), px(900.)));
    f.settle(window, cx);
    f.click(window, "playlist-view-albums", cx);
    f.click(window, "playlist-album-0", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-0").visible());
        assert!(window.try_find("playlist-track-header").is_none());
        assert!(window.try_find("playlist-cell-playlist-0-genre").is_none());
    })
    .unwrap();
    f.click(window, "playlist-view-tracks", cx);
    f.backend.emit(Event::Playlist(playlist("Empty")));
    f.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-track-header").visible());
        assert!(window.try_find("row-playlist-0").is_none());
    })
    .unwrap();
    f.assert_script_finished();
}

#[gpui_kit::test]
fn album_view_groups_playlist_entries_and_preserves_playback_and_readonly(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut loaded = playlist("Roadtrip");
    loaded.entry = fixture
        .catalog
        .by_ids(&[
            "track-2".into(),
            "track-1".into(),
            "track-2".into(),
            "track-3".into(),
        ])
        .unwrap()
        .into();
    loaded.song_count = 4;
    fixture.backend.emit(Event::Playlist(loaded.clone()));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-0").visible());
        assert!(
            window.try_find("playlist-view-albums").is_some(),
            "playlist must offer an album view"
        );
    })
    .unwrap();
    fixture.click(window, "playlist-view-albums", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-album-0").visible());
        assert!(window.find("playlist-album-1").visible());
        assert!(window.find("playlist-album-2").visible());
        assert!(
            window.try_find("playlist-album-3").is_none(),
            "duplicate entries share a card"
        );
        assert!(window.try_find("row-playlist-0").is_none());
        assert!(window.try_find("sort-menu").is_none());
    })
    .unwrap();
    fixture.click(window, "playlist-album-0", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-0").visible());
        assert!(window.find("row-playlist-2").visible());
        assert!(window.try_find("row-playlist-1").is_none());
    })
    .unwrap();
    fixture.backend.player_commands().try_iter().for_each(drop);
    cx.update_window(window, |_, window, cx| {
        window.double_click("row-playlist-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let PlayerCommand::Replace(tracks, index) =
        fixture.backend.player_commands().try_recv().unwrap()
    else {
        panic!("album rows must play their playlist occurrence")
    };
    assert_eq!(index, 2);
    assert_eq!(
        tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["track-2", "track-1", "track-2", "track-3"]
    );
    fixture.click(window, "playlist-view-tracks", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-1").visible())
    })
    .unwrap();
    loaded.readonly = true;
    fixture.backend.emit(Event::Playlist(loaded));
    fixture.settle(window, cx);
    fixture.click(window, "playlist-view-albums", cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-playlist-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert!(menu.try_find_by_label("Remove from Playlist").is_none());
        menu.press("escape", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.search(window, "no such song", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-album-0").is_none());
        assert!(window.find("playlist-empty").visible());
    })
    .unwrap();
    fixture.search(window, "", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-album-0").visible())
    })
    .unwrap();
    fixture.backend.player_commands().try_iter().for_each(drop);
    cx.update_window(window, |_, window, cx| {
        window.double_click("playlist-album-0", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let PlayerCommand::Replace(tracks, index) =
        fixture.backend.player_commands().try_recv().unwrap()
    else {
        panic!("album double click must play only its playlist entries")
    };
    assert_eq!(index, 0);
    assert_eq!(
        tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["track-2", "track-2"]
    );
    fixture.backend.emit(Event::Playlist(playlist("Roadtrip")));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-album-0").is_none());
        assert!(window.try_find("row-playlist-2").is_none());
        assert!(window.find("playlist-empty").visible());
    })
    .unwrap();
    fixture.click(window, "playlist-view-tracks", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("row-playlist-0").is_none())
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose(
        "playlist album view toggles and preserves occurrences",
        window,
        &app,
        cx,
    );
}

#[gpui_kit::test]
fn smart_playlist_owner_can_rename_but_not_edit_tracks(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    let mut smart = playlist("Automatic");
    smart.readonly = true;
    fixture.backend.emit(Event::Playlists(vec![smart.clone()]));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("smart-playlist-playlist-1").visible());
    })
    .unwrap();
    fixture.expect("open smart playlist", move |command, catalog| {
        assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
        smart.entry = catalog
            .by_ids(&["track-1".into(), "track-2".into()])
            .unwrap()
            .into();
        smart.song_count = 2;
        vec![Event::Playlist(smart)]
    });
    fixture.click(window, "nav-playlist-playlist-1", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("smart-playlist-header").label(),
            Some("Smart playlist · updates automatically")
        );
        assert!(window.find("playlist-options").visible());
    })
    .unwrap();
    fixture.click(window, "playlist-options", cx);
    cx.update_window(window, |_, window, _| {
        let menu = window.within("popup-menu");
        assert!(menu.find_by_label("Rename…").visible());
        assert!(menu.find_by_label("Delete playlist…").visible());
        assert!(menu.try_find_by_label("Append Queue").is_none());
    })
    .unwrap();
    fixture.menu_item(window, "Rename…", cx);
    edit_name(&mut fixture, window, "Automatic renamed", cx);
    fixture.expect("rename owned smart playlist", |command, _| {
        let expected = Mutation::RenamePlaylist("playlist-1".into(), "Automatic renamed".into());
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationDone(expected)]
    });
    fixture.click(window, "save-playlist-editor", cx);
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn readonly_playlists_have_no_edit_actions_or_add_target(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    for owner in ["fixture", "another-owner"] {
        let mut readonly = playlist("Automatic");
        readonly.readonly = true;
        readonly.owner = owner.into();
        fixture
            .backend
            .emit(Event::Playlists(vec![readonly.clone()]));
        fixture.settle(window, cx);
        fixture.expect("open readonly playlist", move |command, catalog| {
            assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
            readonly.entry = catalog
                .by_ids(&["track-1".into(), "track-2".into()])
                .unwrap()
                .into();
            readonly.song_count = 2;
            vec![Event::Playlist(readonly)]
        });
        fixture.click(window, "nav-playlist-playlist-1", cx);
        cx.update_window(window, |_, window, cx| {
            assert_eq!(
                window.try_find("playlist-options").is_some(),
                owner == "fixture"
            );
            window.right_click("row-playlist-0", cx);
        })
        .unwrap();
        fixture.settle(window, cx);
        cx.update_window(window, |_, window, cx| {
            let mut menu = window.within("popup-menu");
            assert!(menu.try_find_by_label("Remove from Playlist").is_none());
            let item = menu.find_by_label("Add to Playlist");
            menu.hover(item.path().last().unwrap().clone(), cx);
            menu.press("right", cx);
        })
        .unwrap();
        fixture.settle(window, cx);
        cx.update_window(window, |_, window, cx| {
            let mut submenu = window.within("submenu");
            assert!(submenu.try_find_by_label("Automatic").is_none());
            assert!(submenu.find_by_label("New Playlist…").visible());
            submenu.press("escape", cx);
            window.press("escape", cx);
        })
        .unwrap();
        fixture.settle(window, cx);
    }
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn double_click_replaces_queue_with_playlist_at_clicked_occurrence(cx: &mut TestAppContext) {
    double_click_playlist(cx, "fixture");
}

#[gpui_kit::test]
fn double_click_read_only_playlist_still_replaces_queue(cx: &mut TestAppContext) {
    double_click_playlist(cx, "another-owner");
}

fn double_click_playlist(cx: &mut TestAppContext, owner: &'static str) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture
        .backend
        .emit(Event::Playlists(vec![playlist("Roadtrip")]));
    fixture.settle(window, cx);
    fixture.expect("open mixed playlist", move |command, catalog| {
        assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
        let mut list = playlist("Roadtrip");
        list.owner = owner.into();
        list.entry = Arc::new(
            catalog
                .by_ids(&[
                    "track-3".into(),
                    "track-2".into(),
                    "track-2".into(),
                    "track-1".into(),
                ])
                .unwrap(),
        );
        list.song_count = 4;
        vec![Event::Playlist(list)]
    });
    fixture.click(window, "nav-playlist-playlist-1", cx);
    fixture.backend.player_commands().try_iter().for_each(drop);
    fixture.click(window, "row-playlist-2", cx);
    assert!(
        fixture.backend.player_commands().is_empty(),
        "single click must not play"
    );
    cx.update_window(window, |_, window, cx| {
        window.double_click("row-playlist-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let command = fixture
        .backend
        .player_commands()
        .try_recv()
        .expect("playlist replacement");
    let PlayerCommand::Replace(ref tracks, index) = command else {
        panic!("must replace queue")
    };
    assert_eq!(
        tracks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["track-3", "track-2", "track-2", "track-1"]
    );
    assert_eq!(index, 2, "duplicate occurrences keep their own position");
    let mut player = PausedPlayer::start(Duration::from_secs(5)).unwrap();
    let snapshot = player
        .command(
            PlayerCommand::Restore(tracks.clone(), index, 0., None),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(snapshot.index, Some(2));
    assert_eq!(snapshot.track.as_ref().unwrap().id, "track-2");
    assert_eq!(snapshot.queue[3].id, "track-1");
    fixture.backend.emit(Event::Playback(snapshot));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-2").visible());
        assert!(window.find("play-pause").visible());
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("playlist double click replaced queue", window, &app, cx);
    player.shutdown(Duration::from_secs(5)).unwrap();
}

#[gpui_kit::test]
fn empty_playlist_has_no_song_to_double_click(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    fixture.backend.player_commands().try_iter().for_each(drop);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("row-playlist-0").is_none());
    })
    .unwrap();
    assert!(fixture.backend.player_commands().is_empty());
    fixture.assert_script_finished();
}

fn playlist(name: &str) -> Playlist {
    Playlist {
        id: "playlist-1".into(),
        name: name.into(),
        owner: "fixture".into(),
        ..Default::default()
    }
}

fn edit_name(fixture: &mut Fixture, window: AnyWindowHandle, name: &str, cx: &mut TestAppContext) {
    cx.update_window(window, |_, window, cx| {
        window.click("playlist-name", cx);
        window.press(shortcut("cmd-a", "ctrl-a"), cx);
        window.input(name, cx);
    })
    .unwrap();
    fixture.settle(window, cx);
}

fn open_playlist(fixture: &mut Fixture, window: AnyWindowHandle, cx: &mut TestAppContext) {
    fixture
        .backend
        .emit(Event::Playlists(vec![playlist("Roadtrip")]));
    fixture.settle(window, cx);
    fixture.expect("open playlist", |command, _| {
        assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
        vec![Event::Playlist(playlist("Roadtrip"))]
    });
    fixture.click(window, "nav-playlist-playlist-1", cx);
}

#[gpui_kit::test]
fn append_queue_sends_only_queued_songs_and_retries_failure(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut selected = playlist("Roadtrip");
    selected.entry = fixture.catalog.by_ids(&["track-1".into()]).unwrap().into();
    selected.song_count = 1;
    fixture.backend.emit(Event::Playlist(selected));
    let queue = fixture
        .catalog
        .by_ids(&["track-2".into(), "track-2".into(), "track-3".into()])
        .unwrap();
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        queue: queue.into(),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    for succeeds in [false, true] {
        fixture.expect(
            "append queued songs without existing entries",
            move |command, _| {
                let expected = Mutation::AddToPlaylist(
                    "playlist-1".into(),
                    vec!["track-2".into(), "track-2".into(), "track-3".into()],
                );
                assert!(
                    matches!(command, Command::Mutate(ref mutation) if *mutation == expected),
                    "Append Queue must emit AddToPlaylist with only queued song IDs"
                );
                if succeeds {
                    vec![Event::MutationDone(expected)]
                } else {
                    vec![Event::MutationFailed(expected, "Append rejected".into())]
                }
            },
        );
        if succeeds {
            fixture.click(window, "retry-error", cx);
        } else {
            fixture.click(window, "playlist-options", cx);
            fixture.menu_item(window, "Append Queue", cx);
        }
        cx.update_window(window, |_, window, _| {
            if succeeds {
                assert!(window.try_find("error-banner").is_none());
            } else {
                assert_eq!(window.find("error-banner").label(), Some("Append rejected"));
            }
            assert!(window.find("playlist-options").visible());
        })
        .unwrap();
    }
    fixture.assert_script_finished();
    fixture.diagnose("queue append retry confirmed", window, &app, cx);
}

#[gpui_kit::test]
fn append_empty_queue_does_not_send_existing_playlist_songs(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut selected = playlist("Roadtrip");
    selected.entry = fixture.catalog.by_ids(&["track-1".into()]).unwrap().into();
    selected.song_count = 1;
    fixture.backend.emit(Event::Playlist(selected));
    fixture.settle(window, cx);
    fixture.expect("append no songs", |command, _| {
        let expected = Mutation::AddToPlaylist("playlist-1".into(), vec![]);
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationDone(expected)]
    });
    fixture.click(window, "playlist-options", cx);
    fixture.menu_item(window, "Append Queue", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-options").visible());
        assert!(window.try_find("error-banner").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("empty queue append confirmed", window, &app, cx);
}

#[gpui_kit::test]
fn remove_playlist_entry_refreshes_only_after_success(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let tracks = fixture
        .catalog
        .by_ids(&["track-1".into(), "track-2".into()])
        .unwrap();
    let mut loaded = playlist("Roadtrip");
    loaded.entry = std::sync::Arc::new(vec![
        tracks[0].clone(),
        tracks[1].clone(),
        tracks[0].clone(),
    ]);
    loaded.song_count = 3;
    fixture.backend.emit(Event::Playlist(loaded.clone()));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("row-playlist-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.expect("remove rejected", |command, _| {
        let expected = Mutation::RemoveFromPlaylist("playlist-1".into(), 2);
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationFailed(
            expected,
            "Fixture removal rejected".into(),
        )]
    });
    fixture.menu_item(window, "Remove from Playlist", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("row-playlist-2").visible());
        assert_eq!(
            window.find("error-banner").label(),
            Some("Fixture removal rejected")
        );
    })
    .unwrap();
    for index in (0..3).rev() {
        let mut updated = loaded.clone();
        std::sync::Arc::make_mut(&mut updated.entry).remove(index);
        updated.song_count -= 1;
        let response = updated.clone();
        fixture.expect("remove selected entry", move |command, _| {
            let expected = Mutation::RemoveFromPlaylist("playlist-1".into(), index);
            assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
            vec![
                Event::Playlists(vec![response.clone()]),
                Event::Playlist(response),
                Event::MutationDone(expected),
            ]
        });
        if index == 2 {
            fixture.click(window, "retry-error", cx);
        } else {
            cx.update_window(window, |_, window, cx| {
                window.right_click(format!("row-playlist-{index}"), cx)
            })
            .unwrap();
            fixture.settle(window, cx);
            fixture.menu_item(window, "Remove from Playlist", cx);
        }
        cx.update_window(window, |_, window, _| {
            assert!(window.try_find(format!("row-playlist-{index}")).is_none());
            if index > 0 {
                assert!(window.find("row-playlist-0").visible());
            }
        })
        .unwrap();
        loaded = updated;
    }
    fixture.assert_script_finished();
    fixture.diagnose("playlist entry removed", window, &app, cx);
}

#[gpui_kit::test]
fn drag_playlist_occurrence_saves_order_and_keeps_it_on_failure(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut loaded = playlist("Roadtrip");
    let original = ["track-1", "track-2", "track-1", "track-3"];
    loaded.entry = fixture
        .catalog
        .by_ids(&original.map(String::from))
        .unwrap()
        .into();
    loaded.song_count = original.len() as u32;
    let drag = |from, to, cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.drag_to(from, to, cx))
            .unwrap();
    };
    let mut read_only = loaded.clone();
    read_only.readonly = true;
    fixture.backend.emit(Event::Playlist(read_only));
    fixture.settle(window, cx);
    drag("row-playlist-2", "row-playlist-3", cx);
    fixture.settle(window, cx);

    fixture.backend.emit(Event::Playlist(loaded.clone()));
    fixture.settle(window, cx);

    // A drop on itself does not change the order or send a save.
    drag("row-playlist-2", "row-playlist-2", cx);
    fixture.settle(window, cx);

    let reordered = ["track-1", "track-2", "track-3", "track-1"];
    std::sync::Arc::make_mut(&mut loaded.entry).swap(2, 3);
    for succeeds in [false, true] {
        let updated = loaded.clone();
        fixture.expect("save the dragged playlist occurrence", move |command, _| {
            let expected = Mutation::ReorderPlaylist {
                id: "playlist-1".into(),
                original: original.map(String::from).into(),
                reordered: reordered.map(String::from).into(),
            };
            assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
            if succeeds {
                vec![Event::MutationDone(expected), Event::Playlist(updated)]
            } else {
                vec![Event::MutationFailed(expected, "Reorder rejected".into())]
            }
        });
        drag("row-playlist-2", "row-playlist-3", cx);
        fixture.settle(window, cx);
        fixture.assert_script_finished();
        cx.update_window(window, |_, window, _| {
            let last = window.find("row-playlist-3");
            assert!(last.visible());
            assert!(last.label().unwrap().starts_with(if succeeds {
                "Fixture song"
            } else {
                "Another song"
            }));
            if succeeds {
                assert!(window.try_find("error-banner").is_none());
            } else {
                assert_eq!(
                    window.find("error-banner").label(),
                    Some("Reorder rejected")
                );
            }
        })
        .unwrap();
    }
    fixture.diagnose("playlist reorder confirmed", window, &app, cx);
}

#[gpui_kit::test]
fn create_failure_keeps_the_name_and_retry_creates_one_playlist(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.click(window, "new-playlist", cx);
    // Empty submission stays in the editor and emits no service command.
    fixture.click(window, "save-playlist-editor", cx);
    edit_name(&mut fixture, window, "Roadtrip 日本", cx);
    fixture.expect("create rejected", |command, _| {
        let expected = Mutation::CreatePlaylist("Roadtrip 日本".into(), vec![]);
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationFailed(
            expected,
            "Fixture playlist save rejected".into(),
        )]
    });
    fixture.click(window, "save-playlist-editor", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playlist-name").value(), Some("Roadtrip 日本"));
        assert!(window.find("save-playlist-editor").visible());
        assert_eq!(
            window.find("error-banner").label(),
            Some("Fixture playlist save rejected")
        );
    })
    .unwrap();
    fixture.expect("retry same playlist", |command, _| {
        let expected = Mutation::CreatePlaylist("Roadtrip 日本".into(), vec![]);
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![
            Event::Playlists(vec![playlist("Roadtrip 日本")]),
            Event::MutationDone(expected),
        ]
    });
    fixture.click(window, "retry-error", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("nav-playlist-playlist-1").label(),
            Some("Roadtrip 日本")
        );
        assert!(window.find("nav-playlist-playlist-1").visible());
        assert!(window.try_find("playlist-name").is_none());
        assert!(window.try_find("error-banner").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("playlist retry confirmed", window, &app, cx);
}

#[gpui_kit::test]
fn rename_and_add_an_album_use_the_selected_playlist(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    fixture.click(window, "playlist-options", cx);
    fixture.menu_item(window, "Rename…", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playlist-name").value(), Some("Roadtrip"))
    })
    .unwrap();
    edit_name(&mut fixture, window, "Evening", cx);
    fixture.expect("rename selected playlist", |command, _| {
        let expected = Mutation::RenamePlaylist("playlist-1".into(), "Evening".into());
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![
            Event::Playlists(vec![playlist("Evening")]),
            Event::Playlist(playlist("Evening")),
            Event::MutationDone(expected),
        ]
    });
    fixture.click(window, "save-playlist-editor", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("nav-playlist-playlist-1").label(),
            Some("Evening")
        );
        assert!(window.try_find("playlist-name").is_none());
    })
    .unwrap();
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("explorer-album-album-2", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    // Use real submenu keyboard navigation so this traverses both menu layers.
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        let item = menu.find_by_label("Add to Playlist");
        menu.hover(item.path().last().unwrap().clone(), cx);
        menu.press("right", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.expect("append album to renamed playlist", |command, _| {
        assert!(matches!(command, Command::AlbumToPlaylist(album, PlaylistTarget::Existing(id)) if album == "album-2" && id == "playlist-1"));
        let mut updated = playlist("Evening");
        updated.song_count = 1;
        vec![Event::Playlists(vec![updated]), Event::MutationDone(Mutation::AddToPlaylist("playlist-1".into(), vec!["track-2".into()]))]
    });
    cx.update_window(window, |_, window, cx| {
        let mut submenu = window.within("submenu");
        let target = submenu.find_by_label("Evening");
        submenu.click(target.path().last().unwrap().clone(), cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    fixture.assert_script_finished();
    cx.update_window(window, |_, window, _| {
        let status = window.find("library-status");
        assert!(status.visible());
        assert!(
            status
                .label()
                .unwrap()
                .contains("Added 1 song to “Evening”")
        );
    })
    .unwrap();
    fixture.diagnose("renamed playlist receives album", window, &app, cx);
}

#[gpui_kit::test]
fn cancel_playlist_deletion_keeps_the_playlist_without_a_mutation(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    fixture.click(window, "playlist-options", cx);
    fixture.menu_item(window, "Delete playlist…", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("confirm-delete").visible())
    })
    .unwrap();
    // No deletion expectation: any mutation makes the fixture fail.
    fixture.click(window, "cancel-delete", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("confirm-delete").is_none());
        assert!(window.find("nav-playlist-playlist-1").visible());
        assert!(window.find("playlist-options").visible());
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("playlist deletion canceled", window, &app, cx);
}

#[gpui_kit::test]
fn confirmed_playlist_deletion_removes_the_sidebar_entry(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    fixture.click(window, "playlist-options", cx);
    fixture.menu_item(window, "Delete playlist…", cx);
    fixture.expect("delete selected playlist", |command, _| {
        let expected = Mutation::DeletePlaylist("playlist-1".into());
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::Playlists(vec![]), Event::MutationDone(expected)]
    });
    fixture.click(window, "confirm-delete", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("confirm-delete").is_none());
        assert!(window.try_find("nav-playlist-playlist-1").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
    fixture.diagnose("playlist deletion confirmed", window, &app, cx);
}
