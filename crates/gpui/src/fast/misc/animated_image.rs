//! An animated image draws its view again when its next frame is due, rather
//! than on every frame the display shows.
//!
//! Upstream asks for an animation frame each time the image is laid out. A
//! GIF whose frames last 100 ms then has its view rendered, and the window
//! drawn and presented, 120 times a second on a 120 Hz display to show 10
//! different pictures.

use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

use crate::{App, Window};

/// Waits shorter than this ask for the next frame instead: a display frame is
/// about as long, and a timer would only add its own lateness.
const SHORTEST_WAIT: Duration = Duration::from_millis(17);

/// Whether an image is already waiting to draw its next frame.
#[derive(Default)]
pub(crate) struct FrameWake {
    pending: Rc<Cell<bool>>,
}

/// Has the view being drawn drawn again when the image's frame, shown since
/// `shown_at` for `delay`, gives way to the next. `wake` is the image's own,
/// so that laying it out again while it waits starts no second wait.
pub(crate) fn request_frame(
    wake: Option<&FrameWake>,
    shown_at: Option<Instant>,
    delay: Duration,
    window: &mut Window,
    cx: &mut App,
) {
    let due_in = shown_at
        .and_then(|shown_at| delay.checked_sub(shown_at.elapsed()))
        .filter(|due_in| *due_in > SHORTEST_WAIT);
    let (Some(wake), Some(due_in)) = (wake, due_in) else {
        window.request_animation_frame();
        return;
    };
    if wake.pending.replace(true) {
        return;
    }
    let pending = wake.pending.clone();
    let view = window.current_view();
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(due_in).await;
            pending.set(false);
            cx.update(move |_, cx| cx.notify(view)).ok();
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use image::{Delay, Frame, ImageBuffer, Rgba};
    use smallvec::SmallVec;

    use crate::{
        AppContext as _, Context, ImageSource, InteractiveElement as _, IntoElement, Render,
        RenderImage, TestAppContext, Window, WindowHandle, img,
    };

    struct Animated {
        image: Arc<RenderImage>,
        renders: usize,
    }

    impl Render for Animated {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders += 1;
            img(ImageSource::Render(self.image.clone())).id("animated")
        }
    }

    fn image(frame_ms: u32) -> Arc<RenderImage> {
        let frame = Frame::from_parts(
            ImageBuffer::from_pixel(1, 1, Rgba([0, 0, 0, 0])),
            0,
            0,
            Delay::from_numer_denom_ms(frame_ms, 1),
        );
        Arc::new(RenderImage::new(SmallVec::from_iter([
            frame.clone(),
            frame,
        ])))
    }

    fn open(cx: &mut TestAppContext, frame_ms: u32) -> WindowHandle<Animated> {
        let window = cx.add_window(|window, _| {
            // An image animates only in the active window.
            window.active.set(true);
            Animated {
                image: image(frame_ms),
                renders: 0,
            }
        });
        draw(cx, window);
        window
    }

    /// Shows the next display frame, returning how many times the view has
    /// rendered.
    fn draw(cx: &mut TestAppContext, window: WindowHandle<Animated>) -> usize {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        window.update(cx, |view, _, _| view.renders).unwrap()
    }

    #[crate::test]
    fn a_slow_frame_is_waited_for_rather_than_polled(cx: &mut TestAppContext) {
        let window = open(cx, 10_000);
        let before = draw(cx, window);
        for _ in 0..5 {
            draw(cx, window);
        }
        assert_eq!(draw(cx, window), before);

        cx.executor().advance_clock(Duration::from_secs(11));
        assert_eq!(draw(cx, window), before + 1, "drawn again when it is due");
    }

    #[crate::test]
    fn a_frame_shorter_than_a_display_frame_is_drawn_every_frame(cx: &mut TestAppContext) {
        let window = open(cx, 1);
        let before = draw(cx, window);
        for _ in 0..4 {
            draw(cx, window);
        }
        assert_eq!(draw(cx, window), before + 5);
    }
}
