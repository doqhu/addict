//! The stylesheet of the window, built into the binary and given to the
//! display once, before the window is built.

use gtk::{gdk, prelude::*};

/// The look of the window; the classes it names are given in
/// [`widgets`](crate::widgets) and [`resident`](crate::resident).
const STYLESHEET: &str = include_str!("style.css");

/// Gives the stylesheet to the default display, above the theme's own and
/// below the user's `gtk.css`. Without a display it does nothing: the window
/// then looks as the theme draws it. The dark palette, and the dark variant
/// of the theme under it, follow the color scheme of the desktop, also when
/// it changes while the window is open.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let settings = gtk::Settings::for_display(&display);
    let provider = gtk::CssProvider::new();
    // A provider of the application reads the scheme only once, when it is
    // made, and the desktop's scheme may not be known yet then: bound, it
    // takes the scheme once known, and every change after.
    settings
        .bind_property(
            "gtk-interface-color-scheme",
            &provider,
            "prefers-color-scheme",
        )
        .sync_create()
        .build();
    // GTK's own Adwaita stays light under a dark scheme; it draws what the
    // stylesheet leaves to it (the window controls, the selection, the
    // text of a button), so it is asked for its dark variant too. By name:
    // the property is deprecated for the scheme, which it follows here.
    settings
        .bind_property(
            "gtk-interface-color-scheme",
            &settings,
            "gtk-application-prefer-dark-theme",
        )
        .transform_to(|_, scheme: gtk::InterfaceColorScheme| {
            Some(scheme == gtk::InterfaceColorScheme::Dark)
        })
        .sync_create()
        .build();
    provider.load_from_string(STYLESHEET);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::STYLESHEET;

    #[test]
    fn the_stylesheet_parses_without_an_error() {
        // No display, as on a build machine: there is nothing to parse with.
        if gtk::init().is_err() {
            return;
        }
        let errors = Rc::new(RefCell::new(Vec::new()));
        let provider = gtk::CssProvider::new();
        let seen = errors.clone();
        provider.connect_parsing_error(move |_, section, error| {
            seen.borrow_mut()
                .push(format!("{}: {error}", section.to_str()));
        });
        provider.load_from_string(STYLESHEET);
        assert_eq!(errors.borrow().as_slice(), &[] as &[String]);
    }
}
