//! Addict application entry point.

use gio::glib::ExitCode;
use gio::prelude::*;

fn main() -> ExitCode {
    let application = gio::Application::builder()
        .application_id(addict_core::APP_ID)
        .build();
    application.connect_activate(|_| {});
    application.run()
}
