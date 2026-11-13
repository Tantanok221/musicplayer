//! The window drawn as separately cached parts.
//!
//! GPUI re-renders a view and every view above it when it is notified, and
//! reuses the last frame of any view below that was not. Sonora's state lives
//! in one `MusicApp`, so each part is a small view that renders a slice of it
//! and observes it: any change to the app redraws every part, as before, but
//! an animation (the spectrum, lyric fades, a backdrop crossfade) notifies only
//! its own part. The root stays cheap because it only arranges the parts.
//! Animated parts must be direct children of the root, never nested in a
//! heavier part, or their frames would redraw that part as well.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Part {
    Tabs,
    Navigator,
    Center,
    /// The queue above track information in the right panel.
    SidebarQueue,
    /// Track information's title row and Details / Lyrics / Artwork tabs.
    InfoHeader,
    /// Track information's content, including synchronized lyrics.
    InfoBody,
    Status,
    Player,
    NowBackdrop,
    NowDetails,
    NowLyrics,
    Spectrum,
}

pub(super) struct Section {
    app: WeakEntity<MusicApp>,
    part: Part,
    _changes: Subscription,
}

impl Render for Section {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let part = self.part;
        self.app
            .update(cx, |app, cx| app.render_part(part, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

/// The height of the title bar with its workspace tabs.
pub(super) const TABS_HEIGHT: f32 = 36.;
/// The height of track information's title row and tabs.
pub(super) const INFO_HEADER_HEIGHT: f32 = 57.;
/// The spectrum's bars, frequency labels and padding.
pub(super) const SPECTRUM_BAND_HEIGHT: f32 = SPECTRUM_HEIGHT + 8. + 4. + 14. + 12.;

pub(super) fn style() -> StyleRefinement {
    StyleRefinement::default()
}

impl MusicApp {
    /// A cached view of `part`, laid out by `style` since its content is not
    /// measured when the previous frame is reused.
    pub(super) fn part(
        &mut self,
        part: Part,
        style: StyleRefinement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.entity();
        self.sections
            .entry(part)
            .or_insert_with(|| {
                cx.new(|cx| Section {
                    app: app.downgrade(),
                    part,
                    _changes: cx.observe(&app, |_, _, cx| cx.notify()),
                })
            })
            .clone()
            .cached(style)
            .into_any_element()
    }

    /// Redraws only `part`, for changes nothing else shows.
    pub(super) fn notify_part(&self, part: Part, cx: &mut App) {
        if let Some(section) = self.sections.get(&part) {
            section.update(cx, |_, cx| cx.notify());
        }
    }

    fn render_part(
        &mut self,
        part: Part,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let element = match part {
            Part::Tabs => self.tabs(window, cx),
            Part::Navigator => self.navigator(cx),
            Part::Center => self.center_panel(cx),
            Part::SidebarQueue => self.playing_tracks(cx),
            Part::InfoHeader => self.track_information_header(cx),
            Part::InfoBody => self.track_information_body(window, cx),
            Part::Status => self.status_bar(cx),
            Part::Player => self.player_bar(window, cx),
            Part::NowBackdrop => self.now_playing_backdrop(window, cx),
            Part::NowDetails => self.now_playing_details(window, cx),
            Part::NowLyrics => self.now_playing_lyrics(window, cx),
            Part::Spectrum => self.now_playing_spectrum(window, cx),
        };
        // Covers still settling redraw this part, not the whole window.
        if std::mem::take(&mut self.art_wake) {
            self.request_frame_after(ART_SETTLE, window, cx);
        }
        element
    }
}
