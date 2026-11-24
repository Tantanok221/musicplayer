//! Update status in Settings → About and the status bar's restart button.
use super::*;
use crate::update::{UpdateState, UpdateStatus};

impl MusicApp {
    pub(super) fn update_changed(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        self.update = Some(status);
        self.notify_part(Part::Status, cx);
        cx.notify();
    }

    /// The status bar's restart button, shown once an update is installed.
    pub(super) fn update_restart_button(&self) -> Option<Button> {
        self.update.as_ref()?.ready()?;
        let service = self.service.clone();
        Some(
            Button::new("update-restart")
                .small()
                .label("Restart to update")
                .on_click(move |_, _, _| service.send(Command::RestartToUpdate)),
        )
    }

    pub(super) fn about_settings(&self) -> SettingPage {
        let status = self.update.clone().unwrap_or(UpdateStatus {
            current: crate::update::COMMIT.to_owned(),
            state: UpdateState::Unavailable(None),
        });
        let service = self.service.clone();
        SettingPage::new("About")
            .icon(Icon::new(Glyph::Info))
            .group(
                SettingGroup::new()
                    .title("Updates")
                    .item(SettingItem::render(move |_, _, _| {
                        let label = status.label();
                        let can_check = !matches!(
                            status.state,
                            UpdateState::Unavailable(_) | UpdateState::Checking
                        );
                        let service = service.clone();
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .id("update-status")
                                    .test_support()
                                    .role(Role::Status)
                                    .aria_label(label.clone())
                                    .child(label),
                            )
                            .child(
                                h_flex().child(
                                    Button::new("update-check")
                                        .label("Check for updates")
                                        .disabled(!can_check)
                                        .on_click(move |_, _, _| {
                                            service.send(Command::CheckForUpdates)
                                        }),
                                ),
                            )
                    })),
            )
    }
}
