//! Playlist interactions with scenario-local expected commands and replies.
use crate::common::ui::{Fixture, start};
use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    model::Playlist,
    platform::shortcut,
    service::{Command, Event, Mutation, PlaylistTarget},
};

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
