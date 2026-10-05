use addict_exp4::{
    metrics,
    protocol::{Emitter, FrontendEvent},
};
use gdk4_wayland::prelude::*;
use gtk::{gdk, glib, prelude::*};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry::{self, WlRegistry},
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};

#[derive(Default)]
struct Feedback {
    clock: Option<i32>,
    done: Vec<(String, Option<u64>)>,
}
impl Dispatch<WlRegistry, GlobalListContents> for Feedback {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WpPresentation, ()> for Feedback {
    fn event(
        state: &mut Self,
        _: &WpPresentation,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.clock = i32::try_from(clk_id).ok();
        }
    }
}
impl Dispatch<WpPresentationFeedback, String> for Feedback {
    fn event(
        state: &mut Self,
        _: &WpPresentationFeedback,
        event: wp_presentation_feedback::Event,
        id: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                ..
            } => state.done.push((
                id.clone(),
                Some(presented_ns(tv_sec_hi, tv_sec_lo, tv_nsec)),
            )),
            wp_presentation_feedback::Event::Discarded => state.done.push((id.clone(), None)),
            _ => {}
        }
    }
}
fn presented_ns(tv_sec_hi: u32, tv_sec_lo: u32, tv_nsec: u32) -> u64 {
    ((u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo)) * 1_000_000_000 + u64::from(tv_nsec)
}

/// wp_presentation bound on GDK's own connection through a separate event
/// queue. GDK keeps reading the socket; this queue is only dispatched here.
struct Direct {
    queue: EventQueue<Feedback>,
    state: Feedback,
    presentation: WpPresentation,
}
impl Direct {
    fn connect(display: &gdk::Display) -> Option<Self> {
        let display = display.downcast_ref::<gdk4_wayland::WaylandDisplay>()?;
        let connection = Connection::from_backend(display.wl_display()?.backend().upgrade()?);
        let (globals, mut queue) = registry_queue_init::<Feedback>(&connection).ok()?;
        let presentation = globals.bind(&queue.handle(), 1..=1, ()).ok()?;
        let mut state = Feedback::default();
        queue.roundtrip(&mut state).ok()?;
        Some(Self {
            queue,
            state,
            presentation,
        })
    }
    /// Feedback belongs to the next commit of the surface, so this must run
    /// before the frame clock's paint phase submits the buffer.
    fn request(&self, surface: Option<gdk::Surface>, id: &str) -> bool {
        let Some(surface) = surface
            .and_then(|surface| surface.downcast::<gdk4_wayland::WaylandSurface>().ok())
            .and_then(|surface| surface.wl_surface())
        else {
            return false;
        };
        self.presentation
            .feedback(&surface, &self.queue.handle(), id.to_owned());
        true
    }
    fn take(&mut self, id: &str) -> Option<(Option<u64>, Option<i32>)> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let index = self.state.done.iter().position(|(done, _)| done == id)?;
        let (_, time) = self.state.done.swap_remove(index);
        Some((time, self.state.clock))
    }
}

struct Tracked {
    id: String,
    counter: i64,
    start: Instant,
    direct: bool,
}

/// `before-paint` precedes the buffer commit of the requested frame, so real
/// wp_presentation feedback is armed there. Completed GDK FrameTimings on
/// GTK 4.22.5 Wayland can contain heuristic display times, so they remain a
/// diagnostic fallback when direct feedback cannot be requested.
pub fn watch(
    window: &gtk::ApplicationWindow,
    pending: Rc<RefCell<Option<String>>>,
    emitter: Emitter,
) {
    let weak = window.downgrade();
    let direct = Rc::new(RefCell::new(Direct::connect(&WidgetExt::display(window))));
    let tracked = Rc::new(RefCell::new(None::<Tracked>));
    let poll_weak = weak.clone();
    let poll_tracked = tracked.clone();
    let poll_direct = direct.clone();
    // Feedback is polled only while an invocation's frame is outstanding; an idle or
    // hidden window keeps no timer.
    let poll: Rc<dyn Fn()> = Rc::new(move || {
        let weak = poll_weak.clone();
        let tracked = poll_tracked.clone();
        let direct = poll_direct.clone();
        let emitter = emitter.clone();
        glib::timeout_add_local(Duration::from_millis(2), move || {
            let Some(window) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let mut state = tracked.borrow_mut();
            let Some(tracked) = state.as_ref() else {
                return glib::ControlFlow::Break;
            };
            let presented = |time_ns, clock_id, limitation| FrontendEvent::Presented {
                id: tracked.id.clone(),
                time_ns,
                clock_id,
                usable: true,
                diagnostic_draw_ns: metrics::monotonic_ns(),
                limitation,
            };
            if tracked.direct {
                let feedback = direct
                    .borrow_mut()
                    .as_mut()
                    .and_then(|direct| direct.take(&tracked.id));
                if let Some((time, clock)) = feedback {
                    let limitation = time
                        .is_none()
                        .then(|| "wp_presentation frame discarded".into());
                    emitter.send(presented(time, clock, limitation));
                    *state = None;
                } else if tracked.start.elapsed() > Duration::from_secs(3) {
                    let limitation = Some("wp_presentation feedback missing after 3 s".into());
                    emitter.send(presented(None, None, limitation));
                    *state = None;
                }
                return glib::ControlFlow::Continue;
            }
            let timing = window
                .frame_clock()
                .and_then(|clock| clock.timings(tracked.counter));
            if let Some(timing) = timing
                && timing.is_complete()
            {
                let time = timing.presentation_time();
                // GTK 4.22.5 Wayland synthesizes presentation_time from frame time
                // and refresh interval, and marks those timings complete. Positive
                // or past values therefore cannot prove actual presentation.
                let limitation = Some(format!(
                    "GDK completed FrameTimings is diagnostic: GTK 4.22.5 Wayland may synthesize display time; raw presentation_time_us={time}; direct presentation feedback unavailable"
                ));
                emitter.send(presented(None, None, limitation));
                *state = None;
            } else if tracked.start.elapsed() > Duration::from_secs(3) {
                let limitation = Some("GDK frame timings did not complete within 3 s".into());
                emitter.send(presented(None, None, limitation));
                *state = None;
            }
            glib::ControlFlow::Continue
        });
    });
    let before = tracked.clone();
    let pending_before = pending.clone();
    let direct_before = direct.clone();
    window.connect_realize(move |window| {
        if let Some(clock) = window.frame_clock() {
            let before = before.clone();
            let pending = pending_before.clone();
            let direct = direct_before.clone();
            let weak = window.downgrade();
            let poll = poll.clone();
            clock.connect_before_paint(move |clock| {
                if before.borrow().is_none()
                    && let Some(id) = pending.borrow_mut().take()
                {
                    let surface = weak.upgrade().and_then(|window| window.surface());
                    let direct = direct
                        .borrow()
                        .as_ref()
                        .is_some_and(|direct| direct.request(surface, &id));
                    *before.borrow_mut() = Some(Tracked {
                        id,
                        counter: clock.frame_counter(),
                        start: Instant::now(),
                        direct,
                    });
                    poll();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presented_time_joins_split_seconds() {
        assert_eq!(presented_ns(0, 131_863, 795_439_000), 131_863_795_439_000);
        assert_eq!(presented_ns(1, 0, 1), (1u64 << 32) * 1_000_000_000 + 1);
    }
}
