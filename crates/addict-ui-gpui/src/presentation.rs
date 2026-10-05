use addict_exp4::{
    metrics,
    protocol::{Emitter, FrontendEvent, Invocation},
};
use gpui::Window;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// Only wp_presentation feedback can produce an actual display timestamp.
pub fn arm(window: &Window, invocation: &Invocation, emitter: Emitter) -> bool {
    let done = Arc::new(AtomicBool::new(false));
    let feedback_done = done.clone();
    let id = invocation.id.clone();
    let timeout_id = id.clone();
    let timeout_emitter = emitter.clone();
    let token = window.exp4_activate(
        invocation.activation_token.as_deref(),
        move |time_ns, clock_id| {
            if feedback_done.swap(true, Ordering::Relaxed) {
                return;
            }
            emitter.send(FrontendEvent::Presented {
                id,
                time_ns,
                clock_id,
                usable: true,
                diagnostic_draw_ns: metrics::monotonic_ns(),
                limitation: time_ns
                    .is_none()
                    .then(|| "wp_presentation unavailable or frame discarded".into()),
            });
        },
    );
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        if !done.swap(true, Ordering::Relaxed) {
            timeout_emitter.send(FrontendEvent::Presented {
                id: timeout_id,
                time_ns: None,
                clock_id: None,
                usable: true,
                diagnostic_draw_ns: metrics::monotonic_ns(),
                limitation: Some("wp_presentation feedback missing after 3 s".into()),
            });
        }
    });
    token
}
