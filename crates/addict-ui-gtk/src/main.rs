mod presentation;
use addict_exp4::{
    FIXTURE_INPUT, FIXTURE_RESULT,
    protocol::{Backend, Client, Command, FrontendEvent, Mode},
    work::Workers,
};
use clap::Parser;
use futures_util::StreamExt;
use gtk::{gdk, glib, prelude::*};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: PathBuf,
    #[arg(long, value_enum)]
    mode: Mode,
    #[arg(long)]
    fixture: PathBuf,
}
fn main() {
    let args = Args::parse();
    // Force the comparison onto normal Wayland windows; reject X11 fallback.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("Wayland session required");
        std::process::exit(1);
    }
    // Set before GTK or any worker thread exists; no process-wide environment race.
    unsafe {
        std::env::set_var("GDK_BACKEND", "wayland");
        std::env::remove_var("XDG_ACTIVATION_TOKEN");
    }
    let client = Client::connect(&args.socket, Backend::Gtk4).unwrap_or_else(|e| {
        eprintln!("IPC: {e}");
        std::process::exit(1)
    });
    let emitter = client.emitter();
    let application = gtk::Application::builder()
        .application_id(Backend::Gtk4.app_id())
        .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let workers = Workers::default();
    let ui_workers = workers.clone();
    let pending = Rc::new(RefCell::new(None::<String>));
    let current = Rc::new(RefCell::new(None::<String>));
    let client = Rc::new(client);
    application.connect_activate(move |app| {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Addict EXP-4 GTK4")
            .default_width(640)
            .default_height(360)
            .build();
        let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
        card.set_margin_top(24);
        card.set_margin_bottom(24);
        card.set_margin_start(24);
        card.set_margin_end(24);
        let source = gtk::Label::new(Some("Source: manual fixture • English / Русский"));
        source.set_xalign(0.0);
        let input = gtk::TextView::new();
        input.set_wrap_mode(gtk::WrapMode::WordChar);
        input.buffer().set_text(FIXTURE_INPUT);
        input.set_vexpand(true);
        let result = gtk::Label::new(Some(FIXTURE_RESULT));
        result.set_selectable(true);
        result.set_wrap(true);
        result.set_xalign(0.0);
        let status = gtk::Label::new(Some("Ready • fixture only"));
        status.set_xalign(0.0);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let delay = gtk::Button::with_label("Delayed work (5 s)");
        let ocr = gtk::Button::with_label("Run OCR");
        let close = gtk::Button::with_label("Close / Esc");
        let quit = gtk::Button::with_label("Quit");
        for button in [&delay, &ocr, &close, &quit] {
            actions.append(button);
        }
        for widget in [
            source.upcast_ref::<gtk::Widget>(),
            input.upcast_ref(),
            result.upcast_ref(),
            status.upcast_ref(),
            actions.upcast_ref(),
        ] {
            card.append(widget);
        }
        window.set_child(Some(&card));
        presentation::watch(&window, pending.clone(), emitter.clone());
        let focus_id = current.clone();
        let focus_emit = emitter.clone();
        window.connect_is_active_notify(move |window| {
            if let Some(id) = focus_id.borrow().clone() {
                focus_emit.send(FrontendEvent::Focus {
                    id,
                    focused: window.is_active(),
                });
            }
        });
        let close_id = current.clone();
        let close_emit = emitter.clone();
        let mode = args.mode;
        window.connect_close_request(move |window| {
            if let Some(id) = close_id.borrow_mut().take() {
                close_emit.send(FrontendEvent::Closed { id });
            }
            if mode == Mode::Resident {
                window.set_visible(false);
                glib::Propagation::Stop
            } else {
                if let Some(app) = window.application() {
                    app.quit();
                }
                glib::Propagation::Proceed
            }
        });
        let key = gtk::EventControllerKey::new();
        let key_window = window.downgrade();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(window) = key_window.upgrade() {
                    window.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
        let close_window = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = close_window.upgrade() {
                window.close();
            }
        });
        let quit_app = app.downgrade();
        let quit_emitter = emitter.clone();
        quit.connect_clicked(move |_| {
            quit_emitter.send(FrontendEvent::Quit);
            if let Some(app) = quit_app.upgrade() {
                app.quit();
            }
        });
        let delay_workers = ui_workers.clone();
        let delay_current = current.clone();
        let delay_emit = emitter.clone();
        let delay_status = status.clone();
        delay.connect_clicked(move |_| {
            if let Some(id) = delay_current.borrow().clone() {
                delay_status.set_text("Waiting • delayed work (5 s), editing remains available");
                delay_workers.delayed(delay_emit.clone(), id, Duration::from_secs(5));
            }
        });
        let ocr_workers = ui_workers.clone();
        let ocr_current = current.clone();
        let ocr_emit = emitter.clone();
        let fixture = args.fixture.clone();
        let ocr_status = status.clone();
        ocr.connect_clicked(move |_| {
            if let Some(id) = ocr_current.borrow().clone() {
                ocr_status
                    .set_text("Waiting • Tesseract (15 s deadline), editing remains available");
                ocr_workers.ocr(&fixture, Duration::from_secs(15), ocr_emit.clone(), id);
            }
        });
        // Worker results are also sent to the broker, while a local queue drives the visible outcome.
        let local_current = current.clone();
        let local_pending = pending.clone();
        let local_emit = emitter.clone();
        let client = client.clone();
        let poll_window = window.downgrade();
        let poll_app = app.downgrade();
        let outcomes = emitter.subscribe();
        // The IPC threads signal each queued item; the main loop sleeps in between.
        let (wake_sender, mut wake_receiver) = futures_channel::mpsc::unbounded();
        let _ = wake_sender.unbounded_send(());
        client.on_activity(move || {
            let _ = wake_sender.unbounded_send(());
        });
        let drain = move || {
            let Some(window) = poll_window.upgrade() else {
                return glib::ControlFlow::Break;
            };
            while let Ok(event) = outcomes.try_recv() {
                if let FrontendEvent::Work { id, outcome, .. } = event
                    && local_current.borrow().as_ref() == Some(&id)
                {
                    status.set_text(&outcome);
                }
            }
            while let Ok(command) = client.commands.try_recv() {
                match command {
                    Command::Invoke { invocation } => {
                        let token_consumed =
                            if let Some(token) = invocation.activation_token.as_deref() {
                                window.set_startup_id(token);
                                true
                            } else {
                                false
                            };
                        *local_current.borrow_mut() = Some(invocation.id.clone());
                        *local_pending.borrow_mut() = Some(invocation.id.clone());
                        status.set_text("Ready • fixture only");
                        window.present();
                        input.grab_focus();
                        window.queue_draw();
                        local_emit.send(FrontendEvent::Activated {
                            id: invocation.id,
                            token_consumed,
                        });
                    }
                    Command::Quit => {
                        if let Some(app) = poll_app.upgrade() {
                            app.quit();
                        }
                        return glib::ControlFlow::Break;
                    }
                }
            }
            glib::ControlFlow::Continue
        };
        glib::spawn_future_local(async move {
            while wake_receiver.next().await.is_some() {
                while wake_receiver.try_recv().is_ok() {}
                if drain() == glib::ControlFlow::Break {
                    break;
                }
            }
        });
        emitter.send(FrontendEvent::Ready {
            pid: std::process::id(),
            backend: Backend::Gtk4,
            version: 1,
        });
    });
    application.run_with_args::<&str>(&[]);
    workers.shutdown();
}
