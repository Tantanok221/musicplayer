//! Controlled Soulseek replies exercise missing-release and download handlers.
use crate::common::ui::{ARTIST, ARTIST_TARGET, Fixture};
use gpui_kit::{
    AnyWindowHandle, AppContext, ElementId, ScrollDelta, TestAppContext, point, px,
    test::TestWindowExt,
};
use sonora::{
    artists::cache_key,
    discography::{DiscographyView, MissingRelease, ReleaseDetails},
    model::Preferences,
    service::{Command, Event, SearchUpdate},
    slskd::{Candidate, Completeness, DownloadJob, JobState, Progress, RemoteFile},
};
use std::sync::Arc;

fn candidate(peer: &str, partial: bool) -> Candidate {
    Candidate {
        username: peer.into(),
        directory: format!("Music\\{peer}\\Missing album"),
        files: (1..=if partial { 1 } else { 2 })
            .map(|number| RemoteFile {
                filename: format!("Music\\{peer}\\Missing album\\{number:02}.flac"),
                size: 1024,
                length: Some(120),
                bit_rate: None,
                bit_depth: Some(16),
                sample_rate: Some(48000),
            })
            .collect(),
        tracks: if partial { 1 } else { 2 },
        locked: 0,
        size: if partial { 1024 } else { 2048 },
        seconds: if partial { 120 } else { 240 },
        format: "FLAC".into(),
        lossless: true,
        completeness: if partial {
            Completeness::Partial
        } else {
            Completeness::Exact
        },
        free_slot: true,
        queue: 0,
        speed: 1024,
    }
}

fn job(state: JobState, done: u32) -> DownloadJob {
    DownloadJob {
        artist: cache_key(ARTIST, "artist-1"),
        artist_name: ARTIST.into(),
        release: "missing".into(),
        title: "Missing album".into(),
        username: "second-peer".into(),
        expected: Some(2),
        files: candidate("second-peer", false)
            .files
            .into_iter()
            .map(|file| file.filename)
            .collect(),
        state,
        progress: Progress {
            done,
            total: 2,
            failed: u32::from(state == JobState::Failed),
            failure: if state == JobState::Failed {
                "Fixture transfer disconnected".into()
            } else {
                String::new()
            },
            active: u32::from(state == JobState::Active),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn reveal(
    fixture: &mut Fixture,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
    cx: &mut TestAppContext,
) {
    let id = id.into();
    for _ in 0..12 {
        let direction = cx
            .update_window(window, |_, window, _| {
                let viewport = window.find("library-content").bounds();
                let Some(target) = window.try_find(id.clone()) else {
                    return -1.;
                };
                let center = target.bounds().center().y;
                if target.visible()
                    && center > viewport.top() + px(10.)
                    && center < viewport.bottom() - px(10.)
                {
                    0.
                } else if center <= viewport.top() + px(10.) {
                    1.
                } else {
                    -1.
                }
            })
            .unwrap();
        if direction == 0. {
            return;
        }
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "library-content",
                ScrollDelta::Pixels(point(px(0.), px(direction * 180.))),
                cx,
            );
        })
        .unwrap();
        fixture.settle(window, cx);
    }
    panic!("missing download control {id:?}");
}

fn start_missing(cx: &mut TestAppContext) -> (tempfile::TempDir, Fixture, AnyWindowHandle) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys(sonora::ui::tab_bindings());
    });
    let dir = tempfile::tempdir().unwrap();
    let mut fixture = Fixture::open(&dir.path().join("catalog.sqlite"));
    fixture.seed();
    let mut prefs: Preferences = fixture.catalog.state("preferences").unwrap();
    prefs.downloads.enabled = true;
    fixture.catalog.set_state("preferences", &prefs).unwrap();
    let (window, _app) = fixture.mount(cx);
    fixture.settle(window, cx);
    fixture.click(window, "new-tab", cx);
    fixture.click(window, ARTIST_TARGET, cx);
    fixture.backend.emit(Event::Discography(
        cache_key(ARTIST, "artist-1"),
        Arc::new(DiscographyView {
            missing: vec![MissingRelease {
                id: "missing".into(),
                title: "Missing album".into(),
                year: Some(2020),
                kind: "Album".into(),
                core: true,
                ..Default::default()
            }],
            ..Default::default()
        }),
    ));
    fixture.settle(window, cx);
    reveal(&mut fixture, window, "missing-release-missing", cx);
    fixture.expect("missing release details", |command, _| {
        assert!(matches!(command, Command::ReleaseDetails(id) if id == "missing"));
        vec![Event::ReleaseDetails(
            "missing".into(),
            Ok(Arc::new(ReleaseDetails::default())),
        )]
    });
    fixture.click(window, "missing-release-missing", cx);
    reveal(&mut fixture, window, "missing-search", cx);
    (dir, fixture, window)
}

fn expect_search(fixture: &mut Fixture) {
    fixture.expect("search selected missing release", |command, _| {
        assert!(matches!(command, Command::SoulseekSearch(id, query) if id == "missing" && query == format!("{ARTIST} Missing album")));
        vec![]
    });
}

fn results(fixture: &Fixture) {
    fixture.backend.emit(Event::SoulseekSearch(
        "missing".into(),
        SearchUpdate::Results(
            Arc::new(vec![
                candidate("first-peer", true),
                candidate("second-peer", false),
            ]),
            Some(2),
        ),
    ));
}

#[gpui_kit::test]
fn search_and_folder_failures_recover_without_marking_partial_downloads_complete(
    cx: &mut TestAppContext,
) {
    let (_dir, mut fixture, window) = start_missing(cx);
    expect_search(&mut fixture);
    fixture.click(window, "missing-search", cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("missing-search").disabled(), Some(true));
        assert!(
            window
                .find("soulseek-search-state")
                .label()
                .unwrap()
                .starts_with("Searching Soulseek")
        );
    })
    .unwrap();
    fixture.backend.emit(Event::SoulseekSearch(
        "missing".into(),
        SearchUpdate::Failed("Fixture search timed out".into()),
    ));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("soulseek-search-state").label(),
            Some("Fixture search timed out")
        );
        assert_eq!(
            window.find("soulseek-query").value(),
            Some("ONE OK ROCK Missing album")
        );
    })
    .unwrap();
    expect_search(&mut fixture);
    fixture.click(window, "missing-search", cx);
    results(&fixture);
    fixture.settle(window, cx);
    fixture.expect("first folder refused", |command, _| {
        let Command::Download(request) = command else {
            panic!("expected folder download");
        };
        assert_eq!(request.release, "missing");
        assert_eq!(request.expected, Some(2));
        assert_eq!(request.candidate, candidate("first-peer", true));
        vec![]
    });
    // Scroll inside the actual artist viewport to the folder action.
    reveal(&mut fixture, window, ("soulseek-download", 0_usize), cx);
    fixture.click(window, ("soulseek-download", 0_usize), cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("download-start").role(),
            Some(gpui_kit::Role::Status)
        );
        assert_eq!(
            window.find(("soulseek-download", 1_usize)).disabled(),
            Some(true)
        );
    })
    .unwrap();
    reveal(&mut fixture, window, ("soulseek-download", 0_usize), cx);
    fixture.click(window, ("soulseek-download", 0_usize), cx);
    fixture.backend.emit(Event::DownloadNotStarted(
        "missing".into(),
        "Fixture peer offline".into(),
    ));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("download-start").role(),
            Some(gpui_kit::Role::Alert)
        );
        assert!(
            window
                .find("download-start")
                .label()
                .unwrap()
                .contains("Fixture peer offline")
        );
    })
    .unwrap();
    fixture.expect("choose alternative folder", |command, _| {
        let Command::Download(request) = command else {
            panic!("expected alternative folder");
        };
        assert_eq!(request.artist, ARTIST);
        assert_eq!(request.artist_id, "artist-1");
        assert_eq!(request.release, "missing");
        assert_eq!(request.title, "Missing album");
        assert_eq!(request.candidate, candidate("second-peer", false));
        vec![Event::Downloads(Arc::new(vec![job(JobState::Active, 1)]))]
    });
    reveal(&mut fixture, window, ("soulseek-download", 1_usize), cx);
    fixture.click(window, ("soulseek-download", 1_usize), cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("missing-download-status").label(),
            Some("Downloading 1/2 · from second-peer")
        );
        assert_eq!(
            window.find("downloads-button").label(),
            Some("Downloads · 1 in progress")
        );
        assert!(window.try_find("missing-download-dismiss").is_none());
    })
    .unwrap();
    fixture
        .backend
        .emit(Event::Downloads(Arc::new(vec![job(JobState::Failed, 1)])));
    fixture.settle(window, cx);
    cx.update_window(window, |_, window, _| {
        let status = window.find("missing-download-status");
        assert!(status.label().unwrap().starts_with("Stopped at 1/2 files"));
        assert!(!status.label().unwrap().contains("Downloaded"));
        assert_eq!(
            window.find("downloads-button").label(),
            Some("Downloads · a download needs attention")
        );
    })
    .unwrap();
    fixture.expect("dismiss failed release for another folder", |command, _| {
        assert!(matches!(command, Command::DismissDownload(id) if id == "missing"));
        vec![Event::Downloads(Arc::new(vec![]))]
    });
    fixture.click(window, "missing-download-dismiss", cx);
    reveal(&mut fixture, window, ("soulseek-download", 1_usize), cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("downloads-button").is_none());
        assert!(window.find(("soulseek-download", 1_usize)).visible());
    })
    .unwrap();
    fixture.assert_script_finished();
}

#[gpui_kit::test]
fn completed_download_waits_for_library_scan_and_clear_sends_only_dismiss(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window) = start_missing(cx);
    fixture.backend.emit(Event::Downloads(Arc::new(vec![job(
        JobState::Downloaded,
        2,
    )])));
    fixture.settle(window, cx);
    fixture.click(window, "downloads-button", cx);
    cx.update_window(window, |_, window, _| {
        let status = window.find("download-release-missing");
        assert!(status.visible());
        assert!(
            status
                .label()
                .unwrap()
                .contains("Downloaded · waiting for Navidrome to scan")
        );
        assert!(window.find(("download-refresh", 0_usize)).visible());
    })
    .unwrap();
    fixture.expect("refresh completed download", |command, _| {
        assert!(matches!(command, Command::Refresh));
        vec![]
    });
    fixture.click(window, ("download-refresh", 0_usize), cx);
    fixture.expect("clear completed download", |command, _| {
        assert!(matches!(command, Command::DismissDownload(id) if id == "missing"));
        vec![Event::Downloads(Arc::new(vec![]))]
    });
    fixture.click(window, ("download-clear", 0_usize), cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("downloads-button").is_none())
    })
    .unwrap();
    fixture.assert_script_finished();
}
