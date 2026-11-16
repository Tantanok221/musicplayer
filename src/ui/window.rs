use super::*;
use crate::platform::WindowFrame;

/// The saved window frame, if it still lands on a connected display.
pub fn restored_window_bounds(frame: Option<WindowFrame>, cx: &App) -> Option<WindowBounds> {
    let frame = frame?;
    let bounds = Bounds::new(
        point(px(frame.x), px(frame.y)),
        size(px(frame.width.max(680.)), px(frame.height.max(540.))),
    );
    // A display that was unplugged would leave the window out of reach.
    let visible = cx.displays().iter().any(|display| {
        let area = display.bounds().intersect(&bounds);
        f32::from(area.size.width) >= 100. && f32::from(area.size.height) >= 100.
    });
    visible.then_some(if frame.fullscreen {
        WindowBounds::Fullscreen(bounds)
    } else if frame.maximized {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    })
}
