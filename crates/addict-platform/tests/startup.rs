//! The real startup request against a mock Background portal on an isolated
//! session bus, with no service directory: nothing can activate the desktop's
//! portal. No test talks to it, or touches an autostart directory.

mod common;

use std::{
    collections::HashMap,
    ffi::OsStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use addict_platform::startup::{self, StartupFailure};
use zbus::{
    Connection,
    message::Header,
    zvariant::{Array, OwnedObjectPath, OwnedValue},
};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const APP_ID: &str = "io.github.doqhu.Test";
const CHILD: &str = "ADDICT_TEST_STARTUP_PORTAL_CHILD";
const WAIT: Duration = Duration::from_secs(5);
const BUS_ADDRESS: &str = "unix:path=/nonexistent/addict-test-bus";
type Options = HashMap<String, OwnedValue>;

#[derive(Clone, Copy)]
enum Answer {
    /// Code 0, with this `autostart`.
    Success(bool),
    /// This code, with no results.
    Code(u32),
    /// Never.
    Silent,
}

#[derive(Debug, PartialEq, Eq)]
struct Asked {
    autostart: bool,
    command: Vec<String>,
    reason: String,
}

struct State {
    answer: Answer,
    refuse_register: bool,
    log: Mutex<Vec<&'static str>>,
    registered: Mutex<Vec<(String, String)>>,
    asked: Mutex<Vec<Asked>>,
}

struct Registry(Arc<State>);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    fn register(
        &self,
        app_id: String,
        _options: Options,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        if self.0.refuse_register {
            return Err(zbus::fdo::Error::Failed("registration refused".to_owned()));
        }
        self.0.log.lock().unwrap().push("Register");
        let sender = header.sender().unwrap().to_string();
        self.0.registered.lock().unwrap().push((sender, app_id));
        Ok(())
    }
}

struct Background(Arc<State>);

#[zbus::interface(name = "org.freedesktop.portal.Background")]
impl Background {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        2
    }

    async fn request_background(
        &self,
        _parent: String,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let sender = header.sender().unwrap().to_string();
        // Like the real portal: a host app's identity comes from its
        // registration on this very connection.
        if !self
            .0
            .registered
            .lock()
            .unwrap()
            .iter()
            .any(|(registered, _)| *registered == sender)
        {
            return Err(zbus::fdo::Error::AccessDenied("not registered".to_owned()));
        }
        self.0.log.lock().unwrap().push("RequestBackground");
        let command = options.get("commandline").map_or_else(Vec::new, |items| {
            <&Array>::try_from(items)
                .unwrap()
                .iter()
                .map(|item| <&str>::try_from(item).unwrap().to_owned())
                .collect()
        });
        self.0.asked.lock().unwrap().push(Asked {
            autostart: bool::try_from(&options["autostart"]).unwrap(),
            command,
            reason: <&str>::try_from(&options["reason"]).unwrap().to_owned(),
        });
        let token = <&str>::try_from(&options["handle_token"]).unwrap();
        let sender = sender.trim_start_matches(':').replace('.', "_");
        let request: OwnedObjectPath = format!("{DESKTOP}/request/{sender}/{token}")
            .try_into()
            .unwrap();
        let (code, results) = match self.0.answer {
            Answer::Success(autostart) => (
                0u32,
                HashMap::from([
                    ("background".to_owned(), OwnedValue::from(true)),
                    ("autostart".to_owned(), OwnedValue::from(autostart)),
                ]),
            ),
            Answer::Code(code) => (code, Options::new()),
            Answer::Silent => return Ok(request),
        };
        connection
            .emit_signal(
                None::<&str>,
                &request,
                "org.freedesktop.portal.Request",
                "Response",
                &(code, results),
            )
            .await?;
        Ok(request)
    }
}

struct Mock {
    _runtime: tokio::runtime::Runtime,
    _connection: Connection,
    state: Arc<State>,
}

impl Mock {
    fn start(answer: Answer) -> Self {
        Self::with(answer, false)
    }

    fn with(answer: Answer, refuse_register: bool) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let state = Arc::new(State {
            answer,
            refuse_register,
            log: Mutex::default(),
            registered: Mutex::default(),
            asked: Mutex::default(),
        });
        let connection = runtime.block_on(async {
            zbus::connection::Builder::session()
                .unwrap()
                .name(PORTAL)
                .unwrap()
                .serve_at(DESKTOP, Registry(state.clone()))
                .unwrap()
                .serve_at(DESKTOP, Background(state.clone()))
                .unwrap()
                .build()
                .await
                .unwrap()
        });
        Self {
            _runtime: runtime,
            _connection: connection,
            state,
        }
    }
}

/// Whether this is the child on a private bus; when not, runs it.
fn private(test: &str) -> bool {
    common::on_private_bus(CHILD, "startup", test)
}

fn command() -> Vec<String> {
    vec!["\"/a b/addict\"".to_owned(), "--background".to_owned()]
}

fn ask(enable: bool) -> Result<(), StartupFailure> {
    startup::request_blocking(APP_ID, enable, &command(), WAIT)
}

#[test]
fn registers_first_then_sends_the_options_and_is_confirmed() {
    if !private("registers_first_then_sends_the_options_and_is_confirmed") {
        return;
    }
    let mock = Mock::start(Answer::Success(true));
    assert_eq!(ask(true), Ok(()));
    assert_eq!(
        *mock.state.log.lock().unwrap(),
        ["Register", "RequestBackground"]
    );
    assert_eq!(mock.state.registered.lock().unwrap()[0].1, APP_ID);
    let asked = mock.state.asked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].autostart);
    assert_eq!(asked[0].command, command());
    assert!(!asked[0].reason.is_empty());
}

#[test]
fn each_request_registers_on_a_fresh_connection() {
    if !private("each_request_registers_on_a_fresh_connection") {
        return;
    }
    let mock = Mock::start(Answer::Success(true));
    assert_eq!(ask(true), Ok(()));
    assert_eq!(ask(true), Ok(()));
    let registered = mock.state.registered.lock().unwrap();
    assert_eq!(registered.len(), 2);
    assert_ne!(registered[0].0, registered[1].0);
}

#[test]
fn disable_is_confirmed_by_success_without_autostart() {
    if !private("disable_is_confirmed_by_success_without_autostart") {
        return;
    }
    let mock = Mock::start(Answer::Success(false));
    assert_eq!(ask(false), Ok(()));
    let asked = mock.state.asked.lock().unwrap();
    assert!(!asked[0].autostart);
    assert_eq!(
        asked[0].command,
        Vec::<String>::new(),
        "a disable sends none"
    );
}

#[test]
fn a_disable_needs_no_command() {
    if !private("a_disable_needs_no_command") {
        return;
    }
    let _mock = Mock::start(Answer::Success(false));
    assert_eq!(startup::request_blocking(APP_ID, false, &[], WAIT), Ok(()));
}

#[test]
fn success_without_autostart_refuses_an_enable() {
    if !private("success_without_autostart_refuses_an_enable") {
        return;
    }
    let _mock = Mock::start(Answer::Success(false));
    assert_eq!(ask(true), Err(StartupFailure::Refused));
}

#[test]
fn success_with_autostart_refuses_a_disable() {
    if !private("success_with_autostart_refuses_a_disable") {
        return;
    }
    let _mock = Mock::start(Answer::Success(true));
    assert_eq!(ask(false), Err(StartupFailure::Refused));
}

#[test]
fn request_gives_the_answer_as_it_came() {
    if !private("request_gives_the_answer_as_it_came") {
        return;
    }
    let _mock = Mock::start(Answer::Success(false));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        runtime.block_on(startup::request(APP_ID, true, &command())),
        Ok(false)
    );
}

#[test]
fn code_one_is_a_refusal() {
    if !private("code_one_is_a_refusal") {
        return;
    }
    let _mock = Mock::start(Answer::Code(1));
    assert_eq!(ask(true), Err(StartupFailure::Refused));
}

#[test]
fn code_two_is_a_refusal() {
    if !private("code_two_is_a_refusal") {
        return;
    }
    let _mock = Mock::start(Answer::Code(2));
    assert_eq!(ask(false), Err(StartupFailure::Refused));
}

#[test]
fn no_answer_within_the_limit() {
    if !private("no_answer_within_the_limit") {
        return;
    }
    let _mock = Mock::start(Answer::Silent);
    assert_eq!(
        startup::request_blocking(APP_ID, true, &command(), Duration::from_millis(300)),
        Err(StartupFailure::NoAnswer)
    );
}

#[test]
fn a_refused_registration_stops_the_request() {
    if !private("a_refused_registration_stops_the_request") {
        return;
    }
    let mock = Mock::with(Answer::Success(true), true);
    let failure = ask(true).expect_err("refused");
    assert!(
        matches!(&failure, StartupFailure::Failed(text) if text.contains("host registration")),
        "{failure:?}"
    );
    assert!(mock.state.asked.lock().unwrap().is_empty());
}

#[test]
fn an_absent_portal_is_unavailable() {
    if !private("an_absent_portal_is_unavailable") {
        return;
    }
    assert_eq!(ask(true), Err(StartupFailure::Unavailable));
}

#[test]
fn no_bus_is_unavailable() {
    if std::env::var_os(CHILD).is_none() {
        common::run_child(
            "no_bus_is_unavailable",
            Duration::from_secs(20),
            &[
                (CHILD, OsStr::new("1")),
                ("DBUS_SESSION_BUS_ADDRESS", OsStr::new(BUS_ADDRESS)),
            ],
        );
        return;
    }
    assert_eq!(ask(true), Err(StartupFailure::Unavailable));
}
