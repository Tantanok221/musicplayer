use super::*;
use crate::{service::SettingsCredentials, settings_save::SaveOutcome, tags::MusicFolderCheck};
use gpui_kit::base::{Toast, ToastTransitionStatus};
use gpui_kit::component::switch::Switch;

const PLAYLIST_SETTINGS_PAGE: usize = 6;

#[derive(Default)]
enum FolderStatus {
    #[default]
    Idle,
    Verified,
    Checking,
    Done(MusicFolderCheck),
}

#[derive(Default)]
pub(super) struct FolderCheck {
    folder: String,
    revision: u64,
    status: FolderStatus,
    picker_failed: bool,
}

impl FolderCheck {
    fn restore(&mut self, prefs: &MetadataPreferences) {
        self.revision += 1;
        self.folder.clone_from(&prefs.library_folder);
        self.status = if prefs.folder_verified() {
            FolderStatus::Verified
        } else {
            FolderStatus::Idle
        };
        self.picker_failed = false;
    }

    fn change(&mut self, folder: &str) -> bool {
        if self.folder == folder {
            return false;
        }
        self.revision += 1;
        self.folder = folder.into();
        self.status = FolderStatus::Idle;
        self.picker_failed = false;
        true
    }

    fn begin(&mut self) -> u64 {
        self.revision += 1;
        self.status = FolderStatus::Checking;
        self.picker_failed = false;
        self.revision
    }

    fn complete(&mut self, request: u64, result: MusicFolderCheck) -> bool {
        if self.revision != request {
            return false;
        }
        self.status = FolderStatus::Done(result);
        true
    }

    fn verified(&self) -> bool {
        matches!(
            self.status,
            FolderStatus::Verified | FolderStatus::Done(MusicFolderCheck::Passed { .. })
        )
    }

    fn save_folder(&self, prefs: &mut MetadataPreferences) {
        if self.folder != prefs.library_folder || !self.verified() {
            prefs.write_tags = false;
        }
        prefs.library_folder.clone_from(&self.folder);
        prefs.verified_library_folder = self.verified().then(|| self.folder.clone());
    }
}

fn folder_check_label(status: &FolderStatus) -> (Option<String>, Option<String>) {
    let label = match status {
        FolderStatus::Idle => return (None, None),
        FolderStatus::Verified => "✓ Music folder verified".into(),
        FolderStatus::Checking => "Checking sample tracks…".into(),
        FolderStatus::Done(result) => match result {
            MusicFolderCheck::Passed {
                found,
                server_path,
                local_path,
            } => {
                return (
                    Some(format!("✓ Found {found}/{found} sample tracks")),
                    Some(format!("{server_path} → {}", local_path.display())),
                );
            }
            MusicFolderCheck::MissingFolder => {
                "✗ Music folder not found. Mount the share or choose another folder.".into()
            }
            MusicFolderCheck::NoTracks => "Can't check until the library syncs.".into(),
            MusicFolderCheck::Unmatched { found, total } => format!(
                "✗ Found {found}/{total} sample tracks. Choose the folder that holds your artist folders, \
                 or turn on Report Real Path for the Sonora player in Navidrome, then check again."
            ),
            MusicFolderCheck::FileProblem(message) | MusicFolderCheck::Failed(message) => {
                format!("✗ {message}")
            }
        },
    };
    (Some(label), None)
}

#[cfg(test)]
mod folder_tests {
    use super::{FolderCheck, MusicFolderCheck};

    #[test]
    fn an_old_result_cannot_verify_a_folder_after_editing_away_and_back() {
        let mut check = FolderCheck::default();
        check.change("/music");
        let request = check.begin();
        check.change("/other");
        check.change("/music");
        assert!(!check.complete(
            request,
            MusicFolderCheck::Passed {
                found: 1,
                server_path: "/server/song.flac".into(),
                local_path: "/music/song.flac".into(),
            }
        ));
        assert!(!check.verified());
    }
}

fn gain_index(mode: &GainMode) -> usize {
    match mode {
        GainMode::Smart => 0,
        GainMode::Track => 1,
        GainMode::Album => 2,
        GainMode::Off => 3,
    }
}

fn gain_from_index(index: usize) -> GainMode {
    match index {
        1 => GainMode::Track,
        2 => GainMode::Album,
        3 => GainMode::Off,
        _ => GainMode::Smart,
    }
}

fn gain_description(mode: &GainMode) -> &'static str {
    match mode {
        GainMode::Smart => {
            "Album gain while the queue is a single album; track gain once it mixes albums."
        }
        GainMode::Track => "Every track plays at the same loudness.",
        GainMode::Album => {
            "Albums play at the same loudness, keeping quiet and loud songs within them."
        }
        GainMode::Off => "Files play at their original level.",
    }
}

fn info_source_key(source: InfoSource) -> &'static str {
    match source {
        InfoSource::Selected => "selected",
        InfoSource::Playing => "playing",
    }
}

fn info_tab_key(tab: InfoTab) -> &'static str {
    match tab {
        InfoTab::Details => "details",
        InfoTab::Lyrics => "lyrics",
        InfoTab::Artwork => "artwork",
        InfoTab::Properties => "properties",
    }
}

fn credential_field(
    title: &'static str,
    input: Entity<InputState>,
    description: Option<&'static str>,
    keywords: &'static [&'static str],
) -> SettingItem {
    let item = SettingItem::new(
        title,
        SettingField::render(move |options, _, _| {
            let field = Input::new(&input).id(title).with_size(options.size());
            if options.layout() == Axis::Vertical {
                field.w_full()
            } else {
                field.w_64()
            }
        }),
    )
    .keywords(keywords.iter().copied());
    match description {
        Some(description) => item.description(description),
        None => item,
    }
}

fn connection_message(
    id: &'static str,
    message: String,
    error: bool,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .role(if error { Role::Alert } else { Role::Status })
        .aria_label(message.clone())
        .text_sm()
        .text_color(if error {
            cx.theme().danger
        } else {
            ink(cx).muted
        })
        .child(message)
}

fn connection_field(
    name: &'static str,
    input: Entity<InputState>,
    hint: Option<&'static str>,
    error: Option<(&'static str, String)>,
    keywords: &'static [&'static str],
) -> SettingItem {
    let item = SettingItem::new(
        name,
        SettingField::render(move |options, _, cx| {
            div()
                .max_w(px(440.))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    Input::new(&input)
                        .id(name)
                        .with_size(options.size())
                        .w_full(),
                )
                .when_some(error.clone(), |field, (id, message)| {
                    field.child(connection_message(id, message, true, cx))
                })
        }),
    )
    .layout(Axis::Vertical)
    .keywords(keywords.iter().copied());
    if let Some(hint) = hint {
        item.description(hint)
    } else {
        item
    }
}

fn layout_switch(
    app: WeakEntity<MusicApp>,
    title: &'static str,
    description: &'static str,
    default_value: bool,
    disabled: bool,
    get: fn(&LayoutPreferences) -> bool,
    set: fn(&mut LayoutPreferences, bool),
) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::switch(
            {
                let app = app.clone();
                move |cx: &App| {
                    app.read_with(cx, |app, _| get(&app.preferences.layout))
                        .unwrap_or(default_value)
                }
            },
            move |value, cx: &mut App| {
                let _ = app.update(cx, |app, cx| {
                    set(&mut app.preferences.layout, value);
                    app.settings_page = LAYOUT_SETTINGS_PAGE;
                    app.save_settings(cx);
                    cx.notify();
                });
            },
        )
        .default_value(default_value),
    )
    .description(description)
    .disabled(disabled)
}

fn layout_choice<const N: usize>(
    app: WeakEntity<MusicApp>,
    title: &'static str,
    description: &'static str,
    options: [(&'static str, &'static str); N],
    default_key: &'static str,
    current: &'static str,
    apply: fn(&mut LayoutPreferences, &str),
) -> SettingItem {
    let selected = options.iter().position(|(key, _)| *key == current);
    let keys: Vec<&'static str> = options.iter().map(|(key, _)| *key).collect();
    let labels: Vec<&'static str> = options.iter().map(|(_, label)| *label).collect();
    SettingItem::new(
        title,
        SettingField::render({
            let app = app.clone();
            move |_, _, _| {
                let app = app.clone();
                let keys = keys.clone();
                let mut group = RadioGroup::vertical(title).selected_index(selected);
                for (index, label) in labels.iter().copied().enumerate() {
                    group = group.child(Radio::new((title, index)).label(label));
                }
                group.on_change(move |index, _, cx| {
                    let Some(key) = keys.get(*index).copied() else {
                        return;
                    };
                    let _ = app.update(cx, |app, cx| {
                        apply(&mut app.preferences.layout, key);
                        app.settings_page = LAYOUT_SETTINGS_PAGE;
                        app.save_settings(cx);
                        cx.notify();
                    });
                })
            }
        })
        .on_reset(
            move |_: &App| current != default_key,
            move |_, cx| {
                let _ = app.update(cx, |app, cx| {
                    apply(&mut app.preferences.layout, default_key);
                    app.settings_page = LAYOUT_SETTINGS_PAGE;
                    app.save_settings(cx);
                    cx.notify();
                });
            },
        ),
    )
    .description(description)
    .layout(Axis::Vertical)
}

impl MusicApp {
    pub(super) fn apply_theme(&self, window: &mut Window, cx: &mut App) {
        let mode = self.preferences.dark.map_or_else(
            || ThemeMode::from(window.appearance()),
            |dark| {
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            },
        );
        Theme::change(mode, Some(window), cx);
        let colors = ink(cx);
        // `Theme::update` copies edited colors into the token layer. The settings
        // sidebar and links read those roles; a `global_mut` edit would leave them
        // on the stock theme.
        Theme::update(cx, |theme| {
            theme.font_size = px(12.);
            theme.radius = px(2.);
            theme.radius_lg = px(3.);
            theme.colors.background = colors.background;
            theme.colors.foreground = colors.text;
            theme.colors.primary = colors.accent;
            theme.colors.primary_foreground = colors.background;
            theme.colors.muted = colors.panel;
            theme.colors.muted_foreground = colors.muted;
            theme.colors.border = colors.line;
            theme.colors.popover = colors.background;
            theme.colors.popover_foreground = colors.text;
            theme.colors.sidebar = colors.panel;
            theme.colors.sidebar_foreground = colors.text;
            theme.colors.sidebar_border = colors.line;
            theme.colors.sidebar_accent = colors.selection;
            theme.colors.sidebar_accent_foreground = colors.text;
            theme.colors.sidebar_primary = colors.accent;
            theme.colors.sidebar_primary_foreground = colors.background;
            theme.colors.link = colors.accent;
            theme.colors.link_hover = colors.accent;
            theme.colors.link_active = colors.accent;
        });
    }

    pub(super) fn restore_layout(&mut self, cx: &mut Context<Self>) {
        self.preferences.layout = LayoutPreferences::default();
        self.layout_generation += 1;
        self.album_list.reset(self.album_row_count());
        cx.notify();
    }

    fn restore_settings_layout(&mut self, cx: &mut Context<Self>) {
        self.restore_layout(cx);
        self.save_settings(cx);
    }

    /// Saves the window frame after it has been still for a moment, so a drag
    /// writes once instead of every frame.
    pub(super) fn remember_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (bounds, maximized, fullscreen) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (bounds, false, false),
            WindowBounds::Maximized(bounds) => (bounds, true, false),
            WindowBounds::Fullscreen(bounds) => (bounds, false, true),
        };
        let frame = crate::platform::WindowFrame {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
            maximized,
            fullscreen,
        };
        self.window_save = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |app, _| app.service.send(Command::SaveWindow(frame)))
                .ok();
        }));
    }

    pub(super) fn save_preferences(&self) {
        self.service
            .send(Command::Preferences(self.preferences.clone()));
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.save_settings_with(SettingsCredentials::Unchanged, cx);
    }

    fn save_settings_with(&mut self, credentials: SettingsCredentials, cx: &mut Context<Self>) {
        self.save_settings_with_label(credentials, "Settings saved", cx);
    }

    fn save_settings_with_label(
        &mut self,
        credentials: SettingsCredentials,
        label: &'static str,
        cx: &mut Context<Self>,
    ) {
        let request = self.settings_save.begin();
        let result = self
            .service
            .save_settings(self.preferences.clone(), credentials);
        cx.spawn(async move |this, cx| {
            let outcome = if result.recv_async().await == Ok(true) {
                SaveOutcome::Saved(label)
            } else {
                SaveOutcome::Failed
            };
            let show = this.update(cx, |app, cx| {
                let show = app.settings_save.complete(request, outcome);
                if show {
                    cx.notify();
                }
                show
            });
            if !matches!(show, Ok(true)) {
                return;
            }
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |app, cx| {
                if app.settings_save.expire(request) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn settings_save_toast(&self, cx: &App) -> Option<impl IntoElement> {
        let colors = ink(cx);
        let (id, label) = match self.settings_save.outcome()? {
            SaveOutcome::Saved(label) => ("settings-saved", label),
            SaveOutcome::Failed => ("settings-save-failed", "Could not save settings"),
        };
        Some(
            Toast::new("settings-save-toast")
                .transition_status(ToastTransitionStatus::Present)
                .absolute()
                .bottom(px(16.))
                .right(px(16.))
                .rounded_md()
                .border_1()
                .border_color(colors.line)
                .bg(colors.panel)
                .text_color(colors.text)
                .shadow_md()
                .px_3()
                .py_2()
                .child(div().id(id).test_support().aria_label(label).child(label)),
        )
    }

    pub(super) fn connection_settings(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let app = cx.entity().downgrade();
        div()
            .id("settings")
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                Settings::new("sonora-settings")
                    .small()
                    .default_selected_index(SelectIndex {
                        page_ix: self.settings_page,
                        group_ix: None,
                    })
                    .page(self.library_settings(app.clone()))
                    .page(self.playback_settings(app.clone()))
                    .page(self.download_settings(app.clone()))
                    .page(self.appearance_settings(app.clone()))
                    // Keep the Layout page at LAYOUT_SETTINGS_PAGE.
                    .page(self.layout_settings(app.clone()))
                    .page(self.metadata_settings(app.clone()))
                    .page(self.playlist_settings(app)),
            )
            .into_any_element()
    }

    fn playlist_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let mut columns = SettingGroup::new().title("Track columns").description("Choose metadata for all playlist track views, including smart playlists. You can also right-click the track header. Title, playlist position, and favorite/rating controls always stay visible.");
        for column in crate::playlists::PlaylistColumn::ALL {
            let read = app.clone();
            let write = app.clone();
            let default = crate::playlists::PlaylistColumns::default().contains(column);
            columns = columns.item(
                SettingItem::new(
                    column.label(),
                    SettingField::render(move |_, _, cx| {
                        let checked = read
                            .read_with(cx, |app, _| {
                                app.preferences.playlist_columns.contains(column)
                            })
                            .unwrap_or(default);
                        let write = write.clone();
                        Switch::new(SharedString::from(format!(
                            "playlist-show-{}",
                            column.key()
                        )))
                        .small()
                        .accessibility_label(format!("Show {} in playlists", column.label()))
                        .checked(checked)
                        .on_click(move |checked, _, cx| {
                            let _ = write.update(cx, |app, cx| {
                                app.preferences.playlist_columns.set(column, *checked);
                                app.settings_page = PLAYLIST_SETTINGS_PAGE;
                                app.save_settings(cx);
                                cx.notify();
                            });
                        })
                    }),
                )
                .keywords(["playlist", "metadata", "columns", column.key()]),
            );
        }
        SettingPage::new("Playlists")
            .resettable(false)
            .icon(Glyph::ListMusic)
            .group(columns)
    }

    pub(super) fn connect_library(&mut self, cx: &mut Context<Self>) {
        let url = self.server_url.read(cx).value().to_string();
        let username = self.username.read(cx).value().to_string();
        if !self.sign_in.begin(&url, &username) {
            cx.notify();
            return;
        }
        self.status = "Connecting…".into();
        self.error = None;
        self.service.send(Command::Connect(
            Account { url, username },
            self.password.read(cx).value().to_string(),
        ));
        cx.notify();
    }

    fn connection_form(&self, app: WeakEntity<Self>) -> SettingGroup {
        let (address_error, username_error) = self.sign_in.field_errors();
        let connecting = self.sign_in.is_connecting();
        let failure = self.sign_in.error().map(str::to_owned);
        let saved_account = self.account.is_some();
        SettingGroup::new()
            .title(if saved_account {
                "Change server or login"
            } else {
                "Connect your music library"
            })
            .item(connection_field(
                "Server address",
                self.server_url.clone(),
                None,
                address_error.map(|error| ("server-address-error", error.to_owned())),
                &["navidrome", "url", "host"],
            ))
            .item(connection_field(
                "Username",
                self.username.clone(),
                None,
                username_error.map(|error| ("username-error", error.to_owned())),
                &["account"],
            ))
            .item(connection_field(
                "Password",
                self.password.clone(),
                saved_account.then_some(
                    "Leave blank to keep your current password for this server and username.",
                ),
                None,
                &["token", "sign in"],
            ))
            .item(
                SettingItem::render(move |_, _, cx| {
                    let submit = app.clone();
                    let retry = app.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div().flex().child(
                                Button::new("connect-library")
                                    .primary()
                                    .label("Connect")
                                    .disabled(connecting)
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            submit.update(cx, |app, cx| app.connect_library(cx));
                                    }),
                            ),
                        )
                        .when(connecting, |form| {
                            form.child(connection_message(
                                "sign-in-progress",
                                "Connecting…".into(),
                                false,
                                cx,
                            ))
                        })
                        .when_some(failure.clone(), |form, message| {
                            form.child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap_2()
                                    .child(connection_message("sign-in-error", message, true, cx))
                                    .child(
                                        Button::new("retry-sign-in")
                                            .small()
                                            .label("Retry")
                                            .on_click(move |_, _, cx| {
                                                let _ = retry
                                                    .update(cx, |app, cx| app.connect_library(cx));
                                            }),
                                    ),
                            )
                        })
                })
                .keywords(["connect", "sign in", "library"]),
            )
    }

    fn connection_summary(&self, account: &Account, app: WeakEntity<Self>) -> SettingGroup {
        let status = self.library_connection.status(&self.connection);
        let description = format!("{} · {}", account.username, account.url);
        let pending = self.library_connection.check_pending();
        let feedback = self
            .library_connection
            .check_feedback()
            .map(|(text, error)| (text.to_owned(), error));
        let editing = self.library_connection.editing;
        SettingGroup::new()
            .title("Connection")
            .variant(gpui_kit::component::group_box::GroupBoxVariant::Outline)
            .description(description)
            .item(
                SettingItem::render(move |_, _, cx| {
                    let check = app.clone();
                    let edit = app.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(connection_message(
                            "connection-card-status",
                            status.into(),
                            false,
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new("check-saved-connection")
                                        .label("Check connection")
                                        .disabled(pending)
                                        .on_click(move |_, _, cx| {
                                            let _ = check.update(cx, |app, cx| {
                                                if let Some(id) =
                                                    app.library_connection.begin_check()
                                                {
                                                    app.service
                                                        .send(Command::TestSavedConnection(id));
                                                    cx.notify();
                                                }
                                            });
                                        }),
                                )
                                .when(!editing, |actions| {
                                    actions.child(
                                        Button::new("change-library-login")
                                            .label("Change server or login")
                                            .on_click(move |_, _, cx| {
                                                let _ = edit.update(cx, |app, cx| {
                                                    app.library_connection.editing = true;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                }),
                        )
                        .when_some(feedback.clone(), |card, (message, error)| {
                            card.child(connection_message(
                                "saved-connection-result",
                                message,
                                error,
                                cx,
                            ))
                        })
                })
                .keywords([
                    "connection",
                    "server",
                    "account",
                    "login",
                    "navidrome",
                ]),
            )
    }

    pub(super) fn library_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let page = SettingPage::new("Library")
            .icon(Icon::new(Glyph::Library))
            .resettable(false);
        let Some(account) = &self.account else {
            return page.group(self.connection_form(app));
        };
        let page = page.group(self.connection_summary(account, app.clone()));
        let page = if self.library_connection.editing {
            page.group(self.connection_form(app))
        } else {
            page
        };
        let server = account.url.clone();
        page.group(
            SettingGroup::new()
                .title("Scrobbling")
                .description("Sonora reports plays only to Navidrome. Link ListenBrainz or Last.fm in your server's personal settings to forward them. Artist information follows the server's metadata-agent order. Uncertain submissions are not resent, to avoid duplicates.")
                .item(SettingItem::render(move |_, _, _| {
                    Link::new("open-navidrome")
                        .href(server.clone())
                        .child("Open Navidrome")
                }).keywords(["listenbrainz", "last.fm", "scrobble", "navidrome"])),
        )
    }

    pub(super) fn download_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let status = match &self.slskd_status {
            Some(Ok(message)) => message.clone(),
            Some(Err(message)) => format!("Not linked · {message}"),
            None => "Sonora searches Soulseek only when you ask it to, one release at a time, and slskd does the downloading.".into(),
        };
        SettingPage::new("Downloads")
            .icon(Icon::new(Glyph::Save))
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("slskd")
                    .description(status.clone())
                    .item(
                        SettingItem::new(
                            "Offer missing releases for download",
                            SettingField::switch(
                                {
                                    let app = app.clone();
                                    move |cx: &App| {
                                        app.read_with(cx, |app, _| {
                                            app.preferences.downloads.enabled
                                        })
                                        .unwrap_or(false)
                                    }
                                },
                                {
                                    let app = app.clone();
                                    move |value, cx: &mut App| {
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.downloads.enabled = value;
                                            app.save_settings(cx);
                                            cx.notify();
                                        });
                                    }
                                },
                            )
                            .default_value(false),
                        )
                        .description("Adds Find on Soulseek to missing releases on artist pages."),
                    )
                    .item(credential_field(
                        "slskd address",
                        self.slskd_url.clone(),
                        None,
                        &["soulseek", "slskd", "url", "host"],
                    ))
                    .item(credential_field(
                        "API key",
                        self.slskd_key.clone(),
                        Some("A key with the readwrite role from slskd's web.authentication.api_keys, if you have set one up."),
                        &["soulseek", "slskd", "token"],
                    ))
                    .item(credential_field(
                        "Username",
                        self.slskd_username.clone(),
                        Some("Without an API key, sign in with the username and password of slskd's web page."),
                        &["soulseek", "slskd", "login"],
                    ))
                    .item(credential_field(
                        "Password",
                        self.slskd_password.clone(),
                        Some("Leave all three empty to keep the saved login, or if your slskd needs none. Saved in a private file on this Mac."),
                        &["soulseek", "slskd", "login"],
                    ))
                    .item(
                        SettingItem::render(move |_, _, _| {
                            let app = app.clone();
                            div().id("slskd-status").test_support().role(Role::Group)
                                .aria_label(status.clone())
                                .child(Button::new("link-slskd")
                                .primary()
                                .label("Save and test")
                                .on_click(move |_, window, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        app.preferences.downloads.url =
                                            app.slskd_url.read(cx).value().trim().to_string();
                                        app.preferences.downloads.enabled = true;
                                        let fields = [
                                            app.slskd_key.clone(),
                                            app.slskd_username.clone(),
                                            app.slskd_password.clone(),
                                        ];
                                        let [key, username, password] = fields
                                            .each_ref()
                                            .map(|field| field.read(cx).value().to_string());
                                        app.save_settings_with(
                                            SettingsCredentials::Slskd(key, username, password),
                                            cx,
                                        );
                                        for field in fields {
                                            field.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                        }
                                        app.slskd_status = Some(Ok("Testing…".into()));
                                        app.service.send(Command::SlskdTest);
                                        cx.notify();
                                    });
                                }))
                        })
                        .keywords(["soulseek", "slskd", "download", "test"]),
                    ),
            )
    }

    pub(super) fn restore_metadata_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = &mut self.preferences.metadata;
        prefs.write_tags = prefs.tag_writes_enabled();
        self.metadata_check.restore(prefs);
        let folder = prefs.library_folder.clone();
        self.metadata_folder
            .update(cx, |input, cx| input.set_value(folder, window, cx));
    }

    pub(super) fn metadata_folder_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change)
            && self.metadata_check.change(input.read(cx).value().trim())
        {
            cx.notify();
        }
    }

    fn choose_music_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose music folder".into()),
        });
        let revision = self.metadata_check.revision;
        cx.spawn_in(window, async move |this, cx| {
            let selection = selection.await;
            let _ = this.update_in(cx, |app, window, cx| {
                if app.metadata_check.revision != revision {
                    return;
                }
                match selection {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first().and_then(|path| path.to_str()) {
                            let path = path.to_string();
                            app.metadata_folder.update(cx, |input, cx| {
                                input.set_value(path.clone(), window, cx);
                            });
                            app.metadata_check.change(path.trim());
                            app.check_music_folder(cx);
                            return;
                        }
                    }
                    Ok(Ok(None)) => return,
                    _ => {}
                }
                app.metadata_check.picker_failed = true;
                cx.notify();
            });
        })
        .detach();
    }

    fn check_music_folder(&mut self, cx: &mut Context<Self>) {
        if self.metadata_check.folder.is_empty() {
            return;
        }
        let request = self.metadata_check.begin();
        let (reply, result) = flume::bounded(1);
        self.service.send(Command::CheckMusicFolder(
            self.metadata_check.folder.as_str().into(),
            reply,
        ));
        cx.spawn(async move |this, cx| {
            let result = result.recv_async().await.unwrap_or_else(|_| {
                MusicFolderCheck::Failed("The library service did not finish the check.".into())
            });
            let _ = this.update(cx, |app, cx| {
                let check = &app.metadata_check;
                let active_folder = check.folder == app.preferences.metadata.library_folder;
                if app.metadata_check.complete(request, result) {
                    if active_folder
                        && !app.metadata_check.verified()
                        && app.preferences.metadata.folder_verified()
                    {
                        app.metadata_check
                            .save_folder(&mut app.preferences.metadata);
                        app.save_preferences();
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn save_metadata_folder(&mut self, cx: &mut Context<Self>) {
        self.metadata_check
            .save_folder(&mut self.preferences.metadata);
        let message = if self.metadata_check.verified() {
            "Music folder saved · verified"
        } else {
            "Music folder saved · not verified"
        };
        self.save_metadata_settings(message, cx);
    }

    fn set_tag_writes(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if !self.metadata_check.verified() {
            return;
        }
        self.metadata_check
            .save_folder(&mut self.preferences.metadata);
        self.preferences.metadata.write_tags = enabled;
        let message = if enabled {
            "Tag writing turned on · music folder verified"
        } else {
            "Tag writing turned off · music folder verified"
        };
        self.save_metadata_settings(message, cx);
    }

    fn save_acoustid_key(&mut self, cx: &mut Context<Self>) {
        self.preferences.metadata.acoustid_key =
            self.acoustid_key.read(cx).value().trim().to_string();
        let message = if self.preferences.metadata.folder_verified() {
            "AcoustID key saved · music folder verified"
        } else {
            "AcoustID key saved · music folder not verified"
        };
        self.save_metadata_settings(message, cx);
    }

    fn save_metadata_settings(&mut self, message: &'static str, cx: &mut Context<Self>) {
        self.save_settings_with_label(SettingsCredentials::Unchanged, message, cx);
        cx.notify();
    }

    pub(super) fn metadata_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let can_check = !self.metadata_check.folder.is_empty()
            && !matches!(self.metadata_check.status, FolderStatus::Checking);
        let verified = self.metadata_check.verified();
        let write_tags = verified
            && self.metadata_check.folder == self.preferences.metadata.library_folder
            && self.preferences.metadata.tag_writes_enabled();
        let picker_failed = self.metadata_check.picker_failed;
        let (status, mapping) = folder_check_label(&self.metadata_check.status);
        SettingPage::new("Metadata")
            .icon(Icon::new(Glyph::SlidersHorizontal))
            .resettable(false)
            .group(
                SettingGroup::new()
                    .title("Tag editing")
                    .description(
                        "Set a music folder to edit tags or use Identify by Sound on local files.",
                    )
                    .item(
                        credential_field(
                            "Music folder",
                            self.metadata_folder.clone(),
                            Some("The local folder or mounted share that holds the same files Navidrome serves."),
                            &["metadata", "tags", "folder", "library", "mount"],
                        )
                        .layout(Axis::Vertical),
                    )
                    .item(
                        SettingItem::render({
                            let app = app.clone();
                            move |_, _, _| {
                                let chooser = app.clone();
                                let checker = app.clone();
                                let saver = app.clone();
                                v_flex()
                                    .gap_2()
                                    .child(h_flex().flex_wrap().gap_2()
                                        .child(Button::new("choose-music-folder")
                                            .small()
                                            .label("Choose music folder…")
                                            .on_click(move |_, window, cx| {
                                                let _ = chooser.update(cx, |app, cx| app.choose_music_folder(window, cx));
                                            }))
                                        .child(Button::new("check-music-folder")
                                            .small()
                                            .label("Check folder")
                                            .disabled(!can_check)
                                            .on_click(move |_, _, cx| {
                                                let _ = checker.update(cx, |app, cx| app.check_music_folder(cx));
                                            }))
                                        .child(Button::new("save-music-folder")
                                            .small()
                                            .label("Save folder")
                                            .on_click(move |_, _, cx| {
                                                let _ = saver.update(cx, MusicApp::save_metadata_folder);
                                            })))
                                    .when(picker_failed, |el| {
                                        el.child(div().text_sm().child("Couldn't open or use that folder. Paste the path instead."))
                                    })
                                    .when_some(status.clone(), |el, status| {
                                        el.child(div()
                                            .id("music-folder-check-result")
                                            .test_support()
                                            .aria_label(status.clone())
                                            .text_sm()
                                            .child(status)
                                            .when_some(mapping.clone(), |el, mapping| el.child(div().child(mapping))))
                                    })
                            }
                        })
                        .keywords(["metadata", "tags", "folder", "check", "choose", "save"]),
                    )
                    .item(
                        SettingItem::new(
                            "Write tags to files",
                            SettingField::render({
                                let app = app.clone();
                                move |options, _, _| {
                                    let app = app.clone();
                                    Switch::new("write-tags-to-files")
                                        .with_size(options.size())
                                        .accessibility_label("Write tags to files")
                                        .disabled(!verified)
                                        .checked(write_tags)
                                        .on_change(move |value, _, cx| {
                                            let _ = app.update(cx, |app, cx| app.set_tag_writes(*value, cx));
                                        })
                                }
                            }),
                        )
                        .description(
                            "Off by default. Check the folder before turning this on. A passing check \
                             doesn't promise write permission or that a share stays mounted. Sonora \
                             still checks each file before writing.",
                        ),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("AcoustID")
                    .item(credential_field(
                        "AcoustID key",
                        self.acoustid_key.clone(),
                        Some("Turns on Identify by Sound for songs with poor tags. Register a free application at acoustid.org."),
                        &["metadata", "acoustid", "fingerprint", "identify", "key"],
                    ))
                    .item(
                        SettingItem::render(move |_, _, _| {
                            let app = app.clone();
                            Button::new("save-acoustid")
                                .small()
                                .label("Save AcoustID key")
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, MusicApp::save_acoustid_key);
                                })
                        })
                        .keywords(["metadata", "acoustid", "key", "save"]),
                    ),
            )
    }

    pub(super) fn playback_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let gain = self.preferences.gain.clone();
        let selected = gain_index(&gain);
        SettingPage::new("Playback")
            .icon(Icon::new(Glyph::AudioLines))
            .group(
                SettingGroup::new().title("ReplayGain").item(
                    SettingItem::new(
                        "Loudness",
                        SettingField::render({
                            let app = app.clone();
                            move |_, _, _| {
                                let app = app.clone();
                                RadioGroup::vertical("replay-gain")
                                    .selected_index(Some(selected))
                                    .child(Radio::new("gain-smart").label("Smart"))
                                    .child(Radio::new("gain-track").label("Track"))
                                    .child(Radio::new("gain-album").label("Album"))
                                    .child(Radio::new("gain-off").label("Off"))
                                    .on_change(move |index, _, cx| {
                                        let mode = gain_from_index(*index);
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.gain = mode;
                                            app.save_settings(cx);
                                            cx.notify();
                                        });
                                    })
                            }
                        })
                        .on_reset(
                            {
                                let app = app.clone();
                                move |cx| {
                                    app.read_with(cx, |app, _| {
                                        app.preferences.gain != GainMode::Smart
                                    })
                                    .unwrap_or(false)
                                }
                            },
                            move |_, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.preferences.gain = GainMode::Smart;
                                    app.save_settings(cx);
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .description(gain_description(&gain))
                    .layout(Axis::Vertical)
                    .keywords([
                        "replaygain",
                        "volume",
                        "loudness",
                        "normalize",
                    ]),
                ),
            )
    }

    pub(super) fn appearance_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let dark = self.preferences.dark;
        let selected = match dark {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        };
        SettingPage::new("Appearance")
            .icon(Icon::new(Glyph::SunMoon))
            .group(
                SettingGroup::new().item(
                    SettingItem::new(
                        "Theme",
                        SettingField::render({
                            let app = app.clone();
                            move |_, _, _| {
                                let app = app.clone();
                                RadioGroup::vertical("appearance")
                                    .selected_index(Some(selected))
                                    .child(Radio::new("appearance-system").label("System"))
                                    .child(Radio::new("appearance-light").label("Light"))
                                    .child(Radio::new("appearance-dark").label("Dark"))
                                    .on_change(move |index, window, cx| {
                                        let dark = match *index {
                                            1 => Some(false),
                                            2 => Some(true),
                                            _ => None,
                                        };
                                        let _ = app.update(cx, |app, cx| {
                                            app.preferences.dark = dark;
                                            app.apply_theme(window, cx);
                                            app.save_settings(cx);
                                            cx.notify();
                                        });
                                    })
                            }
                        })
                        .on_reset(
                            {
                                let app = app.clone();
                                move |cx| {
                                    app.read_with(cx, |app, _| app.preferences.dark.is_some())
                                        .unwrap_or(false)
                                }
                            },
                            move |window, cx| {
                                let _ = app.update(cx, |app, cx| {
                                    app.preferences.dark = None;
                                    app.apply_theme(window, cx);
                                    app.save_settings(cx);
                                    cx.notify();
                                });
                            },
                        ),
                    )
                    .description("System follows the appearance of this computer.")
                    .layout(Axis::Vertical)
                    .keywords(["dark", "light", "system", "theme"]),
                ),
            )
    }

    pub(super) fn layout_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let layout = self.preferences.layout.clone();
        SettingPage::new("Layout")
            .icon(Icon::new(Glyph::PanelsTopLeft))
            .description(
                "These choices are also in the layout menu. Drag the panel edges to resize them.",
            )
            .group(
                SettingGroup::new()
                    .title("Panels")
                    .item(
                        layout_choice(
                            app.clone(),
                            "Navigator and artist browser",
                            "The library list on the left. Auto-hide keeps it out of the way until the pointer reaches the left edge.",
                            [
                                ("show", "Show"),
                                ("auto", "Auto-hide"),
                                ("disabled", "Disabled"),
                            ],
                            "show",
                            navigator_key(&layout),
                            set_navigator,
                        )
                        .keywords(["sidebar", "hover", "auto-hide", "hide"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Queue and information",
                            "The panel on the right.",
                            true,
                            false,
                            |layout| layout.show_information,
                            |layout, value| layout.show_information = value,
                        )
                        .keywords(["sidebar", "lyrics"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Column browser",
                            "Genre, artist, and album columns in the music explorer.",
                            true,
                            false,
                            |layout| layout.explorer_browser,
                            |layout, value| layout.explorer_browser = value,
                        )
                        .keywords(["filter", "genre"]),
                    )
                    .item(
                        layout_switch(
                            app.clone(),
                            "Spectrum visualizer",
                            "Frequency bars in Now Playing. Turn off to save energy.",
                            true,
                            false,
                            |layout| layout.show_spectrum,
                            |layout, value| layout.show_spectrum = value,
                        )
                        .keywords([
                            "visualizer",
                            "spectrum",
                            "cpu",
                            "battery",
                            "now playing",
                        ]),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Track information")
                    .item(
                        layout_choice(
                            app.clone(),
                            "Shows",
                            "The track the information panel follows.",
                            [("selected", "Selected track"), ("playing", "Playing track")],
                            "selected",
                            info_source_key(layout.info_source),
                            |layout, key| {
                                layout.info_source = if key == "playing" {
                                    InfoSource::Playing
                                } else {
                                    InfoSource::Selected
                                };
                            },
                        )
                        .keywords(["selected", "playing", "now playing"]),
                    )
                    .item(
                        layout_choice(
                            app.clone(),
                            "Tab",
                            "Details, lyrics, artwork, or properties.",
                            [
                                ("details", "Details"),
                                ("lyrics", "Lyrics"),
                                ("artwork", "Artwork"),
                                ("properties", "Properties"),
                            ],
                            "details",
                            info_tab_key(layout.info_tab),
                            |layout, key| {
                                layout.info_tab = match key {
                                    "lyrics" => InfoTab::Lyrics,
                                    "artwork" => InfoTab::Artwork,
                                    "properties" => InfoTab::Properties,
                                    _ => InfoTab::Details,
                                };
                            },
                        )
                        .keywords(["lyrics", "artwork", "details", "properties", "tags"]),
                    ),
            )
            .group(
                SettingGroup::new().item(
                    SettingItem::render({
                        let app = app.clone();
                        move |_, _, _| {
                            let app = app.clone();
                            Button::new("restore-layout-settings")
                                .small()
                                .label("Restore initial panel layout")
                                .on_click(move |_, _, cx| {
                                    let _ = app.update(cx, |app, cx| app.restore_settings_layout(cx));
                                })
                        }
                    })
                    .keywords(["reset", "default", "panels", "width"])
                    .on_reset(
                        {
                            let app = app.clone();
                            move |cx| {
                                app.read_with(cx, |app, _| {
                                    app.preferences.layout != LayoutPreferences::default()
                                })
                                .unwrap_or(false)
                            }
                        },
                        move |_, cx| {
                            let _ = app.update(cx, |app, cx| app.restore_settings_layout(cx));
                        },
                    ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{gain_from_index, gain_index};
    use crate::model::GainMode;

    #[test]
    fn replaygain_choices_round_trip() {
        for mode in [
            GainMode::Smart,
            GainMode::Track,
            GainMode::Album,
            GainMode::Off,
        ] {
            assert_eq!(gain_from_index(gain_index(&mode)), mode);
        }
        assert_eq!(gain_from_index(9), GainMode::Smart);
    }
}
