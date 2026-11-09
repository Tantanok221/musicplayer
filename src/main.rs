mod artwork_cache;
mod ui;
use anyhow::Result;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use sonora::{
    platform::{Paths, shortcut},
    service::{Command, Service},
};
use ui::*;

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("sonora-network")
        .enable_all()
        .build()?;
    let paths = Paths::new()?;
    let (service, events) = Service::start(paths, runtime.handle().clone());
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    let reopen_service = service.clone();
    let reopen_events = events.clone();
    app.on_reopen(move |cx| {
        if let Some(window) = cx.windows().first() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
        } else {
            open(reopen_service.clone(), reopen_events.clone(), cx);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::from(cx.window_appearance()), None, cx);
        cx.bind_keys([
            KeyBinding::new(shortcut("cmd-q", "ctrl-q"), Quit, None),
            KeyBinding::new(shortcut("cmd-,", "ctrl-,"), OpenSettings, Some("Sonora")),
            KeyBinding::new(shortcut("cmd-f", "ctrl-f"), FocusSearch, Some("Sonora")),
            KeyBinding::new(shortcut("cmd-r", "ctrl-r"), Refresh, Some("Sonora")),
            KeyBinding::new(
                shortcut("cmd-right", "ctrl-right"),
                NextTrack,
                Some("Sonora"),
            ),
            KeyBinding::new(
                shortcut("cmd-left", "ctrl-left"),
                PreviousTrack,
                Some("Sonora"),
            ),
            KeyBinding::new(shortcut("cmd-[", "ctrl-["), GoBack, Some("Sonora")),
            KeyBinding::new(shortcut("cmd-]", "ctrl-]"), GoForward, Some("Sonora")),
            KeyBinding::new(
                shortcut("cmd-1", "ctrl-1"),
                ExplorerWorkspace,
                Some("Sonora"),
            ),
            KeyBinding::new(shortcut("cmd-2", "ctrl-2"), QueueWorkspace, Some("Sonora")),
            KeyBinding::new(
                shortcut("cmd-3", "ctrl-3"),
                PlaylistsWorkspace,
                Some("Sonora"),
            ),
            KeyBinding::new(
                shortcut("cmd-4", "ctrl-4"),
                PlayingWorkspace,
                Some("Sonora"),
            ),
        ]);
        cx.bind_keys(playback_bindings());
        cx.bind_keys([
            KeyBinding::new(shortcut("cmd-z", "ctrl-z"), UndoQueueEdit, Some("Queue")),
            KeyBinding::new("backspace", RemoveQueueSelection, Some("Queue")),
            KeyBinding::new("delete", RemoveQueueSelection, Some("Queue")),
            KeyBinding::new("alt-up", MoveQueueEarlier, Some("Queue")),
            KeyBinding::new("alt-down", MoveQueueLater, Some("Queue")),
            KeyBinding::new(shortcut("cmd-a", "ctrl-a"), SelectAllQueued, Some("Queue")),
            KeyBinding::new("up", PreviousQueueEntry, Some("Queue")),
            KeyBinding::new("down", NextQueueEntry, Some("Queue")),
            KeyBinding::new("shift-up", ExtendQueuePrevious, Some("Queue")),
            KeyBinding::new("shift-down", ExtendQueueNext, Some("Queue")),
            KeyBinding::new("enter", PlayQueueSelection, Some("Queue")),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let menu = |name: &str, items| Menu {
            name: name.into(),
            items,
            disabled: false,
        };
        cx.set_menus(vec![
            menu(
                "Sonora",
                vec![
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Quit Sonora", Quit),
                ],
            ),
            menu(
                "File",
                vec![
                    MenuItem::action("Connect to Navidrome…", OpenSettings),
                    MenuItem::action("Refresh library", Refresh),
                ],
            ),
            menu("Edit", vec![MenuItem::action("Find music", FocusSearch)]),
            menu(
                "View",
                vec![
                    MenuItem::action("Back", GoBack),
                    MenuItem::action("Forward", GoForward),
                    MenuItem::separator(),
                    MenuItem::action("Music Explorer", ExplorerWorkspace),
                    MenuItem::action("Queue", QueueWorkspace),
                    MenuItem::action("Now Playing", PlayingWorkspace),
                    MenuItem::action("Playlists", PlaylistsWorkspace),
                    MenuItem::separator(),
                    MenuItem::action("Navigator", ToggleNavigator),
                    MenuItem::action("Queue and information", ToggleInformation),
                    MenuItem::action("Column browser", ToggleBrowser),
                    MenuItem::separator(),
                    MenuItem::action("Restore initial layout", RestoreLayout),
                ],
            ),
            menu(
                "Controls",
                vec![
                    MenuItem::action("Play / pause", TogglePlayback),
                    MenuItem::action("Next", NextTrack),
                    MenuItem::action("Previous", PreviousTrack),
                ],
            ),
            menu("Tools", vec![MenuItem::action("Settings…", OpenSettings)]),
        ]);
        let shutdown = service.clone();
        cx.on_app_quit(move |_| {
            let (tx, rx) = flume::bounded(1);
            shutdown.send(Command::Shutdown(tx));
            async move {
                let _ = rx.recv_async().await;
            }
        })
        .detach();
        open(service, events, cx);
        cx.activate(true);
    });
    drop(runtime);
    Ok(())
}

fn playback_bindings() -> [KeyBinding; 2] {
    [
        KeyBinding::new("space", TogglePlayback, Some("Sonora")),
        KeyBinding::new("space", NoAction, Some("Input")),
    ]
}

fn open(service: Service, events: flume::Receiver<sonora::service::Event>, cx: &mut App) {
    let options = WindowOptions {
        app_id: Some("app.sonora.player".into()),
        window_bounds: Some(WindowBounds::centered(size(px(1440.), px(900.)), cx)),
        window_min_size: Some(size(px(680.), px(540.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Sonora".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        // Route clicks in our custom title bar to its controls instead of AppKit zoom/drag.
        app_owns_titlebar_drag: true,
        ..Default::default()
    };
    gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| MusicApp::new(service, events, window, cx))
    })
    .expect("Could not open Sonora");
}

#[cfg(test)]
mod tests {
    use super::{TogglePlayback, playback_bindings};
    use gpui_kit::{KeyContext, Keymap, Keystroke};

    #[test]
    fn space_types_in_inputs_and_toggles_playback_elsewhere() {
        let keymap = Keymap::new(playback_bindings().into());
        let space = Keystroke::parse("space").unwrap();
        let root = KeyContext::try_from("Sonora").unwrap();
        let input = KeyContext::try_from("Input").unwrap();
        let (bindings, _) =
            keymap.bindings_for_input(std::slice::from_ref(&space), std::slice::from_ref(&root));
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].action().partial_eq(&TogglePlayback));
        let (bindings, _) = keymap.bindings_for_input(&[space], &[root, input]);
        assert!(
            bindings.is_empty(),
            "Text input must receive Space without a playback action"
        );
    }
}
