//! Production inputs, captured intent, and authoritative paused-player snapshots.
use crate::common::ui::{Fixture, PlaybackIntent, start};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, ElementId, Entity, MouseButton, Pixels, SharedString,
    TestAppContext, Window, point, px,
    test::{TestEventExt, TestQueryExt, TestWindowExt},
};
use sonora::{
    model::{PlaybackSnapshot, duration_label},
    playback::{Placement, PlayerCommand, QueueEdit, test_support::PausedPlayer},
    service::Event,
    ui::MusicApp,
};
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

fn row(id: u64) -> SharedString {
    format!("queue-entry-{id}").into()
}

fn player_commands(fixture: &Fixture) -> Vec<PlayerCommand> {
    fixture
        .backend
        .player_commands()
        .try_iter()
        .filter(|command| !matches!(command, PlayerCommand::Preferences(_)))
        .collect()
}

fn one_player_command(fixture: &Fixture) -> PlayerCommand {
    let mut commands = player_commands(fixture);
    assert_eq!(commands.len(), 1, "exactly one player command");
    commands.pop().unwrap()
}

fn publish(
    fixture: &mut Fixture,
    snapshot: &PlaybackSnapshot,
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
) {
    eprintln!(
        "scenario={} queue ids={:?} tracks={:?} revision={} undo={} index={:?} position={}",
        cx.test_function_name().unwrap_or("playback"),
        snapshot.queue_ids,
        snapshot
            .queue
            .iter()
            .map(|track| &track.id)
            .collect::<Vec<_>>(),
        snapshot.queue_revision,
        snapshot.can_undo_queue,
        snapshot.index,
        snapshot.position
    );
    fixture.backend.emit(Event::Playback(snapshot.clone()));
    fixture.settle(window, cx);
}

fn start_queue(
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Fixture,
    AnyWindowHandle,
    Entity<MusicApp>,
    PausedPlayer,
    PlaybackSnapshot,
) {
    let (dir, mut fixture, window, app) = start(cx);
    let player = PausedPlayer::start(WAIT).unwrap();
    let tracks = fixture
        .catalog
        .by_ids(&[
            "track-1".into(),
            "track-2".into(),
            "track-2".into(),
            "track-3".into(),
        ])
        .unwrap();
    let snapshot = player
        .command(PlayerCommand::Restore(tracks, 0, 12., None), WAIT)
        .unwrap();
    publish(&mut fixture, &snapshot, window, cx);
    assert!(player_commands(&fixture).is_empty());
    assert_queue(window, &snapshot, &[], cx);
    (dir, fixture, window, app, player, snapshot)
}

fn assert_queue(
    window: AnyWindowHandle,
    snapshot: &PlaybackSnapshot,
    selected: &[u64],
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let entries = window
            .elements()
            .into_iter()
            .filter_map(|element| {
                let ElementId::Name(name) = element.path().last()? else {
                    return None;
                };
                let id = name.strip_prefix("queue-entry-")?.parse::<u64>().unwrap();
                element.visible().then_some((id, element))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            entries.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            *snapshot.queue_ids,
            "visible occurrence order"
        );
        for (index, (id, element)) in entries.iter().enumerate() {
            assert_eq!(
                element.label(),
                Some(
                    format!(
                        "Queue {}: {} · {}",
                        index + 1,
                        snapshot.queue[index].title,
                        snapshot.queue[index].artist_label()
                    )
                    .as_str()
                )
            );
            assert_eq!(element.selected(), Some(selected.contains(id)));
        }
        assert_eq!(
            window.find("undo-queue-edit").disabled() == Some(true),
            !snapshot.can_undo_queue
        );
    })
    .unwrap();
}

fn context_menu(
    fixture: &mut Fixture,
    window: AnyWindowHandle,
    id: impl Into<ElementId>,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, cx| window.right_click(id, cx))
        .unwrap();
    fixture.settle(window, cx);
}

#[gpui_kit::test]
fn album_play_next_and_queue_last_send_one_intended_command_each(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.capture_playback();
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    let before = fixture
        .catalog
        .by_ids(&["track-1".into()])
        .unwrap()
        .remove(0);
    let snapshot = PlaybackSnapshot {
        track: Some(before.clone()),
        queue: std::sync::Arc::new(vec![before]),
        queue_ids: std::sync::Arc::new(vec![71]),
        queue_revision: 9,
        index: Some(0),
        duration: 120.,
        ..Default::default()
    };
    publish(&mut fixture, &snapshot, window, cx);
    for (label, intent) in [
        ("Play Album", PlaybackIntent::PlayAlbum("album-2".into())),
        (
            "Play Next",
            PlaybackIntent::EnqueueAlbum("album-2".into(), Placement::Next),
        ),
        (
            "Add to End of Queue",
            PlaybackIntent::EnqueueAlbum("album-2".into(), Placement::Last),
        ),
    ] {
        let count = fixture.playback_intents().len();
        context_menu(&mut fixture, window, "explorer-album-album-2", cx);
        fixture.menu_item(window, label, cx);
        assert_eq!(fixture.playback_intents().len(), count + 1);
        assert_eq!(fixture.playback_intents().last(), Some(&intent));
        assert!(
            player_commands(&fixture).is_empty(),
            "album intents belong to the service"
        );
    }
    fixture.diagnose("album queue intentions", window, &app, cx);
    // The controlled backend has not acknowledged playback, so the queue stays unchanged.
    fixture.click(window, ("workspace-tab", 2_u64), cx);
    assert_queue(window, &snapshot, &[], cx);
}

#[gpui_kit::test]
fn duplicate_queue_occurrences_select_move_remove_and_undo_through_real_player(
    cx: &mut TestAppContext,
) {
    let (_dir, mut fixture, window, app, mut player, initial) = start_queue(cx);
    let ids = initial.queue_ids.clone();
    fixture.click(window, row(ids[1]), cx);
    fixture.press(window, "shift-down", cx);
    assert_queue(window, &initial, &[ids[1], ids[2]], cx);
    assert!(
        player_commands(&fixture).is_empty(),
        "selection must not edit playback"
    );
    fixture.press(window, "alt-down", cx);
    assert_queue(window, &initial, &[ids[1], ids[2]], cx);
    let command = one_player_command(&fixture);
    assert!(
        matches!(&command, PlayerCommand::EditQueue(revision, QueueEdit::Move { indices, before: 4 }) if *revision == initial.queue_revision && indices == &[1, 2])
    );
    let moved = player.command(command, WAIT).unwrap();
    assert_eq!(*moved.queue_ids, [ids[0], ids[3], ids[1], ids[2]]);
    assert_eq!(moved.index, Some(0));
    assert_eq!(moved.position, 12.);
    publish(&mut fixture, &moved, window, cx);
    assert_queue(window, &moved, &[ids[1], ids[2]], cx);

    fixture.click(window, format!("queue-remove-{}", ids[1]), cx);
    let command = one_player_command(&fixture);
    assert!(
        matches!(&command, PlayerCommand::EditQueue(revision, QueueEdit::Remove(indices)) if *revision == moved.queue_revision && indices == &[2, 3])
    );
    assert_queue(window, &moved, &[ids[1], ids[2]], cx);
    let removed = player.command(command, WAIT).unwrap();
    assert_eq!(*removed.queue_ids, [ids[0], ids[3]]);
    publish(&mut fixture, &removed, window, cx);
    assert_queue(window, &removed, &[], cx);
    fixture.click(window, "undo-queue-edit", cx);
    let command = one_player_command(&fixture);
    assert!(matches!(command, PlayerCommand::UndoQueue));
    let undone = player.command(command, WAIT).unwrap();
    assert_eq!(undone.queue_ids, moved.queue_ids);
    assert!(!undone.can_undo_queue);
    publish(&mut fixture, &undone, window, cx);
    assert_queue(window, &undone, &[], cx);
    fixture.click(window, "undo-queue-edit", cx);
    assert!(
        player_commands(&fixture).is_empty(),
        "disabled Undo cannot emit a command"
    );

    cx.update_window(window, |_, window, cx| {
        window.drag_to(row(ids[3]), "queue-drop-end", cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let command = one_player_command(&fixture);
    assert!(
        matches!(&command, PlayerCommand::EditQueue(revision, QueueEdit::Move { indices, before: 4 }) if *revision == undone.queue_revision && indices == &[1])
    );
    let dragged = player.command(command, WAIT).unwrap();
    assert_eq!(dragged.queue_ids, initial.queue_ids);
    publish(&mut fixture, &dragged, window, cx);
    assert_queue(window, &dragged, &[ids[3]], cx);
    fixture.diagnose("duplicate occurrences edited", window, &app, cx);
    player.shutdown(WAIT).unwrap();
}

#[gpui_kit::test]
fn obsolete_queue_menu_edits_and_snapshots_cannot_claim_success(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app, mut player, old) = start_queue(cx);
    context_menu(&mut fixture, window, row(old.queue_ids[2]), cx);
    let current = player
        .command(
            PlayerCommand::EditQueue(
                old.queue_revision,
                QueueEdit::Move {
                    indices: vec![3],
                    before: 1,
                },
            ),
            WAIT,
        )
        .unwrap();
    assert!(current.queue_revision > old.queue_revision);
    assert!(current.can_undo_queue);
    fixture.menu_item(window, "Remove from Queue", cx);
    let command = one_player_command(&fixture);
    assert!(
        matches!(&command, PlayerCommand::EditQueue(revision, QueueEdit::Remove(indices)) if *revision == old.queue_revision && indices == &[2])
    );
    assert_queue(window, &old, &[], cx);
    publish(&mut fixture, &current, window, cx);
    assert_queue(window, &current, &[], cx);
    let rejected = player.command(command, WAIT).unwrap();
    assert_eq!(rejected.queue_ids, current.queue_ids);
    assert_eq!(rejected.queue_revision, current.queue_revision);
    assert_eq!(rejected.can_undo_queue, current.can_undo_queue);
    publish(&mut fixture, &rejected, window, cx);
    assert_queue(window, &current, &[], cx);
    // An older response must not put removed/replaced occurrences back on screen.
    publish(&mut fixture, &old, window, cx);
    assert_queue(window, &current, &[], cx);
    fixture.diagnose("obsolete queue reply rejected", window, &app, cx);
    player.shutdown(WAIT).unwrap();
}

fn seek_geometry(window: &mut Window) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let slider_id = window
        .find_by_label("Playback position")
        .path()
        .last()
        .unwrap()
        .clone();
    let scope = window.within(slider_id);
    (
        scope.find("slider-bar-container").bounds(),
        scope.find(("slider-thumb", 0_u32)).bounds(),
    )
}

#[gpui_kit::test]
fn seek_drag_previews_survives_position_updates_and_sends_once_on_release(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app, mut player, mut snapshot) = start_queue(cx);
    let (start, target) = cx
        .update_window(window, |_, window, _| {
            let (bounds, thumb) = seek_geometry(window);
            let from = thumb.center();
            let to = point(bounds.left() + bounds.size.width * 0.75, bounds.center().y);
            (from, to)
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.pointer_move(start, None, cx);
        window.pointer_down(start, MouseButton::Left, cx);
        window.pointer_move(start + point(px(8.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_move(target, Some(MouseButton::Left), cx);
        window.pointer_move(target, Some(MouseButton::Left), cx);
    })
    .unwrap();
    fixture.settle(window, cx);
    let preview = cx
        .update_window(window, |_, window, _| {
            let (_, thumb) = seek_geometry(window);
            assert!(
                thumb.left() <= target.x && target.x <= thumb.right(),
                "75-percent pointer target must remain inside the rendered thumb: {thumb:?}"
            );
            assert_eq!(window.find("playback-elapsed").label(), Some("1:30"));
            thumb.center()
        })
        .unwrap();
    assert!(
        player_commands(&fixture).is_empty(),
        "a held drag must not seek"
    );
    snapshot.position = 37.;
    publish(&mut fixture, &snapshot, window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(seek_geometry(window).1.center(), preview);
        assert_eq!(window.find("playback-elapsed").label(), Some("1:30"));
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.pointer_up(target, MouseButton::Left, cx)
    })
    .unwrap();
    fixture.settle(window, cx);
    let PlayerCommand::Seek(seconds) = one_player_command(&fixture) else {
        panic!("release must seek");
    };
    assert!((seconds - 90.).abs() < 0.12);
    assert!(player_commands(&fixture).is_empty());
    snapshot.position = seconds;
    publish(&mut fixture, &snapshot, window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("playback-elapsed").label(),
            Some(duration_label(seconds).as_str())
        );
    })
    .unwrap();
    snapshot.position = 48.;
    publish(&mut fixture, &snapshot, window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("playback-elapsed").label(), Some("0:48"));
        assert_ne!(seek_geometry(window).1.center(), preview);
    })
    .unwrap();
    assert!(player_commands(&fixture).is_empty());
    player.shutdown(WAIT).unwrap();
}

#[gpui_kit::test]
fn space_types_in_search_but_toggles_once_with_queue_focus(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, _app, mut player, snapshot) = start_queue(cx);
    fixture.click(window, ("workspace-tab", 1_u64), cx);
    fixture.search(window, "Fixture", cx);
    fixture.press(window, "space", cx);
    assert_eq!(fixture.last_query().text, "Fixture ");
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("music-search").value(), Some("Fixture "));
        assert_eq!(window.find("music-search").focused(), Some(true));
    })
    .unwrap();
    assert!(player_commands(&fixture).is_empty());
    fixture.click(window, ("workspace-tab", 2_u64), cx);
    fixture.click(window, row(snapshot.queue_ids[1]), cx);
    fixture.press(window, "space", cx);
    assert!(matches!(
        one_player_command(&fixture),
        PlayerCommand::Toggle
    ));
    assert_queue(window, &snapshot, &[snapshot.queue_ids[1]], cx);
    let mut playing = snapshot;
    playing.playing = true;
    publish(&mut fixture, &playing, window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find("play-pause").label(), Some("Pause"));
    })
    .unwrap();
    player.shutdown(WAIT).unwrap();
}
