//! Opt-in tag writes: real worker, real files in a temporary music folder.
use crate::fixture::{Fixture, Library, Request, event, synced};
use sonora::{
    autotag::ReleaseQuery,
    model::{MetadataPreferences, Preferences, Track},
    playback::PlayerCommand,
    service::{Command, Event, Mutation},
    tags::{EditValue, MusicFolderCheck, TagEdit, TagField, TagReport, read},
};
use std::{collections::BTreeMap, path::Path};

const PATH: &str = "/music/Fixture Artist/Release album/01 Song.flac";

struct Folder {
    dir: tempfile::TempDir,
}

impl Folder {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Fixture Artist/Release album/01 Song.flac");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tone-0.flac"),
            &file,
        )
        .unwrap();
        Self { dir }
    }
    fn file(&self) -> std::path::PathBuf {
        self.dir
            .path()
            .join("Fixture Artist/Release album/01 Song.flac")
    }
    fn title(&self) -> Vec<String> {
        read(&self.file()).unwrap()[&TagField::Title].clone()
    }
}

fn library(folder: &Folder, scan_admin: bool) -> Library {
    let mut library = Library {
        scan_admin,
        ..Library::default()
    };
    library.tracks[0].path = PATH.into();
    library.tracks[0].size = Some(std::fs::metadata(folder.file()).unwrap().len());
    library
}

fn enable(worker: &sonora::service::test_support::Worker, folder: &Folder) {
    worker.service().send(Command::Preferences(Preferences {
        metadata: MetadataPreferences {
            write_tags: true,
            library_folder: folder.dir.path().to_string_lossy().into(),
            verified_library_folder: Some(folder.dir.path().to_string_lossy().into()),
            ..Default::default()
        },
        ..Default::default()
    }));
}

fn retitle(title: &str) -> TagEdit {
    TagEdit {
        tracks: vec!["song".into()],
        fields: BTreeMap::from([(TagField::Title, EditValue::Set(title.into()))]),
    }
}

fn report(worker: &sonora::service::test_support::Worker) -> TagReport {
    let Event::TagsWritten(report) = event(worker, |event| matches!(event, Event::TagsWritten(_)))
    else {
        unreachable!()
    };
    report
}

fn scans(fixture: &Fixture) -> usize {
    fixture
        .requests()
        .iter()
        .filter(|request| request.endpoint == "startScan.view")
        .count()
}

#[test]
fn checks_indexed_music_files_without_writing_or_rescanning() {
    let folder = Folder::new();
    let original = std::fs::read(folder.file()).unwrap();
    let mut library = library(&folder, true);
    library.tracks.truncate(1);
    let fixture = Fixture::new(library);
    let mut worker = fixture.start();
    synced(&worker);
    let check = |path: &Path| {
        let (reply, result) = flume::bounded(1);
        worker
            .service()
            .send(Command::CheckMusicFolder(path.into(), reply));
        result.recv_timeout(crate::fixture::WAIT).unwrap()
    };
    assert_eq!(
        check(folder.dir.path()),
        MusicFolderCheck::Passed {
            found: 1,
            server_path: PATH.into(),
            local_path: folder.file(),
        }
    );
    assert_eq!(std::fs::read(folder.file()).unwrap(), original);
    let missing = folder.dir.path().join("missing");
    assert_eq!(check(&missing), MusicFolderCheck::MissingFolder);
    std::fs::create_dir(&missing).unwrap();
    assert_eq!(
        check(&missing),
        MusicFolderCheck::Unmatched { found: 0, total: 1 }
    );
    std::fs::write(folder.file(), b"not the same file").unwrap();
    let MusicFolderCheck::FileProblem(problem) = check(folder.dir.path()) else {
        panic!("expected a size mismatch");
    };
    assert!(problem.contains(&folder.file().display().to_string()));
    assert!(problem.contains("different file"));
    assert_eq!(scans(&fixture), 0);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_existing_folder_is_not_verified_without_indexed_tracks() {
    let folder = Folder::new();
    let fixture = Fixture::new(Library {
        tracks: vec![],
        albums: vec![],
        ..Default::default()
    });
    let mut worker = fixture.start();
    synced(&worker);
    let (reply, result) = flume::bounded(1);
    worker
        .service()
        .send(Command::CheckMusicFolder(folder.dir.path().into(), reply));
    assert_eq!(
        result.recv_timeout(crate::fixture::WAIT).unwrap(),
        MusicFolderCheck::NoTracks
    );
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn writes_rescans_and_carries_the_rating_to_the_new_id() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, true));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    // Navidrome gives the retitled file a new ID and no rating.
    fixture.edit_library(|library| {
        library.tracks[0] = Track {
            id: "song-new".into(),
            title: "New title".into(),
            user_rating: None,
            ..library.tracks[0].clone()
        };
    });
    fixture.expect_mutation(
        Request::new("setRating.view", &[("id", "song-new"), ("rating", "1")]),
        true,
    );
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("New title")]));
    let written = report(&worker);
    assert_eq!((written.written, written.failures.len()), (1, 0));
    assert_eq!(folder.title(), vec!["New title"]);
    event(
        &worker,
        |event| matches!(event, Event::MutationDone(Mutation::Rating(id, 1)) if id == "song-new"),
    );
    assert_eq!(scans(&fixture), 1);

    worker.service().send(Command::UndoTags);
    let undone = report(&worker);
    assert!(undone.undo);
    assert_eq!(undone.written, 1);
    assert!(folder.title().is_empty());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn without_admin_rights_it_skips_the_scan_and_waits_for_the_watcher() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("Watched")]));
    assert_eq!(report(&worker).written, 1);
    event(
        &worker,
        |event| matches!(event, Event::Status(message) if message.contains("next scan")),
    );
    assert_eq!(folder.title(), vec!["Watched"]);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn refuses_when_off_or_the_file_is_not_in_the_folder() {
    let folder = Folder::new();
    let original = std::fs::read(folder.file()).unwrap();
    let mut wrong = library(&folder, true);
    wrong.tracks[0].path = "Somewhere/Else.flac".into();
    let fixture = Fixture::new(wrong);
    let mut worker = fixture.start();
    synced(&worker);
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("Nope")]));
    let off = report(&worker);
    assert_eq!(off.written, 0);
    assert!(off.failures[0].1.contains("Settings → Metadata"));

    enable(&worker, &folder);
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("Nope")]));
    let missing = report(&worker);
    assert_eq!(missing.written, 0);
    assert!(missing.failures[0].1.contains("Report Real Path"));
    worker.service().send(Command::UndoTags);
    assert!(report(&worker).failures[0].1.contains("no tag edit"));
    assert_eq!(std::fs::read(folder.file()).unwrap(), original);
    assert_eq!(scans(&fixture), 0);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn the_queued_song_picks_up_the_rescanned_tags_and_new_id() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, true));
    let mut worker = fixture.start_paused();
    synced(&worker);
    enable(&worker, &folder);
    worker
        .service()
        .send(Command::Player(PlayerCommand::Restore(
            fixture.catalog().by_ids(&["song".into()]).unwrap(),
            0,
            5.,
            None,
        )));
    event(
        &worker,
        |e| matches!(e, Event::Playback(s) if s.track.as_ref().is_some_and(|t| t.id == "song")),
    );
    fixture.edit_library(|library| {
        library.tracks[0] = Track {
            id: "song-new".into(),
            title: "New title".into(),
            ..library.tracks[0].clone()
        };
    });
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("New title")]));
    assert_eq!(report(&worker).written, 1);
    // The service and player publish independently after a rescan.
    let mut track_updated = false;
    let mut snapshot = None;
    event(&worker, |event| {
        match event {
            Event::TrackUpdated(track) if track.id == "song-new" && track.title == "New title" => {
                track_updated = true;
            }
            Event::Playback(playback)
                if playback
                    .track
                    .as_ref()
                    .is_some_and(|track| track.id == "song-new") =>
            {
                snapshot = Some(playback.clone());
            }
            _ => {}
        }
        track_updated && snapshot.is_some()
    });
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot.track.unwrap().title, "New title");
    let queued: Vec<_> = snapshot.queue.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(queued, ["song-new"]);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn finds_releases_and_loads_their_tracks_from_musicbrainz() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    let query = ReleaseQuery {
        album: "OK Computer".into(),
        artist: "Radiohead".into(),
        tracks: 2,
        ..Default::default()
    };
    worker.service().send(Command::FindReleases(query.clone()));
    let Event::ReleaseCandidates(asked, Ok(found)) = event(&worker, |event| {
        matches!(event, Event::ReleaseCandidates(..))
    }) else {
        panic!("the search failed");
    };
    assert_eq!(asked, query);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "ok-computer");
    assert_eq!(found[0].score, 100);
    assert_eq!(
        found[0].summary(),
        "1997-05-21 · GB · CD · Parlophone NODATA 02 · 2 tracks"
    );

    worker
        .service()
        .send(Command::LoadRelease("ok-computer".into()));
    let Event::ReleaseLoaded(id, Ok(release)) =
        event(&worker, |event| matches!(event, Event::ReleaseLoaded(..)))
    else {
        panic!("the lookup failed");
    };
    assert_eq!(id, "ok-computer");
    assert_eq!(release.group_id, "rg");
    assert_eq!(release.genres, vec!["alternative rock", "art rock"]);
    assert_eq!(release.tracks.len(), 2);
    assert_eq!(
        release.tracks[0].artists,
        vec![("Radiohead".into(), "a1".into())]
    );
    assert_eq!(release.tracks[1].title, "Paranoid Android");
    assert_eq!(release.tracks[1].recording_id, "r2");
    assert_eq!(release.tracks[1].number, 2);
    assert_eq!(release.tracks[1].length_ms, Some(383_000));

    worker.service().send(Command::LoadRelease("gone".into()));
    let Event::ReleaseLoaded(_, Err(error)) =
        event(&worker, |event| matches!(event, Event::ReleaseLoaded(..)))
    else {
        panic!("a missing release should fail");
    };
    assert!(error.contains("404"), "{error}");
    assert!(fixture.requests().contains(&Request::new(
        "metadata/release-search",
        &[
            (
                "query",
                r#"+release:"OK Computer" +artist:"Radiohead" tracks:2"#
            ),
            ("limit", "25"),
        ],
    )));
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn writes_musicbrainz_ids_and_undo_takes_them_out() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    worker.service().send(Command::WriteTags(vec![TagEdit {
        tracks: vec!["song".into()],
        fields: BTreeMap::from([
            (
                TagField::MusicBrainzRelease,
                EditValue::Set("ok-computer".into()),
            ),
            (TagField::MusicBrainzTrack, EditValue::Set("r1".into())),
        ]),
    }]));
    assert_eq!(report(&worker).written, 1);
    let tags = read(&folder.file()).unwrap();
    assert_eq!(tags[&TagField::MusicBrainzRelease], vec!["ok-computer"]);
    assert_eq!(tags[&TagField::MusicBrainzTrack], vec!["r1"]);
    worker.service().send(Command::UndoTags);
    assert!(report(&worker).undo);
    assert!(read(&folder.file()).unwrap()[&TagField::MusicBrainzRelease].is_empty());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn one_song_finds_the_releases_its_recording_is_on() {
    let fixture = Fixture::new(Library::default());
    let worker = fixture.start();
    let query = ReleaseQuery {
        artist: "Radiohead".into(),
        title: "Airbag".into(),
        length_ms: Some(284_000),
        ..Default::default()
    };
    worker.service().send(Command::FindReleases(query));
    let Event::ReleaseCandidates(_, Ok(found)) = event(&worker, |event| {
        matches!(event, Event::ReleaseCandidates(..))
    }) else {
        panic!("the search failed");
    };
    let ids: Vec<_> = found
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    assert_eq!(ids, ["ok-computer", "airbag-ep"]);
    assert_eq!(found[1].artist, "Radiohead");
    assert_eq!(found[1].score, 90);
    assert_eq!(found[1].summary(), "1998 · JP · CD · 3 tracks");
    fixture.finish();
}

#[test]
fn cover_art_is_fetched_embedded_and_undone() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    let cover = |release: &str| {
        vec![TagEdit {
            tracks: vec!["song".into()],
            fields: BTreeMap::from([(
                TagField::CoverArt,
                EditValue::Set(format!("{}{release}", sonora::tags::COVER_PREFIX)),
            )]),
        }]
    };
    worker.service().send(Command::WriteTags(cover("no-art")));
    let missing = report(&worker);
    assert_eq!(missing.written, 0);
    assert_eq!(
        missing.failures,
        vec![(
            "Cover art".to_string(),
            "Cover Art Archive has no front cover for this release".to_string()
        )]
    );
    assert!(read(&folder.file()).unwrap()[&TagField::CoverArt].is_empty());
    worker
        .service()
        .send(Command::WriteTags(cover("ok-computer")));
    assert_eq!(report(&worker).written, 1);
    assert_eq!(
        read(&folder.file()).unwrap()[&TagField::CoverArt],
        vec![sonora::tags::picture_value("image/png", b"\x89PNG cover")]
    );
    worker.service().send(Command::UndoTags);
    assert!(report(&worker).undo);
    assert!(read(&folder.file()).unwrap()[&TagField::CoverArt].is_empty());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn identify_by_sound_fingerprints_the_file_and_asks_acoustid() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    let identify = |key: &str| {
        worker.service().send(Command::Preferences(Preferences {
            metadata: MetadataPreferences {
                write_tags: true,
                library_folder: folder.dir.path().to_string_lossy().into(),
                acoustid_key: key.into(),
                ..Default::default()
            },
            ..Default::default()
        }));
        worker
            .service()
            .send(Command::IdentifyBySound(Box::new(Track {
                id: "song".into(),
                path: PATH.into(),
                duration: 2.,
                ..Default::default()
            })));
        let Event::SoundIdentified(id, result) =
            event(&worker, |event| matches!(event, Event::SoundIdentified(..)))
        else {
            unreachable!()
        };
        assert_eq!(id, "song");
        result
    };
    assert_eq!(
        identify(""),
        Err("Add an AcoustID key in Settings first".to_string())
    );
    assert_eq!(
        identify("wrong"),
        Err("AcoustID: invalid API key".to_string())
    );
    assert_eq!(identify("fixture-key"), Ok("r1".to_string()));
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn a_tag_job_runs_beside_the_catalog_and_reports_each_file() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let (mut worker, gate) = fixture.start_tag_gated();
    synced(&worker);
    enable(&worker, &folder);
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("Written slowly")]));
    let Event::TagProgress(progress) =
        event(&worker, |event| matches!(event, Event::TagProgress(_)))
    else {
        unreachable!()
    };
    assert_eq!((progress.done, progress.total), (0, 1));
    assert_eq!(progress.current, "Song song");

    // The file is still being written (held by the gate); browsing must not wait for it.
    let request = worker
        .service()
        .query(sonora::model::LibraryQuery::default());
    event(
        &worker,
        |event| matches!(event, Event::View(id, _) if *id == request),
    );
    assert!(folder.title().is_empty(), "the gate holds the write");

    gate.send(()).unwrap();
    let written = report(&worker);
    assert_eq!((written.written, written.failures.len()), (1, 0));
    assert_eq!(folder.title(), vec!["Written slowly"]);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_interrupted_write_leaves_the_original_and_its_staged_copy_is_removed() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    fixture.seed();
    let original = std::fs::read(folder.file()).unwrap();
    // Sonora stopped mid-write: the journal says pending and the staged copy
    // still sits beside the file, where Syncthing or Navidrome would pick it up.
    let staged = folder.file().with_file_name(".sonora-tags.01 Song.flac");
    std::fs::write(&staged, b"half written").unwrap();
    let catalog = fixture.catalog();
    let track = catalog.by_ids(&["song".into()]).unwrap().remove(0);
    catalog
        .journal_tag_edit(1, &track, &folder.file(), &read(&folder.file()).unwrap())
        .unwrap();
    drop(catalog);

    let mut worker = fixture.start();
    let Event::Error(message) = event(&worker, |event| matches!(event, Event::Error(_))) else {
        unreachable!()
    };
    assert!(message.contains("stopped while writing tags"), "{message}");
    assert!(!staged.exists(), "the staged copy must be removed");
    assert_eq!(std::fs::read(folder.file()).unwrap(), original);
    assert!(
        fixture
            .catalog()
            .interrupted_tag_edits()
            .unwrap()
            .is_empty()
    );
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

fn search(
    worker: &sonora::service::test_support::Worker,
    query: &ReleaseQuery,
) -> Result<Vec<sonora::autotag::ReleaseCandidate>, String> {
    worker.service().send(Command::FindReleases(query.clone()));
    let Event::ReleaseCandidates(_, result) = event(
        worker,
        |event| matches!(event, Event::ReleaseCandidates(asked, _) if asked == query),
    ) else {
        unreachable!()
    };
    result
}

fn searches(fixture: &Fixture) -> Vec<String> {
    fixture
        .requests()
        .into_iter()
        .filter(|request| request.endpoint == "metadata/release-search")
        .map(|request| request.params["query"].clone())
        .collect()
}

#[test]
fn an_edition_note_or_another_spelling_of_the_artist_still_finds_the_album() {
    let fixture = Fixture::new(Library::default());
    let mut worker = fixture.start();
    // Tagged with an edition note MusicBrainz keeps out of the title, by an
    // artist spelt differently, and with one song of the release missing.
    let query = ReleaseQuery {
        album: "OK Computer (Collector's Edition)".into(),
        artist: "レディオヘッド".into(),
        tracks: 1,
        ..Default::default()
    };
    let found = search(&worker, &query).expect("the search should succeed");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "ok-computer");
    assert_eq!(
        searches(&fixture),
        vec![
            r#"+(release:"OK Computer (Collector's Edition)" OR release:"OK Computer") +artist:"レディオヘッド" tracks:1"#,
            r#"+(release:"OK Computer (Collector's Edition)" OR release:"OK Computer") artist:"レディオヘッド" tracks:1"#,
        ]
    );
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn a_busy_musicbrainz_is_retried_and_then_explained() {
    let fixture = Fixture::new(Library {
        musicbrainz_busy: 2,
        ..Library::default()
    });
    let mut worker = fixture.start();
    let query = ReleaseQuery {
        album: "OK Computer".into(),
        artist: "Radiohead".into(),
        ..Default::default()
    };
    let found = search(&worker, &query).expect("two busy answers are retried");
    assert_eq!(found[0].id, "ok-computer");
    assert_eq!(searches(&fixture).len(), 3);

    fixture.edit_library(|library| library.musicbrainz_busy = 10);
    let error = search(&worker, &query).expect_err("a server that stays busy fails");
    assert_eq!(
        error,
        "MusicBrainz is busy right now. Wait a minute and try again."
    );
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

/// Where a FLAC file's audio starts, after its metadata blocks.
fn audio_start(bytes: &[u8]) -> usize {
    assert_eq!(&bytes[..4], b"fLaC");
    let mut at = 4;
    loop {
        let last = bytes[at] & 0x80 != 0;
        let len = u32::from_be_bytes([0, bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        at += 4 + len;
        if last {
            return at;
        }
    }
}

fn inode(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).unwrap().ino()
}

#[test]
fn a_small_edit_rewrites_only_the_tag_area_in_place() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    let original = std::fs::read(folder.file()).unwrap();
    let file = inode(&folder.file());
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("In place")]));
    let written = report(&worker);
    assert_eq!((written.written, written.failures.len()), (1, 0));
    assert_eq!(folder.title(), vec!["In place"]);
    // Only the tag area changed: same file, same size, same audio.
    let edited = std::fs::read(folder.file()).unwrap();
    assert_eq!(
        inode(&folder.file()),
        file,
        "the file was replaced, not edited"
    );
    assert_eq!(edited.len(), original.len());
    let audio = audio_start(&original);
    assert_eq!(audio_start(&edited), audio);
    assert!(edited[audio..] == original[audio..], "the audio changed");
    assert!(!sonora::tags::staged_path(&folder.file()).exists());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_edit_too_big_for_the_tag_area_replaces_the_file_safely() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let mut worker = fixture.start();
    synced(&worker);
    enable(&worker, &folder);
    let original = std::fs::read(folder.file()).unwrap();
    let file = inode(&folder.file());
    // Far more than the fixture's 8 KiB of padding.
    let comment = "long ".repeat(4_000);
    worker.service().send(Command::WriteTags(vec![TagEdit {
        tracks: vec!["song".into()],
        fields: BTreeMap::from([(TagField::Comment, EditValue::Set(comment.clone()))]),
    }]));
    let written = report(&worker);
    assert_eq!((written.written, written.failures.len()), (1, 0));
    assert_eq!(
        read(&folder.file()).unwrap()[&TagField::Comment],
        vec![comment]
    );
    assert_ne!(
        inode(&folder.file()),
        file,
        "a grown tag area replaces the file"
    );
    let edited = std::fs::read(folder.file()).unwrap();
    assert!(edited[audio_start(&edited)..] == original[audio_start(&original)..]);
    assert!(!sonora::tags::staged_path(&folder.file()).exists());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_interrupted_in_place_write_is_put_back_at_startup() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    fixture.seed();
    let original = std::fs::read(folder.file()).unwrap();
    let audio = audio_start(&original);
    // Sonora stopped halfway through rewriting the tag area.
    let mut torn = original.clone();
    torn[audio - 64..audio].fill(0xAB);
    std::fs::write(folder.file(), &torn).unwrap();
    let catalog = fixture.catalog();
    let track = catalog.by_ids(&["song".into()]).unwrap().remove(0);
    let id = catalog
        .journal_tag_edit(1, &track, &folder.file(), &BTreeMap::new())
        .unwrap();
    catalog
        .keep_tag_head(
            id,
            &sonora::tags::HeadBackup {
                length: original.len() as u64,
                head: original[..audio].to_vec(),
            },
        )
        .unwrap();
    drop(catalog);

    let mut worker = fixture.start();
    let Event::Error(message) = event(&worker, |event| matches!(event, Event::Error(_))) else {
        unreachable!()
    };
    assert!(message.contains("stopped while writing tags"), "{message}");
    assert!(
        std::fs::read(folder.file()).unwrap() == original,
        "the tag area must be put back"
    );
    assert!(fixture.catalog().tag_head(id).unwrap().is_none());
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}

#[test]
fn an_edit_confirmed_while_another_is_writing_waits_its_turn() {
    let folder = Folder::new();
    let fixture = Fixture::new(library(&folder, false));
    let (mut worker, gate) = fixture.start_tag_gated();
    synced(&worker);
    enable(&worker, &folder);
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("First")]));
    event(&worker, |event| matches!(event, Event::TagProgress(_)));
    worker
        .service()
        .send(Command::WriteTags(vec![retitle("Second")]));
    gate.send(()).unwrap();
    let first = report(&worker);
    assert_eq!((first.written, first.failures), (1, vec![]));
    gate.send(()).unwrap();
    let second = report(&worker);
    assert_eq!((second.written, second.failures), (1, vec![]));
    assert_eq!(folder.title(), vec!["Second"]);
    worker.shutdown(crate::fixture::WAIT).unwrap();
    fixture.finish();
}
