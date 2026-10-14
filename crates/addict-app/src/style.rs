//! The stylesheet of the window, built into the binary and given to the
//! display once, before the window is built.

use gtk::gdk;

/// The look of the window; the classes it names are given in
/// [`widgets`](crate::widgets) and [`resident`](crate::resident).
const STYLESHEET: &str = include_str!("style.css");

/// Gives the stylesheet to the default display, above the theme's own and
/// below the user's `gtk.css`. Without a display it does nothing: the window
/// then looks as the theme draws it.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let provider = gtk::CssProvider::new();
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
