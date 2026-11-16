use crate::common::ui::Fixture;
use gpui_kit::{
    AppContext, ElementId, ScrollDelta, TestAppContext, point, px,
    test::{TestQueryExt, TestWindowExt},
};
use sonora::{
    model::{Album, Preferences, Track},
    service::{Command, Event},
};
use std::{cell::Cell, rc::Rc};

#[gpui_kit::test]
fn five_thousand_tracks_page_and_scroll_with_correct_domain_rows_in_every_layout(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let dir = tempfile::tempdir().unwrap();
    let mut f = Fixture::open(&dir.path().join("library.sqlite"));
    let tracks: Vec<_> = (0..5000)
        .map(|n| Track {
            id: format!("large-{n:04}"),
            title: format!("Song {n:04}"),
            album_id: format!("large-album-{:04}", n / 5),
            album: format!("Release {:04}", n / 5),
            artist: "Large Artist".into(),
            album_artist: "Large Artist".into(),
            duration: 120.,
            ..Default::default()
        })
        .collect();
    let albums: Vec<_> = (0..1000)
        .map(|n| Album {
            id: format!("large-album-{n:04}"),
            name: format!("Release {n:04}"),
            artist: "Large Artist".into(),
            song_count: 5,
            ..Default::default()
        })
        .collect();
    f.catalog.put_tracks(&tracks, 1).unwrap();
    f.catalog.put_albums(&albums, 1).unwrap();
    let mut prefs = Preferences::default();
    prefs.tabs.active = 0;
    prefs.layout.show_spectrum = false;
    f.catalog.set_state("preferences", &prefs).unwrap();
    let (window, _) = f.mount(cx);
    f.settle(window, cx);
    f.click(window, "main-view-menu", cx);
    f.menu_item(window, "Tracks", cx);
    let pages = Rc::new(Cell::new(0));
    let count = pages.clone();
    f.expect("page after initial 500 tracks", move |command, catalog| {
        let Command::Page(request, revision, query, offset) = command else {
            panic!("expected real paging intent")
        };
        assert_eq!(offset, 500);
        count.set(count.get() + 1);
        vec![Event::Page(
            request,
            revision,
            offset,
            catalog.tracks(&query, offset, 500).unwrap(),
        )]
    });
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("row-flat-large-0600").is_none())
    })
    .unwrap();
    for _ in 0..25 {
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "library-content",
                ScrollDelta::Pixels(point(px(0.), px(-700.))),
                cx,
            )
        })
        .unwrap();
        f.settle(window, cx);
    }
    assert_eq!(pages.get(), 1, "scroll must traverse Command::Page");
    f.assert_script_finished();
    let mut offscreen = 0;
    cx.update_window(window, |_, window, _| {
        for row in window.elements().into_iter().filter(|e| e.visible()) {
            let Some(ElementId::Name(name)) = row.path().last() else {
                continue;
            };
            let Some(id) = name.strip_prefix("row-flat-") else {
                continue;
            };
            let n: usize = id.strip_prefix("large-").unwrap().parse().unwrap();
            assert_eq!(
                row.label(),
                Some(format!("Song {n:04} · Large Artist · Release {:04}", n / 5).as_str())
            );
            if n >= 500 {
                offscreen += 1;
            }
        }
    })
    .unwrap();
    assert!(
        offscreen > 0,
        "previously offscreen paged rows must be visible"
    );
    for layout in ["Album and Tracks", "Album Covers"] {
        f.click(window, "main-view-menu", cx);
        f.menu_item(window, layout, cx);
        for _ in 0..4 {
            cx.update_window(window, |_, window, cx| {
                window.scroll(
                    "library-content",
                    ScrollDelta::Pixels(point(px(0.), px(-500.))),
                    cx,
                )
            })
            .unwrap();
            f.settle(window, cx);
        }
        let mut visible = 0;
        cx.update_window(window, |_, window, _| {
            for row in window.elements().into_iter().filter(|e| e.visible()) {
                let Some(ElementId::Name(name)) = row.path().last() else {
                    continue;
                };
                let prefix = if layout == "Album Covers" {
                    "explorer-album-large-album-"
                } else {
                    "album-panel-large-album-"
                };
                let Some(n) = name.strip_prefix(prefix) else {
                    continue;
                };
                let expected = if layout == "Album Covers" {
                    format!("Select album Release {n} by Large Artist")
                } else {
                    format!("Album Release {n} by Large Artist")
                };
                assert_eq!(row.label(), Some(expected.as_str()));
                visible += 1;
            }
        })
        .unwrap();
        assert!(
            visible > 0,
            "the alternate layout must render identified albums"
        );
    }
}
