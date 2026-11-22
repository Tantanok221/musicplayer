//! Playlist interactions with scenario-local expected commands and replies.
use crate::common::ui::{Fixture, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, TestAppContext,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::{
    model::{PlaybackSnapshot, Playlist},
    platform::shortcut,
    playback::{Placement, PlayerCommand, test_support::PausedPlayer},
    service::{Command, Event, Mutation, PlaylistTarget},
};
use std::{sync::Arc, time::Duration};

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

#[gpui_kit::test]
fn empty_playlist_play_preserves_queue(cx: &mut TestAppContext) {
    assert_empty_play_preserves_queue(cx, false);
}

#[gpui_kit::test]
fn loading_playlist_play_preserves_queue(cx: &mut TestAppContext) {
    assert_empty_play_preserves_queue(cx, true);
}

fn assert_empty_play_preserves_queue(cx: &mut TestAppContext, loading: bool) {
    let (_dir, mut fixture, window, _app) = start(cx);
    let tracks = fixture.catalog.by_ids(&["track-1".into()]).unwrap();
    let mut player = PausedPlayer::start(Duration::from_secs(5)).unwrap();
    let before = player
        .command(
            PlayerCommand::Restore(tracks, 0, 12., None),
            Duration::from_secs(5),
        )
        .unwrap();
    fixture.backend.emit(Event::Playback(before));
    let mut summary = playlist("Roadtrip");
    summary.song_count = if loading { 3 } else { 0 };
    fixture.backend.emit(Event::Playlists(vec![summary]));
    fixture.settle(window, cx);
    fixture.expect("open playlist", move |command, _| {
        assert!(matches!(command, Command::Playlist(id) if id == "playlist-1"));
        if loading {
            vec![]
        } else {
            vec![Event::Playlist(playlist("Roadtrip"))]
        }
    });
    fixture.click(window, "nav-playlist-playlist-1", cx);
    fixture.backend.player_commands().try_iter().for_each(drop);
    fixture.click(window, "play-playlist", cx);
    assert!(
        fixture.backend.player_commands().is_empty(),
        "empty or loading playlist Play must not send Replace"
    );
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("play-playlist").disabled(), Some(true));
        assert!(window.find("play-pause").visible());
    })
    .unwrap();
    let mut loaded = playlist("Roadtrip");
    loaded.entry = fixture.catalog.by_ids(&["track-2".into()]).unwrap().into();
    loaded.song_count = 1;
    fixture.backend.emit(Event::Playlist(loaded));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_ne!(window.find("play-playlist").disabled(), Some(true));
    })
    .unwrap();
    fixture.click(window, "play-playlist", cx);
    let PlayerCommand::Replace(tracks, index) =
        fixture.backend.player_commands().try_recv().unwrap()
    else {
        panic!("loaded playlist must replace the queue")
    };
    assert_eq!(tracks[0].id, "track-2");
    assert_eq!(index, 0);
    fixture.assert_script_finished();
    player.shutdown(Duration::from_secs(5)).unwrap();
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
    let expected = sonora::playlists::PlaylistEntry::new(&loaded, 2)
        .mutation(Some(&loaded))
        .unwrap();
    fixture.expect("remove rejected", move |command, _| {
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
        let expected = sonora::playlists::PlaylistEntry::new(&loaded, index)
            .mutation(Some(&loaded))
            .unwrap();
        fixture.expect("remove selected entry", move |command, _| {
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
    fixture.click(window, "create-playlist", cx);
    edit_name(&mut fixture, window, "Roadtrip 日本", cx);
    fixture.expect("create in progress", |command, _| {
        let expected = Mutation::CreatePlaylist("Roadtrip 日本".into(), vec![]);
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationDone(Mutation::CreatePlaylist(
            "Unrelated".into(),
            vec![],
        ))]
    });
    fixture.click(window, "create-playlist", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("create-playlist").disabled(), Some(true));
    })
    .unwrap();
    fixture.press(window, "enter", cx);
    fixture.click(window, "create-playlist", cx);
    fixture.backend.emit(Event::MutationFailed(
        Mutation::CreatePlaylist("Roadtrip 日本".into(), vec![]),
        "Fixture playlist save rejected".into(),
    ));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playlist-name").value(), Some("Roadtrip 日本"));
        assert!(window.find("create-playlist").visible());
        assert_eq!(
            window.find("playlist-create-error").label(),
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
    fixture.click(window, "create-playlist", cx);
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
fn creating_from_an_album_uses_the_shared_editor_and_keeps_the_album(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    fixture.backend.emit(Event::NativeAvailable(true));
    fixture.settle(window, cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("explorer-album-album-2", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        let item = menu.find_by_label("Add to Playlist");
        menu.hover(item.path().last().unwrap().clone(), cx);
        menu.press("right", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let mut submenu = window.within("submenu");
        let item = submenu.find_by_label("New Playlist…");
        submenu.click(item.path().last().unwrap().clone(), cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-creation").visible());
        assert!(window.try_find("playlist-type-smart").is_none());
        let name = window.find("playlist-name");
        assert!(name.value().is_some_and(|value| !value.is_empty()));
        assert_eq!(name.focused(), Some(true));
    })
    .unwrap();
    edit_name(&mut fixture, window, "Album copy", cx);
    fixture.expect("create playlist from the chosen album", |command, _| {
        assert!(
            matches!(command, Command::AlbumToPlaylist(id, PlaylistTarget::New(name))
            if id == "album-2" && name == "Album copy")
        );
        vec![Event::MutationDone(Mutation::CreatePlaylist(
            "Album copy".into(),
            vec!["track-2".into()],
        ))]
    });
    fixture.press(window, "enter", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-creation").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn saving_upcoming_queue_tracks_keeps_order_and_duplicate_occurrences(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app) = start(cx);
    fixture.backend.emit(Event::NativeAvailable(true));
    let tracks = fixture
        .catalog
        .by_ids(&["track-1", "track-2", "track-2", "track-3"].map(String::from))
        .unwrap();
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        index: Some(0),
        track: Some(tracks[0].clone()),
        queue: tracks.into(),
        queue_ids: vec![1, 2, 3, 4].into(),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    fixture.click(window, "save-queue-playlist", cx);
    fixture.menu_item(window, "Upcoming tracks…", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.find("playlist-creation").visible());
        assert!(window.try_find("playlist-type-smart").is_none());
    })
    .unwrap();
    edit_name(&mut fixture, window, "Later", cx);
    fixture.expect("create playlist from the upcoming queue", |command, _| {
        let expected = Mutation::CreatePlaylist(
            "Later".into(),
            ["track-2", "track-2", "track-3"].map(String::from).to_vec(),
        );
        assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
        vec![Event::MutationDone(expected)]
    });
    fixture.click(window, "create-playlist", cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("playlist-creation").is_none());
    })
    .unwrap();
    fixture.assert_script_finished();
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

#[gpui_kit::test]
fn playlist_context_menu_queues_an_unopened_shared_playlist(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut shared = playlist("Shared mix");
    shared.id = "shared".into();
    shared.owner = "another-user".into();
    shared.readonly = true;
    shared.song_count = 3;
    fixture
        .backend
        .emit(Event::Playlists(vec![shared, playlist("Roadtrip")]));
    let queue = fixture.catalog.by_ids(&["track-1".into()]).unwrap();
    fixture.backend.emit(Event::Playback(PlaybackSnapshot {
        track: queue.first().cloned(),
        index: Some(0),
        queue: queue.into(),
        ..Default::default()
    }));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        window.right_click("nav-playlist-shared", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, cx| {
        let mut menu = window.within("popup-menu");
        assert!(menu.find_by_label("Play Next").visible());
        menu.press("escape", cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("popup-menu").is_none());
    })
    .unwrap();
    for (label, placement) in [
        ("Play Next", Placement::Next),
        ("Play After This Album", Placement::AfterAlbum),
        ("Add to End of Queue", Placement::Last),
    ] {
        cx.update_window(window, |_, window, cx| {
            window.right_click("nav-playlist-shared", cx)
        })
        .unwrap();
        fixture.settle(window, cx);
        cx.update_window(window, |_, window, _| {
            let menu = window.within("popup-menu");
            assert!(menu.find_by_label(label).visible());
            assert!(menu.try_find_by_label("Delete playlist…").is_none());
        })
        .unwrap();
        fixture.expect("queue the playlist under the pointer", move |command, _| {
            assert!(matches!(command, Command::EnqueuePlaylist(id, place) if id == "shared" && place == placement));
            vec![]
        });
        fixture.menu_item(window, label, cx);
        cx.update_window(window, |_, window, _| {
            assert!(window.find("playlist-options").visible());
            assert!(window.try_find("popup-menu").is_none());
        })
        .unwrap();
    }
    fixture.assert_script_finished();
    fixture.diagnose("queued unopened shared playlist", window, &app, cx);
}

#[gpui_kit::test]
fn playlist_context_menu_deletes_only_the_confirmed_row(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    open_playlist(&mut fixture, window, cx);
    let mut target = playlist("Night drive");
    target.id = "target".into();
    fixture
        .backend
        .emit(Event::Playlists(vec![target, playlist("Roadtrip")]));
    fixture.settle(window, cx);
    for confirm in [false, true] {
        cx.update_window(window, |_, window, cx| {
            window.right_click("nav-playlist-target", cx)
        })
        .unwrap();
        fixture.settle(window, cx);
        fixture.menu_item(window, "Delete playlist…", cx);
        cx.update_window(window, |_, window, _| {
            assert!(
                window
                    .find_by_label("Delete “Night drive” from Navidrome?")
                    .visible()
            );
            assert!(window.find("confirm-delete").visible());
            assert!(window.find("playlist-options").visible());
        })
        .unwrap();
        if confirm {
            fixture.expect("delete the right-clicked playlist", |command, _| {
                let expected = Mutation::DeletePlaylist("target".into());
                assert!(matches!(command, Command::Mutate(ref mutation) if *mutation == expected));
                vec![
                    Event::Playlists(vec![playlist("Roadtrip")]),
                    Event::MutationDone(expected),
                ]
            });
        }
        fixture.click(
            window,
            if confirm {
                "confirm-delete"
            } else {
                "cancel-delete"
            },
            cx,
        );
        cx.update_window(window, |_, window, _| {
            assert!(window.try_find("confirm-delete").is_none());
            assert_eq!(window.try_find("nav-playlist-target").is_none(), confirm);
            assert!(window.find("playlist-options").visible());
        })
        .unwrap();
    }
    fixture.assert_script_finished();
    fixture.diagnose("deleted confirmed row", window, &app, cx);
}
