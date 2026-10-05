mod input;
mod presentation;
use addict_exp4::{
    FIXTURE_INPUT, FIXTURE_RESULT,
    protocol::{Backend, Client, Command, Emitter, FrontendEvent, Invocation, Mode},
    work::Workers,
};
use clap::Parser;
use futures::StreamExt;
use gpui::{
    App, Bounds, Context, Entity, Focusable, KeyBinding, MouseButton, QuitMode, Window,
    WindowBounds, WindowHandle, WindowOptions, WindowingModes, actions, div, prelude::*, px, rgb,
    size,
};
use input::TextInput;
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};
actions!(exp4, [Close, Quit, Delay, Ocr]);
#[derive(Parser)]
struct Args {
    #[arg(long)]
    socket: PathBuf,
    #[arg(long, value_enum)]
    mode: Mode,
    #[arg(long)]
    fixture: PathBuf,
}
struct Card {
    input: Entity<TextInput>,
    result: Entity<TextInput>,
    status: String,
    id: String,
    emitter: Emitter,
    workers: Workers,
    fixture: PathBuf,
}
impl Card {
    fn close(&mut self, _: &Close, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }
    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        self.emitter.send(FrontendEvent::Quit);
        cx.quit();
    }
    fn delay(&mut self, _: &Delay, _: &mut Window, cx: &mut Context<Self>) {
        self.status = "Waiting • delayed work (5 s), editing remains available".into();
        self.workers.delayed(
            self.emitter.clone(),
            self.id.clone(),
            Duration::from_secs(5),
        );
        cx.notify();
    }
    fn ocr(&mut self, _: &Ocr, _: &mut Window, cx: &mut Context<Self>) {
        self.status = "Waiting • Tesseract (15 s deadline), editing remains available".into();
        self.workers.ocr(
            &self.fixture,
            Duration::from_secs(15),
            self.emitter.clone(),
            self.id.clone(),
        );
        cx.notify();
    }
}
impl Render for Card {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id, label| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .border_1()
                .border_color(rgb(0xaaaaaa))
                .rounded_md()
                .cursor_pointer()
                .child(label)
        };
        div()
            .size_full()
            .p_6()
            .bg(rgb(0xf7f7f7))
            .text_color(rgb(0x202020))
            .flex()
            .flex_col()
            .gap_3()
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::delay))
            .on_action(cx.listener(Self::ocr))
            .child("Source: manual fixture • English / Русский")
            .child(self.input.clone())
            .child(self.result.clone())
            .child(self.status.clone())
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(button("delay", "Delayed work (5 s)").on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.delay(&Delay, window, cx)),
                    ))
                    .child(button("ocr", "Run OCR").on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.ocr(&Ocr, window, cx)),
                    ))
                    .child(button("close", "Close / Esc").on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close(&Close, window, cx)),
                    ))
                    .child(button("quit", "Quit").on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.quit(&Quit, window, cx)),
                    )),
            )
    }
}
fn main() {
    let args = Args::parse();
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("Wayland session required");
        std::process::exit(1);
    }
    // IPC owns single-use activation tokens. Disable inherited startup consumption.
    unsafe {
        std::env::remove_var("XDG_ACTIVATION_TOKEN");
        std::env::remove_var("DESKTOP_STARTUP_ID");
    }
    let client = Client::connect(&args.socket, Backend::Gpui).unwrap_or_else(|e| {
        eprintln!("IPC: {e}");
        std::process::exit(1)
    });
    let emitter = client.emitter();
    let outcomes = emitter.subscribe();
    let workers = Workers::default();
    let ui_workers = workers.clone();
    // Linux GPUI quits with its last window by default; a resident frontend must outlive it.
    gpui_platform::linux(WindowingModes::WAYLAND)
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx: &mut App| {
            input::bindings(cx);
            cx.bind_keys([
                KeyBinding::new("escape", Close, None),
                KeyBinding::new("ctrl-q", Quit, None),
            ]);
            let text = cx.new(|cx| TextInput::new(cx, FIXTURE_INPUT));
            let result = cx.new(|cx| TextInput::readonly(cx, FIXTURE_RESULT));
            let current = Rc::new(RefCell::new(None::<Invocation>));
            let closed_current = current.clone();
            let close_emitter = emitter.clone();
            let mode = args.mode;
            cx.on_window_closed(move |cx, _| {
                if let Some(invocation) = closed_current.borrow_mut().take() {
                    close_emitter.send(FrontendEvent::Closed { id: invocation.id });
                }
                if mode == Mode::OnDemand {
                    cx.quit();
                }
            })
            .detach();
            let mut window = None::<WindowHandle<Card>>;
            let mut last_focus = None;
            emitter.send(FrontendEvent::Ready {
                pid: std::process::id(),
                backend: Backend::Gpui,
                version: 1,
            });
            // Without a window nothing needs polling: sleep until the IPC threads queue
            // something. Focus changes are still polled while a window is open.
            let (wake_sender, mut wake_receiver) = futures::channel::mpsc::unbounded();
            let _ = wake_sender.unbounded_send(());
            client.on_activity(move || {
                let _ = wake_sender.unbounded_send(());
            });
            cx.spawn(async move |cx| {
                loop {
                    if window.is_some_and(|w| w.update(cx, |_, _, _| {}).is_err()) {
                        window = None;
                    }
                    if window.is_some() {
                        cx.background_executor()
                            .timer(Duration::from_millis(2))
                            .await;
                    } else if wake_receiver.next().await.is_none() {
                        return;
                    }
                    while wake_receiver.try_recv().is_ok() {}
                    while let Ok(command) = client.commands.try_recv() {
                        match command {
                            Command::Quit => {
                                cx.update(|cx| cx.quit());
                                return;
                            }
                            Command::Invoke { invocation } => {
                                if window.is_some_and(|w| w.update(cx, |_, _, _| {}).is_err()) {
                                    window = None;
                                }
                                *current.borrow_mut() = Some(invocation.clone());
                                if window.is_none() {
                                    let bounds = cx.update(|cx| {
                                        Bounds::centered(None, size(px(640.0), px(360.0)), cx)
                                    });
                                    let text = text.clone();
                                    let result = result.clone();
                                    let emitter = emitter.clone();
                                    let workers = ui_workers.clone();
                                    let fixture = args.fixture.clone();
                                    let id = invocation.id.clone();
                                    window = cx
                                        .open_window(
                                            WindowOptions {
                                                window_bounds: Some(WindowBounds::Windowed(bounds)),
                                                app_id: Some(Backend::Gpui.app_id().into()),
                                                focus: false,
                                                ..Default::default()
                                            },
                                            move |_, cx| {
                                                cx.new(|_| Card {
                                                    input: text,
                                                    result,
                                                    status: "Ready • fixture only".into(),
                                                    id,
                                                    emitter,
                                                    workers,
                                                    fixture,
                                                })
                                            },
                                        )
                                        .ok();
                                }
                                if let Some(window) = window {
                                    let activate_emitter = emitter.clone();
                                    let _ = window.update(cx, |view, window, cx| {
                                        view.id = invocation.id.clone();
                                        view.status = "Ready • fixture only".into();
                                        let token_consumed = presentation::arm(
                                            window,
                                            &invocation,
                                            activate_emitter.clone(),
                                        );
                                        window.focus(&view.input.focus_handle(cx), cx);
                                        window.refresh();
                                        cx.notify();
                                        activate_emitter.send(FrontendEvent::Activated {
                                            id: invocation.id.clone(),
                                            token_consumed,
                                        });
                                    });
                                    last_focus = None;
                                } else {
                                    emitter.send(FrontendEvent::Error {
                                        id: Some(invocation.id),
                                        error: "window creation failed".into(),
                                    });
                                }
                            }
                        }
                    }
                    while let Ok(event) = outcomes.try_recv() {
                        if let FrontendEvent::Work { id, outcome, .. } = event
                            && let Some(window) = window
                        {
                            let _ = window.update(cx, |view, _, cx| {
                                if view.id == id {
                                    view.status = outcome;
                                    cx.notify();
                                }
                            });
                        }
                    }
                    if let (Some(window), Some(invocation)) = (window, current.borrow().as_ref())
                        && let Ok(focused) =
                            window.update(cx, |_, window, _| window.is_window_active())
                        && last_focus != Some(focused)
                    {
                        last_focus = Some(focused);
                        emitter.send(FrontendEvent::Focus {
                            id: invocation.id.clone(),
                            focused,
                        });
                    }
                }
            })
            .detach();
        });
    workers.shutdown();
}
