//! The About window, which the main menu and the tray's About entry show.
//!
//! Two pages under a switcher: About, with the application's icon, looked up
//! in the icon theme by the app id, its name, what it is, and a card of its
//! version, its license and its repository; and Credits, with every row of
//! the table of sources, the dictionary indexes first and then OCR, each with
//! its credit line and license. The credits are read from
//! `addict_dictionary::sources` and never typed here. A credit is plain text,
//! never markup, and an address in it stays text: a label makes no links of
//! its own. The texts are plain functions, so that they can be tested without
//! a display.

use addict_core::APP_ID;
use addict_dictionary::sources::{self, Row};
use gtk::{glib, prelude::*};

use crate::settings::{label, list_section, row};

/// The name of the application, as the window shows it.
pub const NAME: &str = "Addict";

/// The address of the repository.
pub const REPOSITORY: &str = "https://github.com/doqhu/addict";

/// What the application is, under its name.
const SUMMARY: &str = "A contextual dictionary for the Linux desktop.";

/// The name of the About page in the stack.
const ABOUT_PAGE: &str = "about";

/// The name of the Credits page in the stack.
const CREDITS_PAGE: &str = "credits";

/// The version of this build.
pub fn version_text() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The license, as the crate declares it.
pub fn license_text() -> &'static str {
    env!("CARGO_PKG_LICENSE")
}

/// The address of the repository as the window shows it: without the scheme.
pub fn repository_text() -> &'static str {
    REPOSITORY.trim_start_matches("https://")
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
            .margin_top(16)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&switcher);
        content.append(&stack);

        let window = gtk::Window::builder()
            .title("About Addict")
            .transient_for(parent)
            .default_width(520)
            .default_height(560)
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

/// The About page: the icon, the name and what it is, over a card of the
/// version, the license and the repository. An icon that is not installed
/// shows as the theme's missing icon.
fn about_page() -> gtk::Box {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .valign(gtk::Align::Center)
        .margin_top(16)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .build();
    let logo = gtk::Image::from_icon_name(APP_ID);
    logo.set_pixel_size(96);
    logo.set_margin_bottom(6);
    page.append(&logo);
    let name = label(NAME, &["about-name"]);
    name.set_halign(gtk::Align::Center);
    page.append(&name);
    let summary = label(SUMMARY, &["hint"]);
    summary.set_halign(gtk::Align::Center);
    page.append(&summary);
    let link = gtk::LinkButton::builder()
        .uri(REPOSITORY)
        .label(repository_text())
        .css_classes(["repository"])
        .build();
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_top(16)
        .css_classes(["settings-list"])
        .build();
    card.append(&fact("Version", label(version_text(), &[]).upcast_ref()));
    card.append(&fact("License", label(license_text(), &[]).upcast_ref()));
    card.append(&fact("Source", link.upcast_ref()));
    page.append(&card);
    page
}

/// A row of the card: what it says, and `value` at its end.
fn fact(name: &str, value: &gtk::Widget) -> gtk::Box {
    let name = label(name, &["hint"]);
    name.set_hexpand(true);
    name.set_valign(gtk::Align::Center);
    let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    line.append(&name);
    line.append(value);
    row(&[line.upcast_ref()])
}

/// The row of one source: its index, its credit line, its license.
fn credit(source: &Row) -> gtk::Box {
    let name = label(source.index, &["credit-name"]);
    let line = label(source.credit, &[]);
    let license = label(source.license, &["hint"]);
    row(&[name.upcast_ref(), line.upcast_ref(), license.upcast_ref()])
}

/// The Credits page: a section per table, a row per source, in a scrolled
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
        let rows: Vec<gtk::Box> = rows.iter().map(credit).collect();
        let rows: Vec<&gtk::Box> = rows.iter().collect();
        content.append(&list_section(heading, &rows));
    }
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&content)
        .build()
}

#[cfg(test)]
mod tests {
    use gtk::{glib, prelude::*};

    use super::{About, license_text, repository_text, version_text};
    use crate::style;

    #[test]
    fn the_version_is_the_crate_version() {
        assert_eq!(version_text(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_license_is_the_crate_license() {
        assert_eq!(license_text(), env!("CARGO_PKG_LICENSE"));
        assert!(!license_text().is_empty());
    }

    #[test]
    fn the_repository_is_shown_without_its_scheme() {
        assert_eq!(repository_text(), "github.com/doqhu/addict");
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
