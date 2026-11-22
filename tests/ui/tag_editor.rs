//! The opt-in tag editor: context menu entry, multi-edit form, failure and undo.
use crate::common::ui::{Fixture, start};
use gpui_kit::{AnyWindowHandle, AppContext, TestAppContext, test::TestWindowExt};
use sonora::{
    model::{MetadataPreferences, Preferences, Track},
    platform::{Account, shortcut},
    service::{Command, Event},
    tags::{EditValue, TagField, TagReport},
};
use std::sync::Arc;

fn writes_tags(fixture: &mut Fixture, on: bool) {
    let mut prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    prefs.metadata = MetadataPreferences {
        write_tags: on,
        library_folder: "/Volumes/music".into(),
    };
    fixture.backend.emit(Event::Ready(
        Some(Account {
            url: "https://fixture.invalid".into(),
            username: "fixture".into(),
        }),
        prefs,
    ));
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

fn label(window: AnyWindowHandle, id: &str, cx: &mut TestAppContext) -> Option<String> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window
            .try_find(id.to_string())
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
        let Command::WriteTags(edit) = command else {
            panic!("expected a tag write");
        };
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
        label(window, "tag-failures", cx).as_deref(),
        Some("Second: Couldn't find the file")
    );
    assert_eq!(
        label(window, "tag-notice", cx).as_deref(),
        Some("Wrote tags to 1 file · 1 not changed")
    );

    fixture.expect("retry", |command, _| {
        assert!(matches!(command, Command::WriteTags(edit) if edit.fields.len() == 1));
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
