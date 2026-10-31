//! `addict uninstall` against a private `HOME`, `XDG_DATA_HOME` and
//! `XDG_CONFIG_HOME`, on no bus or on a private bus with a mock Background
//! portal. As the real one does, the mock removes the login entry; nothing
//! else may. No test can reach the desktop's portal.

mod common;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use common::{Homes, LEGACY, TempDir, lines, on_private_bus, place, shown, text};
use zbus::{
    Connection,
    message::Header,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const FOREIGN: &str = "[Desktop Entry]\nName=Someone else\n";
/// What the Background portal leaves there.
const FROM_PORTAL: &str =
    "[Desktop Entry]\nExec=addict --background\nX-XDP-Autostart=io.github.doqhu.Addict\n";

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const PORTAL: &str = "org.freedesktop.portal.Desktop";

type Options = HashMap<String, OwnedValue>;

/// How the mock answers a request to disable.
#[derive(Clone, Copy)]
enum Answer {
    /// Removes the entry and answers success, as the real portal does.
    Disable,
    /// Answers with this code.
    Code(u32),
    /// Answers success and leaves the file.
    LeaveTheFile,
}

struct MockState {
    entry: PathBuf,
    answer: Answer,
    log: Mutex<Vec<String>>,
    registered: Mutex<HashSet<String>>,
}

struct Registry(Arc<MockState>);

#[zbus::interface(name = "org.freedesktop.host.portal.Registry")]
impl Registry {
    fn register(&self, app_id: String, _options: Options, #[zbus(header)] header: Header<'_>) {
        let sender = header.sender().unwrap().to_string();
        self.0.registered.lock().unwrap().insert(sender);
        self.0
            .log
            .lock()
            .unwrap()
            .push(format!("Register {app_id}"));
    }
}

struct Background(Arc<MockState>);

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
        if !self.0.registered.lock().unwrap().contains(&sender) {
            return Err(zbus::fdo::Error::AccessDenied("not registered".to_owned()));
        }
        let autostart = bool::try_from(&options["autostart"]).unwrap();
        self.0
            .log
            .lock()
            .unwrap()
            .push(format!("RequestBackground autostart={autostart}"));
        let code = match self.0.answer {
            Answer::Disable => {
                std::fs::remove_file(&self.0.entry).unwrap();
                0
            }
            Answer::Code(code) => code,
            Answer::LeaveTheFile => 0,
        };
        let token = <&str>::try_from(&options["handle_token"]).unwrap();
        let sender = sender.trim_start_matches(':').replace('.', "_");
        let request: OwnedObjectPath = format!("{DESKTOP}/request/{sender}/{token}")
            .try_into()
            .unwrap();
        let results = if code == 0 {
            HashMap::from([
                ("background".to_owned(), OwnedValue::from(true)),
                ("autostart".to_owned(), OwnedValue::from(false)),
            ])
        } else {
            Options::new()
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

/// The mock portal, serving for as long as the test runs.
struct Mock(Arc<MockState>);

impl Mock {
    fn start(entry: &Path, answer: Answer) -> Self {
        let state = Arc::new(MockState {
            entry: entry.to_path_buf(),
            answer,
            log: Mutex::default(),
            registered: Mutex::default(),
        });
        let served = state.clone();
        let (ready, started) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let _connection = zbus::connection::Builder::session()
                    .unwrap()
                    .name(PORTAL)
                    .unwrap()
                    .serve_at(DESKTOP, Registry(served.clone()))
                    .unwrap()
                    .serve_at(DESKTOP, Background(served))
                    .unwrap()
                    .build()
                    .await
                    .unwrap();
                ready.send(()).unwrap();
                std::future::pending::<()>().await;
            });
        });
        started.recv().expect("the mock portal started");
        Self(state)
    }

    fn log(&self) -> Vec<String> {
        self.0.log.lock().unwrap().clone()
    }

    fn assert_asked_to_disable(&self) {
        assert_eq!(
            self.log(),
            [
                "Register io.github.doqhu.Addict",
                "RequestBackground autostart=false"
            ]
        );
    }

    fn assert_not_called(&self) {
        assert_eq!(self.log(), Vec::<String>::new());
    }
}

/// Both entry paths, applications first: the order of the output.
fn entries(homes: &Homes) -> [PathBuf; 2] {
    [homes.applications_entry(), homes.autostart_entry()]
}

fn installed(homes: &Homes) {
    let output = homes.run("install");
    assert!(output.status.success(), "{}", text(&output.stderr));
}

/// An install with a login entry of this content.
fn installed_with_login_entry(homes: &Homes, content: &str) {
    installed(homes);
    place(&homes.autostart_entry(), content);
}

fn replace(path: &Path, content: &str) {
    std::fs::remove_file(path).expect("remove entry");
    std::fs::write(path, content).expect("write entry");
}

/// Everything an install with a login entry and a saved state holds.
fn all_files(homes: &Homes) -> Vec<PathBuf> {
    let mut files = entries(homes).to_vec();
    files.extend(homes.icons());
    files.push(homes.startup_file());
    files
}

#[test]
fn removes_the_entry_and_icons_and_prints_their_paths() {
    let temp = TempDir::new("uninstall");
    let homes = Homes::inside(temp.path());
    let applications = homes.applications_entry();
    installed(&homes);

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let mut printed = vec![shown(&applications)];
    printed.extend(homes.icons().iter().map(|icon| shown(icon)));
    assert_eq!(lines(&output.stdout), printed);
    for path in [&applications].into_iter().chain(&homes.icons()) {
        assert!(!path.exists(), "{} is still there", path.display());
        // The directories are not removed.
        assert!(path.parent().expect("parent").is_dir());
    }
    // Removing creates nothing.
    assert!(!homes.config_home.exists());
}

/// A login entry of ours, from either writer: the portal removes it, with the
/// launcher still there, and then the files and the state go.
#[test]
fn a_login_entry_of_ours_goes_through_the_portal_and_is_listed() {
    if !on_private_bus("a_login_entry_of_ours_goes_through_the_portal_and_is_listed") {
        return;
    }
    let temp = TempDir::new("uninstall-ours");
    let homes = Homes::inside(temp.path());
    let [applications, autostart] = entries(&homes);
    let mock = Mock::start(&autostart, Answer::Disable);
    for (round, content) in [LEGACY, FROM_PORTAL].into_iter().enumerate() {
        installed_with_login_entry(&homes, content);
        place(&homes.startup_file(), "enabled\n");

        let output = homes.run_on_bus("uninstall");
        assert!(output.status.success(), "{}", text(&output.stderr));
        let mut printed = vec![shown(&applications), shown(&autostart)];
        printed.extend(homes.icons().iter().map(|icon| shown(icon)));
        printed.push(shown(&homes.startup_file()));
        assert_eq!(lines(&output.stdout), printed);
        for path in all_files(&homes) {
            assert!(!path.exists(), "{} is still there", path.display());
        }
        assert!(autostart.parent().expect("parent").is_dir());
        assert_eq!(mock.log().len(), (round + 1) * 2);
    }
}

#[test]
fn the_portal_is_registered_with_before_it_is_asked() {
    if !on_private_bus("the_portal_is_registered_with_before_it_is_asked") {
        return;
    }
    let temp = TempDir::new("uninstall-order");
    let homes = Homes::inside(temp.path());
    let mock = Mock::start(&homes.autostart_entry(), Answer::Disable);
    installed_with_login_entry(&homes, LEGACY);

    let output = homes.run_on_bus("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    mock.assert_asked_to_disable();
}

fn assert_refused_keeping_all(homes: &Homes, output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains(&shown(&homes.autostart_entry())),
        "{stderr}"
    );
    assert!(stderr.contains("nothing was removed"), "{stderr}");
    for path in all_files(homes) {
        assert!(path.is_file(), "{} was removed", path.display());
    }
}

#[test]
fn a_refusing_portal_removes_nothing_and_names_the_file() {
    if !on_private_bus("a_refusing_portal_removes_nothing_and_names_the_file") {
        return;
    }
    let temp = TempDir::new("uninstall-refused");
    let homes = Homes::inside(temp.path());
    let mock = Mock::start(&homes.autostart_entry(), Answer::Code(1));
    installed_with_login_entry(&homes, LEGACY);
    place(&homes.startup_file(), "enabled\n");

    let output = homes.run_on_bus("uninstall");
    assert_refused_keeping_all(&homes, &output);
    assert!(text(&output.stderr).contains("refused"));
    mock.assert_asked_to_disable();
}

#[test]
fn a_portal_that_ends_the_request_removes_nothing() {
    if !on_private_bus("a_portal_that_ends_the_request_removes_nothing") {
        return;
    }
    let temp = TempDir::new("uninstall-ended");
    let homes = Homes::inside(temp.path());
    let _mock = Mock::start(&homes.autostart_entry(), Answer::Code(2));
    installed_with_login_entry(&homes, FROM_PORTAL);
    place(&homes.startup_file(), "enabled\n");

    assert_refused_keeping_all(&homes, &homes.run_on_bus("uninstall"));
}

#[test]
fn a_success_that_leaves_the_file_removes_nothing() {
    if !on_private_bus("a_success_that_leaves_the_file_removes_nothing") {
        return;
    }
    let temp = TempDir::new("uninstall-left");
    let homes = Homes::inside(temp.path());
    let _mock = Mock::start(&homes.autostart_entry(), Answer::LeaveTheFile);
    installed_with_login_entry(&homes, LEGACY);
    place(&homes.startup_file(), "enabled\n");

    assert_refused_keeping_all(&homes, &homes.run_on_bus("uninstall"));
}

#[test]
fn no_bus_removes_nothing_and_names_the_file() {
    let temp = TempDir::new("uninstall-no-bus");
    let homes = Homes::inside(temp.path());
    installed_with_login_entry(&homes, LEGACY);
    place(&homes.startup_file(), "enabled\n");

    let output = homes.run("uninstall");
    assert_refused_keeping_all(&homes, &output);
    assert!(text(&output.stderr).contains("not available"));
}

/// No portal on the bus at all: an empty bus, nothing to activate.
#[test]
fn no_portal_on_the_bus_removes_nothing_and_names_the_file() {
    if !on_private_bus("no_portal_on_the_bus_removes_nothing_and_names_the_file") {
        return;
    }
    let temp = TempDir::new("uninstall-no-portal");
    let homes = Homes::inside(temp.path());
    installed_with_login_entry(&homes, FROM_PORTAL);
    place(&homes.startup_file(), "enabled\n");

    assert_refused_keeping_all(&homes, &homes.run_on_bus("uninstall"));
}

/// No entry: the portal has nothing to do, whatever the state says, and an
/// absent portal is no obstacle. The state goes with the rest.
#[test]
fn no_login_entry_calls_no_portal_and_the_state_goes() {
    if !on_private_bus("no_login_entry_calls_no_portal_and_the_state_goes") {
        return;
    }
    let temp = TempDir::new("uninstall-no-entry");
    let homes = Homes::inside(temp.path());
    let mock = Mock::start(&homes.autostart_entry(), Answer::Code(1));
    installed(&homes);
    place(&homes.startup_file(), "enabled\n");

    let output = homes.run_on_bus("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        lines(&output.stdout).last(),
        Some(&shown(&homes.startup_file()))
    );
    assert!(!homes.startup_file().exists());
    mock.assert_not_called();
}

#[test]
fn a_foreign_or_linked_login_entry_is_refused_and_the_portal_not_called() {
    if !on_private_bus("a_foreign_or_linked_login_entry_is_refused_and_the_portal_not_called") {
        return;
    }
    let temp = TempDir::new("uninstall-foreign-portal");
    let homes = Homes::inside(temp.path());
    let autostart = homes.autostart_entry();
    let mock = Mock::start(&autostart, Answer::Disable);
    installed_with_login_entry(&homes, FOREIGN);
    let output = homes.run_on_bus("uninstall");
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains(&shown(&autostart)));
    assert_eq!(std::fs::read_to_string(&autostart).expect("read"), FOREIGN);

    let target = temp.path().join("target.desktop");
    std::fs::write(&target, LEGACY).expect("write target");
    std::fs::remove_file(&autostart).expect("remove");
    std::os::unix::fs::symlink(&target, &autostart).expect("symlink");
    let output = homes.run_on_bus("uninstall");
    assert!(!output.status.success());
    assert!(
        std::fs::symlink_metadata(&autostart)
            .expect("metadata")
            .is_symlink()
    );
    assert!(homes.applications_entry().is_file());
    mock.assert_not_called();
}

/// Another target that would be refused stops it before the portal, which
/// could not be undone.
#[test]
fn a_foreign_icon_stops_it_before_the_portal() {
    if !on_private_bus("a_foreign_icon_stops_it_before_the_portal") {
        return;
    }
    let temp = TempDir::new("uninstall-foreign-icon");
    let homes = Homes::inside(temp.path());
    let mock = Mock::start(&homes.autostart_entry(), Answer::Disable);
    installed_with_login_entry(&homes, LEGACY);
    std::fs::write(homes.icon(256), b"someone else's icon").expect("write foreign");

    let output = homes.run_on_bus("uninstall");
    assert!(!output.status.success());
    assert!(homes.autostart_entry().is_file());
    mock.assert_not_called();
}

/// An install of before the icons: its entries are removed, no icon is
/// missed.
#[test]
fn entries_without_icons_are_not_an_error() {
    if !on_private_bus("entries_without_icons_are_not_an_error") {
        return;
    }
    let temp = TempDir::new("uninstall-no-icons");
    let homes = Homes::inside(temp.path());
    let entries = entries(&homes);
    let _mock = Mock::start(&homes.autostart_entry(), Answer::Disable);
    installed_with_login_entry(&homes, LEGACY);
    std::fs::remove_dir_all(homes.data_home.join("icons")).expect("remove icons");

    let output = homes.run_on_bus("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(lines(&output.stdout), entries.map(|entry| shown(&entry)));
}

/// A foreign file or a symlink at an icon path: nothing is removed.
#[test]
fn refuses_a_foreign_or_linked_icon_and_removes_nothing() {
    for link in [false, true] {
        let temp = TempDir::new("uninstall-icon");
        let homes = Homes::inside(temp.path());
        installed_with_login_entry(&homes, LEGACY);
        let icon = homes.icon(256);
        let kept: Vec<PathBuf> = entries(&homes)
            .into_iter()
            .chain(homes.icons().into_iter().filter(|other| *other != icon))
            .collect();
        if link {
            let target = temp.path().join("target.png");
            std::fs::rename(&icon, &target).expect("move icon");
            std::os::unix::fs::symlink(&target, &icon).expect("symlink");
        } else {
            std::fs::write(&icon, b"someone else's icon").expect("write foreign");
        }

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(&icon)), "{stderr}");
        for path in &kept {
            assert!(path.is_file(), "{} was removed", path.display());
        }
        assert_eq!(
            std::fs::symlink_metadata(&icon)
                .expect("metadata")
                .is_symlink(),
            link
        );
    }
}

#[test]
fn nothing_installed_is_not_an_error() {
    let temp = TempDir::new("uninstall-none");
    let homes = Homes::inside(temp.path());

    let output = homes.run("uninstall");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty());
    // Nothing was created either.
    assert_eq!(std::fs::read_dir(temp.path()).expect("list").count(), 0);
}

/// A login entry with no launcher: the real portal could not register this
/// application, so it is not asked and nothing goes.
#[test]
fn a_login_entry_without_the_launcher_is_refused_before_the_portal() {
    if !on_private_bus("a_login_entry_without_the_launcher_is_refused_before_the_portal") {
        return;
    }
    let temp = TempDir::new("uninstall-one");
    let homes = Homes::inside(temp.path());
    let [applications, autostart] = entries(&homes);
    let mock = Mock::start(&autostart, Answer::Disable);
    installed_with_login_entry(&homes, LEGACY);
    place(&homes.startup_file(), "enabled\n");
    std::fs::remove_file(&applications).expect("remove the launcher");

    let output = homes.run_on_bus("uninstall");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = text(&output.stderr);
    assert!(stderr.contains(&shown(&autostart)), "{stderr}");
    assert!(stderr.contains("nothing was removed"), "{stderr}");
    assert!(
        stderr.contains("run `addict install` and then `addict uninstall`"),
        "{stderr}"
    );
    assert!(autostart.is_file());
    assert!(homes.startup_file().is_file());
    for icon in homes.icons() {
        assert!(icon.is_file(), "{} was removed", icon.display());
    }
    mock.assert_not_called();
}

/// A foreign file at either path: neither file is removed.
#[test]
fn refuses_a_foreign_entry_and_removes_neither() {
    for foreign_at in 0..2 {
        let temp = TempDir::new("uninstall-foreign");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (foreign, other) = (&entries[foreign_at], &entries[1 - foreign_at]);
        installed_with_login_entry(&homes, LEGACY);
        let own = std::fs::read_to_string(other).expect("read entry");
        replace(foreign, FOREIGN);

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(foreign)), "{stderr}");
        assert_eq!(std::fs::read_to_string(foreign).expect("read"), FOREIGN);
        assert_eq!(std::fs::read_to_string(other).expect("read"), own);
    }
}

/// A portal entry where the launcher belongs is no launcher: refused before
/// the portal, not half way through.
#[test]
fn a_portal_entry_at_the_launcher_path_is_refused() {
    let temp = TempDir::new("uninstall-launcher");
    let homes = Homes::inside(temp.path());
    place(&homes.applications_entry(), FROM_PORTAL);

    let output = homes.run("uninstall");
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains(&shown(&homes.applications_entry())));
    assert!(homes.applications_entry().is_file());
}

/// A symlink at either path, even to a file with the marker.
#[test]
fn refuses_a_symlink_and_removes_neither() {
    for link_at in 0..2 {
        let temp = TempDir::new("uninstall-symlink");
        let homes = Homes::inside(temp.path());
        let entries = entries(&homes);
        let (link, other) = (&entries[link_at], &entries[1 - link_at]);
        installed_with_login_entry(&homes, LEGACY);
        let own = std::fs::read_to_string(other).expect("read entry");
        let linked = std::fs::read_to_string(link).expect("read linked entry");
        let target = temp.path().join("target.desktop");
        std::fs::rename(link, &target).expect("move entry");
        std::os::unix::fs::symlink(&target, link).expect("symlink");

        let output = homes.run("uninstall");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = text(&output.stderr);
        assert!(stderr.contains(&shown(link)), "{stderr}");
        assert_eq!(std::fs::read_to_string(&target).expect("read"), linked);
        let metadata = std::fs::symlink_metadata(link).expect("metadata");
        assert!(metadata.is_symlink());
        assert_eq!(std::fs::read_to_string(other).expect("read"), own);
    }
}

#[test]
fn relative_or_empty_config_home_falls_back_to_home() {
    if !on_private_bus("relative_or_empty_config_home_falls_back_to_home") {
        return;
    }
    let temp = TempDir::new("uninstall-relative");
    let autostart = temp
        .path()
        .join(".config/autostart/io.github.doqhu.Addict.desktop");
    let _mock = Mock::start(&autostart, Answer::Disable);
    for config_home in ["relative/config", ""] {
        let homes = Homes {
            config_home: PathBuf::from(config_home),
            ..Homes::inside(temp.path())
        };
        installed(&homes);
        let applications = homes.applications_entry();
        place(&autostart, LEGACY);

        let output = homes.run_on_bus("uninstall");
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(lines(&output.stdout).len(), 6);
        assert_eq!(
            lines(&output.stdout)[..2],
            [shown(&applications), shown(&autostart)]
        );
        assert!(!applications.exists());
        assert!(!autostart.exists());
    }
}
