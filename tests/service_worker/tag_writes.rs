//! Opt-in tag writes: real worker, real files in a temporary music folder.
use crate::fixture::{Fixture, Library, Request, event, synced};
use sonora::{
    model::{MetadataPreferences, Preferences, Track},
    service::{Command, Event, Mutation},
    tags::{EditValue, TagEdit, TagField, TagReport, read},
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
        .send(Command::WriteTags(retitle("New title")));
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
        .send(Command::WriteTags(retitle("Watched")));
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
    worker.service().send(Command::WriteTags(retitle("Nope")));
    let off = report(&worker);
    assert_eq!(off.written, 0);
    assert!(off.failures[0].1.contains("Settings → Metadata"));

    enable(&worker, &folder);
    worker.service().send(Command::WriteTags(retitle("Nope")));
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
