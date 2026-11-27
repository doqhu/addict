//! GTK widgets for the download offer.

use std::cell::{Cell, RefCell};

use addict_core::translation::Source as Language;
use addict_dictionary::release::Published;
use addict_engine::view::{first_run::*, settings::my_language_names};
use gtk::{glib, prelude::*};

use crate::settings::label;

struct Row {
    name: String,
    /// The whole line, tinted while its source runs.
    line: gtk::Box,
    /// The title of its source, or why it failed.
    sub: gtk::Label,
    /// The check of an installed source.
    icon: gtk::Image,
    /// The progress of a running source.
    bar: gtk::ProgressBar,
    status: gtk::Label,
    cancel: gtk::Button,
}

/// A vertical box with `classes`.
fn column(spacing: i32, classes: &[&str]) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(spacing)
        .css_classes(classes.to_vec())
        .build()
}

/// A horizontal box with `classes`.
fn line(spacing: i32, classes: &[&str]) -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(spacing)
        .css_classes(classes.to_vec())
        .build()
}

/// A label on one line.
fn single(text: &str, classes: &[&str]) -> gtk::Label {
    let single = label(text, classes);
    single.set_wrap(false);
    single
}

/// A round mark holding a check, `classes` giving its size and colours.
fn check(classes: &[&str], pixels: i32) -> gtk::Box {
    let mark = line(0, classes);
    mark.set_halign(gtk::Align::Center);
    mark.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name("object-select-symbolic");
    image.set_pixel_size(pixels);
    image.set_hexpand(true);
    image.set_halign(gtk::Align::Center);
    mark.append(&image);
    mark
}

/// The card of the three figures of the offer.
fn figures_card(indexes: &[Published]) -> gtk::Box {
    let card = line(0, &["first-run-card", "first-run-figures"]);
    card.set_homogeneous(true);
    for (i, (value, caption)) in figures(indexes).into_iter().enumerate() {
        let cell = column(0, &["figure"]);
        if i > 0 {
            cell.add_css_class("divided");
        }
        cell.append(&single(&value, &["figure-value"]));
        cell.append(&single(caption, &["hint"]));
        card.append(&cell);
    }
    card
}

/// The card of a group once every source is installed.
fn group_card(group: &Group) -> gtk::Box {
    let card = line(12, &["first-run-card", "group-card"]);
    let text = column(1, &[]);
    text.set_hexpand(true);
    text.append(&single(group.name, &["group-name"]));
    text.append(&single(&group_text(&group.indexes), &["hint"]));
    card.append(&text);
    let mark = check(&["group-check"], 14);
    mark.update_property(&[gtk::accessible::Property::Label(INSTALLED)]);
    card.append(&mark);
    card
}

/// The row of `published`, whose Cancel calls `on_cancel` with its name.
fn source_row(published: &Published, on_cancel: &std::rc::Rc<dyn Fn(Option<String>)>) -> Row {
    let line_box = line(12, &["first-run-row"]);
    if let Some(credit) = row_credit_text(&published.name) {
        line_box.set_tooltip_text(Some(&credit));
    }
    let text = column(1, &[]);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);
    text.append(&single(&published.name, &["row-name"]));
    let title = describe(&published.name).map_or("", |(_, title)| title);
    let sub = single(title, &["row-sub"]);
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.set_visible(!title.is_empty());
    text.append(&sub);
    line_box.append(&text);

    let icon = gtk::Image::from_icon_name("object-select-symbolic");
    icon.add_css_class("row-check");
    icon.set_visible(false);
    let bar = gtk::ProgressBar::builder()
        .css_classes(["row-bar"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let status = single(&offered_text(published), &["row-status"]);
    status.set_valign(gtk::Align::Center);
    let cancel = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .css_classes(["row-cancel"])
        .tooltip_text(format!("{CANCEL} {}", published.name))
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    cancel.update_property(&[gtk::accessible::Property::Label(&format!(
        "{CANCEL} {}",
        published.name
    ))]);
    let pressed = on_cancel.clone();
    let which = published.name.clone();
    cancel.connect_clicked(move |_| pressed(Some(which.clone())));
    // Cancel keeps its place on every row, so that the states line up.
    let slot = line(0, &["cancel-slot"]);
    slot.set_valign(gtk::Align::Center);
    slot.append(&cancel);
    for widget in [
        icon.upcast_ref::<gtk::Widget>(),
        bar.upcast_ref(),
        status.upcast_ref(),
        slot.upcast_ref(),
    ] {
        line_box.append(widget);
    }
    Row {
        name: published.name.clone(),
        line: line_box,
        sub,
        icon,
        bar,
        status,
        cancel,
    }
}

/// Selects `language` in `dropdown` while `rendering` is set, so that its
/// handler takes it for no choice.
fn render_language(dropdown: &gtk::DropDown, rendering: &Cell<bool>, language: Language) {
    let Some(index) = Language::ALL.iter().position(|one| *one == language) else {
        return;
    };
    rendering.set(true);
    dropdown.set_selected(index as u32);
    rendering.set(false);
}

/// The dialog of the offer.
pub struct Offer {
    window: gtk::Window,
    recovery: bool,
    hero: gtk::Box,
    done_mark: gtk::Box,
    eyebrow: gtk::Label,
    heading: gtk::Label,
    lead: gtk::Label,
    /// The my-language choice of the first-run offer, and the flag that
    /// keeps its rendering from being taken for a choice.
    language: gtk::Box,
    my_language: gtk::DropDown,
    rendering: std::rc::Rc<Cell<bool>>,
    /// Holds the card of the figures.
    figures: gtk::Box,
    progress: gtk::Box,
    current: gtk::Label,
    bytes: gtk::Label,
    bar: gtk::ProgressBar,
    counts: gtk::Label,
    list: gtk::ScrolledWindow,
    list_box: gtk::Box,
    cards: gtk::Grid,
    rows: RefCell<Vec<Row>>,
    on_cancel: std::rc::Rc<dyn Fn(Option<String>)>,
    note: gtk::Label,
    credit: gtk::Label,
    footer: gtk::Box,
    download: gtk::Button,
    not_now: gtk::Button,
    cancel: gtk::Button,
    close: gtk::Button,
}

impl Offer {
    /// Builds the dialog, hidden, above `parent`, with a row for each of
    /// `indexes`, grouped by language. `on_download` and `on_not_now` run at
    /// a press of their buttons; Not now also closes the dialog. `on_cancel`
    /// runs at a press of a row's Cancel, with its name, and of the
    /// dialog's, with `None`. Esc and the title bar close it and choose
    /// nothing. With `my`, the dialog shows the my-language choice at that
    /// language, and `on_my_language` runs with the language the user
    /// chooses in it; the caller lists its indexes ([`Offer::list`]).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent: &impl IsA<gtk::Window>,
        indexes: &[Published],
        recovery: bool,
        my: Option<Language>,
        on_my_language: impl Fn(Language) + 'static,
        on_download: impl Fn() + 'static,
        on_not_now: impl Fn() + 'static,
        on_cancel: impl Fn(Option<String>) + 'static,
    ) -> Self {
        let on_cancel: std::rc::Rc<dyn Fn(Option<String>)> = std::rc::Rc::new(on_cancel);

        let done_mark = check(&["done-mark"], 26);
        done_mark.set_visible(false);
        let eyebrow = single(
            &eyebrow_text(recovery, indexes.len()),
            &["section-label", "eyebrow", "warn"],
        );
        let heading = label(
            if recovery { RECOVERY_HEADING } else { HEADING },
            &["first-run-heading"],
        );
        let lead = label(BODY, &["first-run-lead"]);

        let my_language = gtk::DropDown::from_strings(&my_language_names());
        my_language.set_valign(gtk::Align::Center);
        my_language.update_property(&[gtk::accessible::Property::Label(MY_LANGUAGE)]);
        let rendering = std::rc::Rc::new(Cell::new(false));
        let language_name = single(MY_LANGUAGE, &["strong"]);
        language_name.set_hexpand(true);
        let choice = line(12, &[]);
        choice.append(&language_name);
        choice.append(&my_language);
        let language = column(4, &["first-run-language"]);
        language.set_margin_top(6);
        language.append(&choice);
        language.append(&label(MY_LANGUAGE_HINT, &["hint"]));
        language.set_visible(my.is_some());

        let hero = column(8, &[]);
        for widget in [
            done_mark.upcast_ref::<gtk::Widget>(),
            eyebrow.upcast_ref(),
            heading.upcast_ref(),
            lead.upcast_ref(),
            language.upcast_ref(),
        ] {
            hero.append(widget);
        }

        let figures = column(0, &[]);

        let current = single(INSTALLING, &["strong"]);
        current.set_hexpand(true);
        current.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let bytes = single("", &["hint", "figures-tabular"]);
        let top = line(12, &[]);
        top.append(&current);
        top.append(&bytes);
        let bar = gtk::ProgressBar::builder()
            .css_classes(["first-run-bar"])
            .build();
        let counts = single("", &["hint", "figures-tabular"]);
        let progress = column(10, &["first-run-card", "first-run-progress"]);
        progress.append(&top);
        progress.append(&bar);
        progress.append(&counts);
        progress.set_visible(false);

        let list_box = column(1, &["first-run-groups"]);
        let list = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(340)
            .css_classes(["first-run-list"])
            .overflow(gtk::Overflow::Hidden)
            .child(&list_box)
            .build();

        let cards = gtk::Grid::builder()
            .column_spacing(10)
            .row_spacing(10)
            .column_homogeneous(true)
            .visible(false)
            .build();

        let note = label(CREDIT, &["hint"]);
        note.set_hexpand(true);
        let credit = label(CREDIT, &["hint"]);
        credit.set_justify(gtk::Justification::Center);
        credit.set_halign(gtk::Align::Center);
        credit.set_xalign(0.5);
        credit.set_visible(false);

        let not_now = gtk::Button::builder()
            .label(if recovery { CLOSE } else { NOT_NOW })
            .css_classes(["first-run-button"])
            .build();
        let download_content = line(8, &[]);
        download_content.append(&gtk::Image::from_icon_name("folder-download-symbolic"));
        download_content.append(&gtk::Label::new(Some(DOWNLOAD)));
        let download = gtk::Button::builder()
            .child(&download_content)
            .css_classes(["save"])
            .build();
        download.update_property(&[gtk::accessible::Property::Label(DOWNLOAD)]);
        let cancel = gtk::Button::builder()
            .label(CANCEL_ALL)
            .css_classes(["first-run-button"])
            .visible(false)
            .build();
        let close = gtk::Button::builder()
            .label(CLOSE)
            .css_classes(["save"])
            .visible(false)
            .build();
        let buttons = line(10, &[]);
        buttons.set_valign(gtk::Align::Center);
        for button in [&not_now, &download, &cancel, &close] {
            buttons.append(button);
        }
        let footer = line(16, &[]);
        footer.append(&note);
        footer.append(&buttons);

        let content = column(18, &[]);
        content.set_margin_top(26);
        content.set_margin_bottom(24);
        content.set_margin_start(28);
        content.set_margin_end(28);
        for widget in [
            hero.upcast_ref::<gtk::Widget>(),
            figures.upcast_ref(),
            progress.upcast_ref(),
            list.upcast_ref(),
            cards.upcast_ref(),
            credit.upcast_ref(),
            footer.upcast_ref(),
        ] {
            content.append(widget);
        }

        let window = gtk::Window::builder()
            .title(TITLE)
            .transient_for(parent)
            .default_width(560)
            .resizable(false)
            .hide_on_close(true)
            .titlebar(&crate::resident::header_bar(TITLE))
            .child(&content)
            .css_classes(["addict", "settings", "first-run"])
            .build();
        let escape = gtk::EventControllerKey::new();
        let esc = window.downgrade();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(window) = esc.upgrade() {
                    window.close();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(escape);

        if let Some(my) = my {
            render_language(&my_language, &rendering, my);
        }
        let guard = rendering.clone();
        my_language.connect_selected_notify(move |dropdown| {
            if !guard.get()
                && let Some(chosen) = Language::ALL.get(dropdown.selected() as usize)
            {
                on_my_language(*chosen);
            }
        });
        download.connect_clicked(move |_| on_download());
        let closing = window.downgrade();
        not_now.connect_clicked(move |_| {
            on_not_now();
            if let Some(window) = closing.upgrade() {
                window.close();
            }
        });
        let cancel_all = on_cancel.clone();
        cancel.connect_clicked(move |_| cancel_all(None));
        let closing = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = closing.upgrade() {
                window.close();
            }
        });
        let offer = Self {
            window,
            recovery,
            hero,
            done_mark,
            eyebrow,
            heading,
            lead,
            language,
            my_language,
            rendering,
            figures,
            progress,
            current,
            bytes,
            bar,
            counts,
            list,
            list_box,
            cards,
            rows: RefCell::new(Vec::new()),
            on_cancel,
            note,
            credit,
            footer,
            download,
            not_now,
            cancel,
            close,
        };
        offer.list(indexes);
        offer
    }

    /// The dialog lists `indexes`, grouped by language, in place of what it
    /// listed, and its figures and cards are theirs.
    pub fn list(&self, indexes: &[Published]) {
        for container in [&self.figures, &self.list_box] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        while let Some(child) = self.cards.first_child() {
            self.cards.remove(&child);
        }
        self.figures.append(&figures_card(indexes));
        self.eyebrow
            .set_label(&eyebrow_text(self.recovery, indexes.len()));
        let groups = groups(indexes);
        let mut rows = Vec::new();
        for group in &groups {
            let head = line(12, &["group-head"]);
            let name = single(group.name, &["section-label"]);
            name.set_hexpand(true);
            head.append(&name);
            head.append(&single(
                &group_text(&group.indexes),
                &["hint", "group-size"],
            ));
            self.list_box.append(&head);
            for published in &group.indexes {
                let row = source_row(published, &self.on_cancel);
                self.list_box.append(&row.line);
                rows.push(row);
            }
        }
        for (i, group) in groups.iter().enumerate() {
            let i = i32::try_from(i).unwrap_or(i32::MAX);
            self.cards.attach(&group_card(group), i % 2, i / 2, 1, 1);
        }
        *self.rows.borrow_mut() = rows;
    }

    /// Shows `language` in the my-language choice without choosing it.
    pub fn render_my_language(&self, language: Language) {
        render_language(&self.my_language, &self.rendering, language);
    }

    #[cfg(test)]
    pub(crate) fn my_language(&self) -> &gtk::DropDown {
        &self.my_language
    }

    /// The names of the rows, in their order.
    #[cfg(test)]
    pub(crate) fn listed(&self) -> Vec<String> {
        self.rows
            .borrow()
            .iter()
            .map(|row| row.name.clone())
            .collect()
    }

    /// Whether the my-language choice is shown.
    #[cfg(test)]
    pub(crate) fn shows_my_language(&self) -> bool {
        self.language.is_visible()
    }

    #[cfg(test)]
    pub(crate) fn window(&self) -> &gtk::Window {
        &self.window
    }

    /// Closes the dialog: a recovery offer left with nothing to list.
    pub fn close(&self) {
        self.window.close();
    }

    /// Shows the dialog, with Download focused.
    pub fn present(&self) {
        self.window.present();
        self.download.grab_focus();
    }

    /// The installs run: the figures give way to the progress card, and
    /// Download and Not now to Cancel all.
    pub fn installing(&self) {
        self.eyebrow.remove_css_class("warn");
        self.eyebrow.add_css_class("accent");
        self.heading.set_label(INSTALLING_HEADING);
        self.lead.set_label(INSTALLING_BODY);
        self.language.set_visible(false);
        self.figures.set_visible(false);
        self.progress.set_visible(true);
        self.note
            .set_label(if self.recovery { RETRY } else { AFTER_FAILURE });
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(true);
        self.cancel.grab_focus();
    }

    /// Every row says where its source stands in `queue`, and the card
    /// above them how the whole stands.
    pub fn show(&self, queue: &Queue) {
        for (published, state) in queue.rows() {
            self.show_row(&published.name, state, published.size);
        }
        let progress = progress(queue);
        self.current.set_label(&progress.current);
        self.bytes.set_label(&progress.bytes);
        self.bar.set_fraction(progress.fraction);
        self.counts.set_label(&progress.counts);
        if let Some(step) = &progress.step {
            self.eyebrow.set_label(step);
        }
    }

    /// The row of `name` says `state`, for an asset of `size` bytes. Its
    /// Cancel is there while the source has not ended.
    pub fn show_row(&self, name: &str, state: &RowState, size: u64) {
        let rows = self.rows.borrow();
        let Some(row) = rows.iter().find(|row| row.name == name) else {
            return;
        };
        row.line.remove_css_class("running");
        row.status.set_css_classes(&["row-status"]);
        row.icon.set_visible(false);
        row.bar.set_visible(false);
        row.status.set_label(&state.text(size));
        match state {
            RowState::Running { received } => {
                row.line.add_css_class("running");
                row.status.add_css_class("running");
                row.bar.set_visible(true);
                row.bar
                    .set_fraction(percent(*received, size) as f64 / 100.0);
            }
            RowState::Installed => {
                row.icon.set_visible(true);
                row.status.add_css_class("good");
            }
            RowState::Failed(message) => {
                row.status.add_css_class("failed");
                row.sub.set_label(&reason_text(name, message));
                row.sub.add_css_class("failed");
                row.sub.set_tooltip_text(Some(message));
                row.sub.set_visible(true);
            }
            RowState::Queued | RowState::Cancelled => {}
        }
        row.cancel.set_visible(state.is_open());
    }

    /// The installs ended: the heading says how, `line` under it says what
    /// came, and only Close is left. `complete`, every source installed,
    /// trades the list for a card per language. The dialog is shown
    /// again, as it may have been closed while they ran.
    pub fn ended(&self, heading: &str, line: &str, complete: bool) {
        self.heading.set_label(heading);
        if complete && !self.recovery {
            self.lead.set_label(&format!("{line} {READY_BODY}"));
        } else {
            self.lead.set_label(line);
        }
        self.eyebrow.set_visible(false);
        self.figures.set_visible(false);
        self.progress.set_visible(false);
        for row in self.rows.borrow().iter() {
            row.cancel.set_visible(false);
        }
        if complete {
            self.done_mark.set_visible(true);
            self.hero.set_halign(gtk::Align::Center);
            for text in [&self.heading, &self.lead] {
                text.set_halign(gtk::Align::Center);
                text.set_justify(gtk::Justification::Center);
                text.set_xalign(0.5);
            }
            self.list.set_visible(false);
            self.cards.set_visible(true);
            self.note.set_visible(false);
            self.credit.set_visible(true);
            self.footer.set_halign(gtk::Align::Center);
            if !self.recovery {
                self.close.set_label(START);
            }
        } else {
            self.note.set_label(CREDIT);
        }
        self.download.set_visible(false);
        self.not_now.set_visible(false);
        self.cancel.set_visible(false);
        self.close.set_visible(true);
        self.window.present();
        self.close.grab_focus();
    }
}

impl Drop for Offer {
    fn drop(&mut self) {
        self.window.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry of `name` whose asset has `size` bytes.
    fn entry(name: &str, size: u64) -> Published {
        Published {
            name: name.to_owned(),
            format: addict_dictionary::format::VERSION,
            size,
            sha256: "0".repeat(64),
        }
    }

    /// Draws the dialog into the PNG file `ADDICT_SHOT` names: as offered,
    /// or, with `ADDICT_SHOT_STATE=installing` or `ended`, in that state.
    /// Run by hand, and alone, as `draws_the_window_into_a_png` is;
    /// `ADDICT_SHOT_SCHEME=dark` or `light` draws it in that scheme.
    #[test]
    #[ignore = "opens a window; run alone with ADDICT_SHOT=<file.png>"]
    fn draws_the_offer_into_a_png() {
        let path = std::env::var("ADDICT_SHOT").expect("ADDICT_SHOT");
        gtk::init().expect("a display");
        crate::style::install();
        let scheme = match std::env::var("ADDICT_SHOT_SCHEME").as_deref() {
            Ok("dark") => Some(gtk::InterfaceColorScheme::Dark),
            Ok("light") => Some(gtk::InterfaceColorScheme::Light),
            _ => None,
        };
        if let Some(scheme) = scheme {
            gtk::Settings::default()
                .expect("settings")
                .set_gtk_interface_color_scheme(scheme);
        }
        let parent = gtk::Window::new();
        let indexes: Vec<Published> = [
            ("oewn", 5_936_412),
            ("freedict", 4_477_832),
            ("wikdict-es-ru", 455_188),
            ("kaikki-es-Espanol", 31_450_328),
            ("omw-es", 2_990_976),
            ("kaikki-en-Spanish", 24_882_036),
            ("wikdict-ca-ru", 283_076),
            ("dbnary-ca", 12_662_468),
            ("sinonims-cat", 1_466_508),
            ("kaikki-en-Catalan", 6_575_224),
            ("wikdict-ru-en", 1_474_032),
            ("wikdict-ru-es", 511_408),
            ("wikdict-ru-ca", 361_780),
        ]
        .into_iter()
        .map(|(name, size)| entry(name, size))
        .collect();
        let offer = Offer::new(
            &parent,
            &indexes,
            false,
            Some(Language::Ru),
            |_| {},
            || {},
            || {},
            |_| {},
        );
        offer.present();
        let mut queue = Queue::new(indexes);
        let state = std::env::var("ADDICT_SHOT_STATE");
        if state.as_deref() == Ok("complete") {
            offer.installing();
            while let Some(published) = queue.next() {
                queue.finished(&published.name, Ok(()));
            }
            offer.show(&queue);
            offer.ended(ended_heading(&queue), &summary_text(&queue), true);
        } else if matches!(state.as_deref(), Ok("installing" | "ended")) {
            offer.installing();
            queue.next();
            queue.finished("oewn", Ok(()));
            queue.next();
            queue.finished("freedict", Ok(()));
            queue.next();
            queue.finished(
                "wikdict-es-ru",
                Err("cannot download wikdict-es-ru: the server answered HTTP 503".to_owned()),
            );
            queue.next();
            queue.progress("kaikki-es-Espanol", 20_128_210);
            if state.as_deref() == Ok("ended") {
                queue.cancel_all();
            }
            offer.show(&queue);
            if state.as_deref() == Ok("ended") {
                offer.ended(ended_heading(&queue), &summary_text(&queue), false);
            }
        }
        let context = glib::MainContext::default();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            context.iteration(false);
        }
        let window = &offer.window;
        let (width, height) = (window.width(), window.height());
        let paintable = gtk::WidgetPaintable::new(Some(window));
        let snapshot = gtk::Snapshot::new();
        paintable.snapshot(&snapshot, f64::from(width), f64::from(height));
        let node = snapshot.to_node().expect("something drawn");
        let renderer = window
            .native()
            .expect("a native")
            .renderer()
            .expect("a renderer");
        let texture = renderer.render_texture(&node, None);
        texture.save_to_png(&path).expect("the PNG");
        window.destroy();
        parent.destroy();
    }
}
