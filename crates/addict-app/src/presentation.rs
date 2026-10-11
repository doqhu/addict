//! The instant a frame of the window reached the screen, from the
//! compositor's `wp_presentation` feedback.
//!
//! Used only with `--evidence`. `wp_presentation` is bound on GDK's own
//! Wayland connection through a second event queue: GDK keeps reading the
//! socket, and this queue is dispatched only while a frame is outstanding.
//!
//! [`Watch::arm`] names an activation; the feedback is requested for the next
//! commit of the window and its outcome is delivered once, as a time with its
//! clock id or as a limitation. No other source of time stands in for a
//! missing one.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gdk4_wayland::prelude::*;
use gtk::{gdk, glib, prelude::*};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry::{self, WlRegistry},
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, WpPresentationFeedback},
};

/// How long an armed activation waits, first for a frame and then for its
/// feedback.
const LIMIT: Duration = Duration::from_secs(3);

/// How often the feedback queue is looked at while a frame is outstanding.
const POLL: Duration = Duration::from_millis(2);

/// What became of the frame of one armed activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presented {
    /// The `wp_presentation` time of the frame, in nanoseconds of `clock_id`.
    pub time_ns: Option<u64>,
    pub clock_id: Option<i32>,
    /// Why there is no time.
    pub limitation: Option<String>,
}

impl Presented {
    fn missing(limitation: impl Into<String>) -> Self {
        Self {
            time_ns: None,
            clock_id: None,
            limitation: Some(limitation.into()),
        }
    }

    fn from_feedback(time_ns: Option<u64>, clock_id: Option<i32>) -> Self {
        match time_ns {
            Some(_) => Self {
                time_ns,
                clock_id,
                limitation: None,
            },
            None => Self::missing("wp_presentation frame discarded"),
        }
    }
}

#[derive(Default)]
struct Feedback {
    clock: Option<i32>,
    /// Activation id and the time of its frame; no time for a discarded one.
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

/// `wp_presentation` on GDK's connection, with its own event queue.
struct Direct {
    queue: EventQueue<Feedback>,
    state: Feedback,
    presentation: WpPresentation,
}

impl Direct {
    /// The error is the limitation every presentation of the session carries.
    fn connect(display: &gdk::Display) -> Result<Self, String> {
        let display = display
            .downcast_ref::<gdk4_wayland::WaylandDisplay>()
            .ok_or("wp_presentation unavailable: the display is not Wayland")?;
        let backend = display
            .wl_display()
            .and_then(|display| display.backend().upgrade())
            .ok_or("wp_presentation unavailable: no Wayland connection")?;
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<Feedback>(&connection)
            .map_err(|e| format!("wp_presentation unavailable: {e}"))?;
        let presentation = globals
            .bind(&queue.handle(), 1..=1, ())
            .map_err(|e| format!("wp_presentation unavailable: {e}"))?;
        let mut state = Feedback::default();
        // Brings the clock id.
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("wp_presentation unavailable: {e}"))?;
        Ok(Self {
            queue,
            state,
            presentation,
        })
    }

    /// Feedback belongs to the next commit of the surface, so this must run
    /// before the paint phase of the frame clock submits the buffer.
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

    fn take(&mut self, id: &str) -> Option<Presented> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let index = self.state.done.iter().position(|(done, _)| done == id)?;
        let (_, time) = self.state.done.swap_remove(index);
        Some(Presented::from_feedback(time, self.state.clock))
    }
}

/// An activation whose feedback was requested.
struct Outstanding {
    id: String,
    requested: Instant,
}

/// Gets the id of an activation and the outcome of its frame.
type Deliver = dyn Fn(&str, Presented);

struct State {
    direct: Result<Direct, String>,
    /// Armed, waiting for the next frame of the window.
    pending: Option<String>,
    outstanding: Vec<Outstanding>,
    /// Whether the poll timer is running.
    polling: bool,
    deliver: Rc<Deliver>,
}

/// Reports the presentation of the frame that follows each armed activation.
pub struct Watch {
    state: Rc<RefCell<State>>,
}

impl Watch {
    /// `deliver` gets the id given to [`Watch::arm`] and the outcome, once per
    /// armed activation. It may run inside `arm`.
    pub fn new(
        window: &gtk::ApplicationWindow,
        deliver: impl Fn(&str, Presented) + 'static,
    ) -> Self {
        let state = Rc::new(RefCell::new(State {
            direct: Direct::connect(&WidgetExt::display(window)),
            pending: None,
            outstanding: Vec::new(),
            polling: false,
            deliver: Rc::new(deliver),
        }));
        // The window is realized by its first `present()`, after the first
        // `arm`: the clock exists before the first frame is painted.
        let realized = Rc::downgrade(&state);
        window.connect_realize(move |window| {
            let Some(clock) = window.frame_clock() else {
                return;
            };
            let state = realized.clone();
            let window = window.downgrade();
            // `before-paint` precedes the buffer commit of the frame.
            clock.connect_before_paint(move |_| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let Some(id) = state.borrow_mut().pending.take() else {
                    return;
                };
                let surface = window.upgrade().and_then(|window| window.surface());
                let requested = match &state.borrow().direct {
                    Ok(direct) => direct.request(surface, &id),
                    Err(_) => false,
                };
                if requested {
                    state.borrow_mut().outstanding.push(Outstanding {
                        id,
                        requested: Instant::now(),
                    });
                    start_poll(&state);
                } else {
                    let missing = Presented::missing("the window has no Wayland surface");
                    report(&state, &id, missing);
                }
            });
        });
        Self { state }
    }

    /// Call before the `present()` of the activation `id`: the next frame the
    /// window commits is the one that is reported.
    pub fn arm(&self, id: String) {
        let unavailable = self.state.borrow().direct.as_ref().err().cloned();
        if let Some(reason) = unavailable {
            report(&self.state, &id, Presented::missing(reason));
            return;
        }
        let superseded = self.state.borrow_mut().pending.replace(id.clone());
        if let Some(earlier) = superseded {
            let missing = Presented::missing("no frame was committed before the next activation");
            report(&self.state, &earlier, missing);
        }
        // One shot, not a poll: a window that paints no frame, as under a
        // locked session, still ends the wait.
        let state = Rc::downgrade(&self.state);
        glib::timeout_add_local_once(LIMIT, move || {
            let Some(state) = state.upgrade() else {
                return;
            };
            let waiting = state.borrow().pending.as_deref() == Some(id.as_str());
            if waiting {
                state.borrow_mut().pending = None;
                report(
                    &state,
                    &id,
                    Presented::missing("no frame committed within 3 s"),
                );
            }
        });
    }
}

/// Delivers with no borrow held: the receiver may arm again.
fn report(state: &Rc<RefCell<State>>, id: &str, presented: Presented) {
    let deliver = state.borrow().deliver.clone();
    deliver(id, presented);
}

/// Runs the poll timer while feedback is outstanding; an idle or hidden
/// window keeps no timer.
fn start_poll(state: &Rc<RefCell<State>>) {
    if state.borrow().polling {
        return;
    }
    state.borrow_mut().polling = true;
    let state = Rc::downgrade(state);
    glib::timeout_add_local(POLL, move || {
        let Some(state) = state.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let mut finished = Vec::new();
        {
            let mut state = state.borrow_mut();
            let State {
                direct,
                outstanding,
                ..
            } = &mut *state;
            outstanding.retain(|frame| {
                let feedback = direct
                    .as_mut()
                    .ok()
                    .and_then(|direct| direct.take(&frame.id));
                let presented = match feedback {
                    Some(presented) => presented,
                    None if frame.requested.elapsed() > LIMIT => {
                        Presented::missing("wp_presentation feedback missing after 3 s")
                    }
                    None => return true,
                };
                finished.push((frame.id.clone(), presented));
                false
            });
        }
        for (id, presented) in finished {
            report(&state, &id, presented);
        }
        // Checked after the reports: one of them may have armed a new frame.
        let mut state = state.borrow_mut();
        if state.outstanding.is_empty() {
            state.polling = false;
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
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

    #[test]
    fn a_discarded_frame_has_no_time_and_says_why() {
        assert_eq!(
            Presented::from_feedback(Some(5), Some(1)),
            Presented {
                time_ns: Some(5),
                clock_id: Some(1),
                limitation: None
            }
        );
        let discarded = Presented::from_feedback(None, Some(1));
        assert_eq!(discarded.time_ns, None);
        assert_eq!(discarded.clock_id, None);
        assert_eq!(
            discarded.limitation.as_deref(),
            Some("wp_presentation frame discarded")
        );
    }
}
