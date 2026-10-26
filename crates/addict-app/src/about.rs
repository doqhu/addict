//! The About window, which the tray's About entry shows.
//!
//! Two pages under a switcher: About, with the application's icon, looked up
//! in the icon theme by the app id, its name, its version, its license and
//! the address of its repository; and Credits, with every row of the table
//! of sources, the dictionary indexes first and then OCR, each with its
//! credit line and license. The credits are read from
//! `addict_dictionary::sources` and never typed here. A credit is escaped
//! before it is set in markup, and an address in it stays text: a label
//! makes no links of its own. The texts are plain functions, so that they
//! can be tested without a display.

use addict_core::APP_ID;
use addict_dictionary::sources::{self, Row};
use gtk::{glib, prelude::*};

use crate::settings::{label, section};

/// The name of the application, as the window shows it.
pub const NAME: &str = "Addict";

/// The address of the repository.
pub const REPOSITORY: &str = "https://github.com/doqhu/addict";

/// The name of the About page in the stack.
const ABOUT_PAGE: &str = "about";

/// The name of the Credits page in the stack.
const CREDITS_PAGE: &str = "credits";

/// The line of the version of this build.
pub fn version_text() -> String {
    format!("Version {}", env!("CARGO_PKG_VERSION"))
}

/// The line of the license, as the crate declares it.
pub fn license_text() -> String {
    format!("Licensed under {}.", env!("CARGO_PKG_LICENSE"))
}

/// The markup of one row: its index in bold, its credit line, its license.
/// Every text is escaped before the markup is put around it.
pub fn credit_markup(row: &Row) -> String {
    format!(
        "<b>{}</b>\n{}\n<small>{}</small>",
        glib::markup_escape_text(row.index),
        glib::markup_escape_text(row.credit),
        glib::markup_escape_text(row.license),
    )
}

/// The window and its pages.
pub struct About {
    window: gtk::Window,
    stack: gtk::Stack,
}

impl About {
    /// Builds the window, hidden, above `parent`.
    pub fn new(parent: &impl IsA<gtk::Window>) -> Self {
        let stack = gtk::Stack::builder()
            .vexpand(true)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_titled(&about_page(), Some(ABOUT_PAGE), "About");
        stack.add_titled(&credits_page(), Some(CREDITS_PAGE), "Credits");
        let switcher = gtk::StackSwitcher::builder()
            .stack(&stack)
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&switcher);
        content.append(&stack);

        let window = gtk::Window::builder()
            .title("About Addict")
            .transient_for(parent)
            .default_width(480)
            .default_height(600)
            .hide_on_close(true)
            .titlebar(&crate::resident::header_bar("About Addict"))
            .child(&content)
            .css_classes(["addict", "settings", "about"])
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
        Self { window, stack }
    }

    /// Shows the window on its About page.
    pub fn present(&self) {
        self.stack.set_visible_child_name(ABOUT_PAGE);
        self.window.present();
    }

    /// Turns to the Credits page.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn show_credits(&self) {
        self.stack.set_visible_child_name(CREDITS_PAGE);
    }
}

/// The About page: the icon, the name, the version, the license and the
/// repository. An icon that is not installed shows as the theme's missing
/// icon.
fn about_page() -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .build();
    let logo = gtk::Image::from_icon_name(APP_ID);
    logo.set_pixel_size(128);
    logo.set_margin_bottom(8);
    page.append(&logo);
    let name = label(NAME, &["about-name"]);
    name.set_halign(gtk::Align::Center);
    page.append(&name);
    for text in [version_text(), license_text()] {
        let line = label(&text, &["hint"]);
        line.set_halign(gtk::Align::Center);
        page.append(&line);
    }
    let link = gtk::LinkButton::builder()
        .uri(REPOSITORY)
        .label(REPOSITORY)
        .halign(gtk::Align::Center)
        .build();
    page.append(&link);
    page
}

/// The Credits page: a section per table, a line per row, in a scrolled
/// window.
fn credits_page() -> gtk::ScrolledWindow {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    for (heading, rows) in sources::SECTIONS {
        let lines: Vec<gtk::Widget> = rows
            .iter()
            .map(|row| {
                let line = label("", &["credit"]);
                line.set_markup(&credit_markup(row));
                line.upcast()
            })
            .collect();
        let lines: Vec<&gtk::Widget> = lines.iter().collect();
        content.append(&section(heading, &lines));
    }
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build()
}

#[cfg(test)]
mod tests {
    use addict_dictionary::sources::{OCR, ROWS, Row};
    use gtk::{glib, prelude::*};

    use super::{About, credit_markup, license_text, version_text};
    use crate::style;

    #[test]
    fn each_line_holds_its_index_and_its_credit() {
        for row in ROWS.iter().chain(OCR) {
            let markup = credit_markup(row);
            let index = glib::markup_escape_text(row.index);
            let credit = glib::markup_escape_text(row.credit);
            assert!(markup.contains(index.as_str()), "{}", row.index);
            assert!(markup.contains(credit.as_str()), "{}", row.index);
            gtk::pango::parse_markup(&markup, '\0').expect("valid markup");
        }
    }

    #[test]
    fn an_address_in_a_credit_stays_text_and_whole() {
        let kaikki = ROWS
            .iter()
            .find(|row| row.index == "kaikki-es-Espanol")
            .expect("the Kaikki row");
        let markup = credit_markup(kaikki);
        assert!(!markup.contains("<a"), "{markup}");
        assert!(
            markup.contains("(https://www.wiktionary.org/);"),
            "{markup}"
        );
    }

    #[test]
    fn a_credit_is_escaped_before_markup() {
        let row = Row {
            index: "x<y",
            upstream: "",
            license: "A & B",
            credit: "<a href=\"https://example.org/\">here</a>",
        };
        let markup = credit_markup(&row);
        assert!(!markup.contains("<a "), "{markup}");
        assert!(markup.contains("x&lt;y"), "{markup}");
        assert!(markup.contains("A &amp; B"), "{markup}");
        let (_, text, _) = gtk::pango::parse_markup(&markup, '\0').expect("valid markup");
        assert!(text.contains(row.credit), "{text}");
    }

    #[test]
    fn the_version_is_the_crate_version() {
        assert_eq!(
            version_text(),
            format!("Version {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_license_is_the_crate_license() {
        assert_eq!(
            license_text(),
            format!("Licensed under {}.", env!("CARGO_PKG_LICENSE"))
        );
        assert!(!env!("CARGO_PKG_LICENSE").is_empty());
    }

    /// Draws the About window on its Credits page into the PNG file
    /// `ADDICT_SHOT` names, tall enough for every row. Run by hand, and
    /// alone, as `draws_the_window_into_a_png` is;
    /// `ADDICT_SHOT_SCHEME=dark` or `light` draws it in that scheme.
    #[test]
    #[ignore = "opens a window; run alone with ADDICT_SHOT=<file.png>"]
    fn draws_about_credits_into_a_png() {
        let path = std::env::var("ADDICT_SHOT").expect("ADDICT_SHOT");
        gtk::init().expect("a display");
        style::install();
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
        let about = About::new(&parent);
        // Taller than a screen, which caps a default size: every row shows.
        about.window.set_size_request(520, 1700);
        about.present();
        about.show_credits();
        GtkWindowExt::set_focus(&about.window, None::<&gtk::Widget>);
        let context = glib::MainContext::default();
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            context.iteration(false);
        }
        let window = &about.window;
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
