use super::{
    settings::{connection_field, connection_message},
    *,
};
use crate::{
    service::SettingsCredentials,
    slskd::{ConnectionStatus, SlskdKey, connection_address},
};
use gpui_kit::component::group_box::GroupBoxVariant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    Save,
    Check,
    Monitor,
}

#[derive(Default)]
pub(super) struct SlskdConnection {
    editing: bool,
    revision: u64,
    pending: Option<(u64, Request)>,
    status: Option<ConnectionStatus>,
    errors: [Option<String>; 4],
    failure: Option<String>,
}

impl SlskdConnection {
    fn restore(&mut self) {
        *self = Self {
            revision: self.revision,
            ..Self::default()
        };
    }

    fn begin(&mut self, request: Request) -> Option<u64> {
        if self.pending.is_some() && (request != Request::Save || self.saving()) {
            return None;
        }
        self.revision += 1;
        self.pending = Some((self.revision, request));
        Some(self.revision)
    }

    fn saving(&self) -> bool {
        matches!(self.pending, Some((_, Request::Save)))
    }

    fn validate(&mut self, [address, key, username, password]: [&str; 4]) -> Option<String> {
        let address = connection_address(address);
        self.errors = Default::default();
        self.failure = None;
        self.errors[0] = address.as_ref().err().map(ToString::to_string);
        if !key.trim().is_empty() {
            self.errors[1] = SlskdKey::new(key).err().map(|error| error.to_string());
        } else if username.trim().is_empty() != password.is_empty() {
            if username.trim().is_empty() {
                self.errors[2] = Some("Enter the slskd web username.".into());
            } else {
                self.errors[3] = Some("Enter the slskd web password.".into());
            }
        }
        if self.errors.iter().any(Option::is_some) {
            return None;
        }
        address.ok().map(Into::into)
    }

    pub(super) fn complete(&mut self, id: u64, status: ConnectionStatus) {
        let Some((current, request)) = self.pending else {
            return;
        };
        if id != current {
            return;
        }
        self.pending = None;
        if request == Request::Save {
            let (message, error) = status.feedback();
            self.failure = error.then_some(message);
            self.editing = error;
        }
        self.status = Some(status);
    }
}

impl MusicApp {
    pub(super) fn restore_slskd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.slskd_connection.restore();
        let address = self.preferences.downloads.url.clone();
        self.slskd_url
            .update(cx, |input, cx| input.set_value(address, window, cx));
        self.clear_slskd_login(window, cx);
    }

    fn clear_slskd_login(&self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [&self.slskd_key, &self.slskd_username, &self.slskd_password] {
            field.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    pub(super) fn watch_slskd(&mut self, cx: &mut Context<Self>) {
        self.slskd_monitor = None;
        if !self.settings {
            return;
        }
        self.check_slskd(Request::Monitor);
        self.slskd_monitor = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                let watching = this.update(cx, |app, cx| {
                    if app.settings && app.check_slskd(Request::Monitor) {
                        cx.notify();
                    }
                    app.settings
                });
                if !matches!(watching, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn check_slskd(&mut self, request: Request) -> bool {
        if !self.preferences.downloads.url.is_empty()
            && let Some(id) = self.slskd_connection.begin(request)
        {
            self.service.send(Command::SlskdTest(id));
            return true;
        }
        false
    }

    fn save_slskd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.slskd_connection.saving() {
            return;
        }
        let [address, key, username, password] = [
            &self.slskd_url,
            &self.slskd_key,
            &self.slskd_username,
            &self.slskd_password,
        ]
        .map(|input| input.read(cx).value().to_string());
        let Some(address) = self
            .slskd_connection
            .validate([&address, &key, &username, &password])
        else {
            cx.notify();
            return;
        };
        let Some(id) = self.slskd_connection.begin(Request::Save) else {
            return;
        };
        self.slskd_connection.editing = true;
        let mut prefs = self.preferences.clone();
        prefs.downloads.url.clone_from(&address);
        prefs.downloads.enabled = true;
        let result = self
            .service
            .save_settings(prefs, SettingsCredentials::Slskd(key, username, password));
        cx.spawn_in(window, async move |this, cx| {
            let saved = result.recv_async().await == Ok(true);
            let _ = this.update_in(cx, |app, window, cx| {
                if app.slskd_connection.pending != Some((id, Request::Save)) {
                    return;
                }
                if saved {
                    app.preferences.downloads.url = address;
                    app.preferences.downloads.enabled = true;
                    app.clear_slskd_login(window, cx);
                    app.service.send(Command::SlskdTest(id));
                } else {
                    app.slskd_connection.complete(id, ConnectionStatus::Failed("Could not save slskd settings. Check that Sonora can write its settings and try again.".into()));
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn slskd_form(&self, app: WeakEntity<Self>, configured: bool) -> SettingGroup {
        let state = &self.slskd_connection;
        let saving = state.saving();
        let failure = state.failure.clone();
        let error = |index: usize, id| state.errors[index].clone().map(|message| (id, message));
        SettingGroup::new()
            .title(if configured { "Change slskd settings" } else { "Connect to slskd" })
            .item(connection_field("slskd address", self.slskd_url.clone(), None, error(0, "slskd-address-error"), &["soulseek", "slskd", "url", "host"]))
            .item(connection_field("API key", self.slskd_key.clone(), Some("Use a key with the readwrite role, or sign in below."), error(1, "slskd-key-error"), &["soulseek", "slskd", "token"]))
            .item(connection_field("Username", self.slskd_username.clone(), Some("Use the username from slskd's web interface, not your Soulseek account."), error(2, "slskd-username-error"), &["soulseek", "slskd", "login"]))
            .item(connection_field("Password", self.slskd_password.clone(), Some("Leave all credential fields blank to keep the saved login, or if slskd does not require one."), error(3, "slskd-password-error"), &["soulseek", "slskd", "login"]))
            .item(SettingItem::render(move |_, _, cx| {
                let save = app.clone();
                let retry = app.clone();
                div().flex().flex_col().gap_2()
                    .child(div().flex().child(Button::new("link-slskd").primary().label("Save and test").disabled(saving)
                        .on_click(move |_, window, cx| { let _ = save.update(cx, |app, cx| app.save_slskd(window, cx)); })))
                    .when(saving, |form| form.child(connection_message("slskd-progress", "Saving and testing…".into(), false, cx)))
                    .when_some(failure.clone(), |form, failure| form.child(div().flex().items_start().gap_2()
                        .child(connection_message("slskd-form-error", failure, true, cx))
                        .child(Button::new("retry-slskd").small().label("Retry").on_click(move |_, window, cx| {
                            let _ = retry.update(cx, |app, cx| app.save_slskd(window, cx));
                        }))))
            }).keywords(["slskd", "save", "test", "connect"]))
    }

    fn slskd_summary(&self, app: WeakEntity<Self>) -> SettingGroup {
        let state = &self.slskd_connection;
        let pending = state.pending.is_some();
        let checking = matches!(state.pending, Some((_, Request::Check)));
        let editing = state.editing;
        let label = state
            .status
            .as_ref()
            .map(ConnectionStatus::label)
            .unwrap_or(if pending {
                "Checking…"
            } else {
                "Not checked"
            });
        let feedback = state.status.as_ref().map(ConnectionStatus::feedback);
        let address = connection_address(&self.preferences.downloads.url)
            .map(String::from)
            .unwrap_or_else(|_| "Invalid saved slskd address".into());
        SettingGroup::new()
            .title("slskd connection")
            .description(address)
            .variant(GroupBoxVariant::Outline)
            .item(
                SettingItem::render(move |_, _, cx| {
                    let check = app.clone();
                    let edit = app.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(connection_message("slskd-status", label.into(), false, cx))
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Button::new("check-slskd")
                                        .label("Check connection")
                                        .disabled(pending)
                                        .on_click(move |_, _, cx| {
                                            let _ = check.update(cx, |app, cx| {
                                                if app.check_slskd(Request::Check) {
                                                    cx.notify();
                                                }
                                            });
                                        }),
                                )
                                .when(!editing, |actions| {
                                    actions.child(
                                        Button::new("change-slskd")
                                            .label("Change slskd settings")
                                            .on_click(move |_, _, cx| {
                                                let _ = edit.update(cx, |app, cx| {
                                                    app.slskd_connection.editing = true;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                }),
                        )
                        .when_some(
                            if checking {
                                Some(("Checking connection…".into(), false))
                            } else {
                                feedback.clone()
                            },
                            |card, (message, error)| {
                                card.child(connection_message(
                                    "slskd-check-result",
                                    message,
                                    error,
                                    cx,
                                ))
                            },
                        )
                })
                .keywords(["slskd", "soulseek", "connection", "status"]),
            )
    }

    pub(super) fn download_settings(&self, app: WeakEntity<Self>) -> SettingPage {
        let configured = !self.preferences.downloads.url.is_empty();
        let mut page = SettingPage::new("Downloads")
            .icon(Icon::new(Glyph::Save))
            .resettable(false);
        if configured {
            page = page.group(self.slskd_summary(app.clone()));
        }
        if !configured || self.slskd_connection.editing {
            page = page.group(self.slskd_form(app.clone(), configured));
        }
        let read = app.clone();
        page.group(
            SettingGroup::new().title("Soulseek downloads").item(
                SettingItem::new(
                    "Offer missing releases for download",
                    SettingField::switch(
                        move |cx: &App| {
                            read.read_with(cx, |app, _| app.preferences.downloads.enabled)
                                .unwrap_or(false)
                        },
                        move |value, cx: &mut App| {
                            let _ = app.update(cx, |app, cx| {
                                app.preferences.downloads.enabled = value;
                                app.save_settings(cx);
                                cx.notify();
                            });
                        },
                    )
                    .default_value(false),
                )
                .description("Adds Find on Soulseek to missing releases on artist pages."),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionStatus, Request, SlskdConnection};

    #[test]
    fn anonymous_auth_is_allowed_but_partial_credentials_are_not() {
        let mut state = SlskdConnection::default();
        assert!(
            state
                .validate(["https://slskd.example", "", "", ""])
                .is_some()
        );
        assert!(
            state
                .validate(["ftp://slskd.example", "", "listener", ""])
                .is_none()
        );
        assert!(state.errors[0].is_some());
        assert!(state.errors[3].is_some());
        assert!(
            state
                .validate(["https://slskd.example", "short", "listener", ""])
                .is_none()
        );
        assert!(state.errors[1].is_some());
    }

    #[test]
    fn account_or_form_changes_make_old_checks_stale() {
        let mut state = SlskdConnection::default();
        let old = state.begin(Request::Check).unwrap();
        let save = state.begin(Request::Save).unwrap();
        state.complete(old, ConnectionStatus::Unreachable("old result".into()));
        assert!(state.saving());
        state.restore();
        let check = state.begin(Request::Monitor).unwrap();
        state.complete(save, ConnectionStatus::Unreachable("other account".into()));
        assert_eq!(state.pending, Some((check, Request::Monitor)));
        assert!(state.status.is_none());
    }
}
