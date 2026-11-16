use super::{ARTIST_TARGET, Fixture};
use gpui_kit::{
    AnyWindowHandle, AppContext, ElementId, SharedString, TestAppContext, test::TestWindowExt,
};
use sonora::platform::shortcut;

impl Fixture {
    pub(crate) fn click(
        &mut self,
        window: AnyWindowHandle,
        id: impl Into<ElementId>,
        cx: &mut TestAppContext,
    ) {
        cx.update_window(window, |_, window, cx| window.click(id, cx))
            .unwrap();
        self.settle(window, cx);
    }

    pub(crate) fn press(&mut self, window: AnyWindowHandle, key: &str, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        self.settle(window, cx);
    }

    pub(crate) fn open_album(&mut self, window: AnyWindowHandle, cx: &mut TestAppContext) {
        self.click(window, "new-tab", cx);
        self.click(window, SharedString::from(ARTIST_TARGET), cx);
        self.click(window, "artist-release-album-1", cx);
    }

    pub(crate) fn search(&mut self, window: AnyWindowHandle, text: &str, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, cx| {
            window.click("music-search", cx);
            window.press(shortcut("cmd-a", "ctrl-a"), cx);
            window.press("backspace", cx);
            window.input(text, cx);
        })
        .unwrap();
        self.settle(window, cx);
    }

    pub(crate) fn menu_item(
        &mut self,
        window: AnyWindowHandle,
        label: &str,
        cx: &mut TestAppContext,
    ) {
        cx.update_window(window, |_, window, cx| {
            let mut menu = window.within("popup-menu");
            let target = menu.find_by_label(label);
            let id = target.path().last().unwrap().clone();
            menu.click(id, cx);
        })
        .unwrap();
        self.settle(window, cx);
    }
}
