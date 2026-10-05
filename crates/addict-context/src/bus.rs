//! The shared connection: where the accessibility bus is, what
//! `org.a11y.Status` says, which applications are registered.

use std::time::Duration;

use addict_probe::record::{Bounded, Progress, bounded};
use futures_util::future::join_all;
use serde::{Serialize, de::DeserializeOwned};
use zbus::{
    Connection,
    zvariant::{DynamicType, OwnedObjectPath, OwnedValue, Type},
};

use crate::record::{Accessibility, Finish, Outcome};

pub const A11Y_BUS_NAME: &str = "org.a11y.Bus";
pub const A11Y_BUS_PATH: &str = "/org/a11y/bus";
pub const REGISTRY_NAME: &str = "org.a11y.atspi.Registry";
pub const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

pub const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
pub const APPLICATION: &str = "org.a11y.atspi.Application";
pub const TEXT: &str = "org.a11y.atspi.Text";

/// Key under which the accessibility block travels in `Progress`, so that a
/// run cut by the timeout still records it.
pub const ACCESSIBILITY_KEY: &str = "accessibility";
/// Key under which the matched application(s) travel in `Progress`.
pub const TARGET_KEY: &str = "target";

/// One accessible object: a bus name and an object path on the
/// accessibility bus.
#[derive(Debug, Clone)]
pub struct Object {
    connection: Connection,
    pub bus_name: String,
    pub path: String,
}

impl Object {
    pub fn new(connection: &Connection, bus_name: &str, path: &str) -> Self {
        Self {
            connection: connection.clone(),
            bus_name: bus_name.to_owned(),
            path: path.to_owned(),
        }
    }

    /// Calls a method and returns its reply, or the error text as the bus
    /// gave it.
    pub async fn call<B, R>(&self, interface: &str, member: &str, body: &B) -> Result<R, String>
    where
        B: Serialize + DynamicType + Sync,
        R: DeserializeOwned + Type,
    {
        let reply = self
            .connection
            .call_method(
                Some(self.bus_name.as_str()),
                self.path.as_str(),
                Some(interface),
                member,
                body,
            )
            .await
            .map_err(|e| e.to_string())?;
        let body = reply.body();
        body.deserialize::<R>().map_err(|e| e.to_string())
    }

    /// Reads one property.
    pub async fn property<T>(&self, interface: &str, name: &str) -> Result<T, String>
    where
        T: TryFrom<OwnedValue>,
        T::Error: std::fmt::Display,
    {
        let value: OwnedValue = self
            .call("org.freedesktop.DBus.Properties", "Get", &(interface, name))
            .await?;
        T::try_from(value).map_err(|e| e.to_string())
    }

    /// Children as the object lists them; falls back to asking one index at
    /// a time when `GetChildren` is not answered.
    pub async fn children(&self) -> Result<Vec<Object>, String> {
        let listed: Result<Vec<(String, OwnedObjectPath)>, String> =
            self.call(ACCESSIBLE, "GetChildren", &()).await;
        let pairs = match listed {
            Ok(pairs) => pairs,
            Err(first) => {
                let count: i32 = self
                    .property(ACCESSIBLE, "ChildCount")
                    .await
                    .map_err(|second| format!("GetChildren: {first}; ChildCount: {second}"))?;
                let mut pairs = Vec::new();
                for index in 0..count.max(0) {
                    let pair: (String, OwnedObjectPath) = self
                        .call(ACCESSIBLE, "GetChildAtIndex", &(index,))
                        .await
                        .map_err(|e| format!("GetChildAtIndex({index}): {e}"))?;
                    pairs.push(pair);
                }
                pairs
            }
        };
        Ok(pairs
            .into_iter()
            .map(|(bus_name, path)| Object::new(&self.connection, &bus_name, path.as_str()))
            .collect())
    }
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Finds and opens the accessibility bus: `AT_SPI_BUS_ADDRESS` if set, else
/// the address `org.a11y.Bus` gives on the session bus. `org.a11y.Status` is
/// read on the way. A bus that is absent or refuses is `unavailable`.
pub async fn open(progress: &Progress) -> Result<Connection, Finish> {
    let mut accessibility = Accessibility::default();
    progress.phase("session-bus");
    let session = Connection::session().await;
    let launcher = session
        .as_ref()
        .ok()
        .map(|connection| Object::new(connection, A11Y_BUS_NAME, A11Y_BUS_PATH));

    if let Some(launcher) = &launcher {
        progress.phase("a11y-status");
        let mut errors = Vec::new();
        match launcher
            .property::<bool>("org.a11y.Status", "IsEnabled")
            .await
        {
            Ok(value) => accessibility.is_enabled = Some(value),
            Err(e) => errors.push(format!("IsEnabled: {e}")),
        }
        match launcher
            .property::<bool>("org.a11y.Status", "ScreenReaderEnabled")
            .await
        {
            Ok(value) => accessibility.screen_reader_enabled = Some(value),
            Err(e) => errors.push(format!("ScreenReaderEnabled: {e}")),
        }
        if !errors.is_empty() {
            accessibility.status_error = Some(errors.join("; "));
        }
    } else if let Err(e) = &session {
        accessibility.status_error = Some(format!("session bus: {e}"));
    }
    progress.put(ACCESSIBILITY_KEY, &accessibility);

    progress.phase("a11y-address");
    let address = if let Some(address) = env_var("AT_SPI_BUS_ADDRESS") {
        accessibility.bus_address_source = Some("AT_SPI_BUS_ADDRESS".to_owned());
        address
    } else {
        let Some(launcher) = &launcher else {
            let error = accessibility.status_error.clone().unwrap_or_default();
            return Err(Finish::with_error(
                Outcome::Unavailable,
                format!(
                    "AT_SPI_BUS_ADDRESS is not set and the session bus is unreachable: {error}"
                ),
            ));
        };
        accessibility.bus_address_source = Some(A11Y_BUS_NAME.to_owned());
        match launcher
            .call::<_, String>(A11Y_BUS_NAME, "GetAddress", &())
            .await
        {
            Ok(address) if !address.is_empty() => address,
            Ok(_) => {
                progress.put(ACCESSIBILITY_KEY, &accessibility);
                return Err(Finish::with_error(
                    Outcome::Unavailable,
                    "org.a11y.Bus GetAddress returned an empty address",
                ));
            }
            Err(e) => {
                progress.put(ACCESSIBILITY_KEY, &accessibility);
                return Err(Finish::with_error(
                    Outcome::Unavailable,
                    format!("org.a11y.Bus GetAddress: {e}"),
                ));
            }
        }
    };
    accessibility.bus_address = Some(address.clone());
    progress.put(ACCESSIBILITY_KEY, &accessibility);

    progress.phase("a11y-connect");
    let unavailable = |e: zbus::Error| {
        Finish::with_error(
            Outcome::Unavailable,
            format!("accessibility bus at {address}: {e}"),
        )
    };
    zbus::connection::Builder::address(address.as_str())
        .map_err(unavailable)?
        .build()
        .await
        .map_err(unavailable)
}

/// One application registered on the accessibility bus.
#[derive(Debug, Clone, Serialize)]
pub struct Application {
    pub bus_name: String,
    pub path: String,
    pub name: Option<String>,
    pub toolkit: Option<String>,
    pub toolkit_version: Option<String>,
    pub pid: Option<u32>,
    /// Why a value above is `null`.
    pub error: Option<String>,
}

async fn describe(root: Object, dbus: Object) -> Application {
    let owner = (root.bus_name.as_str(),);
    let (name, toolkit, version, pid) = tokio::join!(
        root.property::<String>(ACCESSIBLE, "Name"),
        root.property::<String>(APPLICATION, "ToolkitName"),
        root.property::<String>(APPLICATION, "Version"),
        dbus.call::<_, u32>("org.freedesktop.DBus", "GetConnectionUnixProcessID", &owner),
    );
    let mut errors = Vec::new();
    let mut keep = |label: &str, error: Option<String>| {
        if let Some(error) = error {
            errors.push(format!("{label}: {error}"));
        }
    };
    keep("Name", name.as_ref().err().cloned());
    keep("ToolkitName", toolkit.as_ref().err().cloned());
    keep("Version", version.as_ref().err().cloned());
    keep("pid", pid.as_ref().err().cloned());
    Application {
        bus_name: root.bus_name,
        path: root.path,
        name: name.ok(),
        toolkit: toolkit.ok(),
        toolkit_version: version.ok(),
        pid: pid.ok(),
        error: (!errors.is_empty()).then(|| errors.join("; ")),
    }
}

/// Every application the registry lists. The registry's own answer is not
/// bounded here (the run's deadline covers it); each application gets
/// `per_application` to describe itself and is listed without details when
/// it does not.
pub async fn applications(
    connection: &Connection,
    per_application: Duration,
    progress: &Progress,
) -> Result<Vec<Application>, String> {
    progress.phase("registry");
    let registry = Object::new(connection, REGISTRY_NAME, ROOT_PATH);
    let roots = registry
        .children()
        .await
        .map_err(|e| format!("registry: {e}"))?;
    progress.phase("applications");
    let dbus = Object::new(connection, "org.freedesktop.DBus", "/org/freedesktop/DBus");
    let described = roots.into_iter().map(|root| {
        let dbus = dbus.clone();
        async move {
            let (bus_name, path) = (root.bus_name.clone(), root.path.clone());
            match bounded(per_application, describe(root, dbus)).await {
                Bounded::Done(application) => application,
                Bounded::TimedOut => Application {
                    bus_name,
                    path,
                    name: None,
                    toolkit: None,
                    toolkit_version: None,
                    pid: None,
                    error: Some(format!(
                        "no answer within {} s",
                        per_application.as_secs_f64()
                    )),
                },
            }
        }
    });
    Ok(join_all(described).await)
}

/// Applications whose registered name is `wanted`, compared without case.
/// Exact matches win; otherwise every name containing `wanted` matches.
pub fn matching<'a>(applications: &'a [Application], wanted: &str) -> Vec<&'a Application> {
    let wanted = wanted.to_lowercase();
    let lowered = |a: &Application| a.name.as_deref().map(str::to_lowercase);
    let exact: Vec<_> = applications
        .iter()
        .filter(|a| lowered(a).is_some_and(|name| name == wanted))
        .collect();
    if !exact.is_empty() || wanted.is_empty() {
        return exact;
    }
    applications
        .iter()
        .filter(|a| lowered(a).is_some_and(|name| name.contains(&wanted)))
        .collect()
}

/// Lists the applications and picks those matching `wanted`; no match is
/// `app-not-found` with the names that were seen, or `timeout` when an
/// application did not answer; a registry that cannot be read is `unavailable`.
pub async fn find(
    connection: &Connection,
    wanted: &str,
    per_application: Duration,
    progress: &Progress,
) -> Result<Vec<Application>, Finish> {
    let all = applications(connection, per_application, progress)
        .await
        .map_err(|e| Finish::with_error(Outcome::Unavailable, e))?;
    let matched: Vec<Application> = matching(&all, wanted).into_iter().cloned().collect();
    if matched.is_empty() {
        let seen: Vec<&str> = all
            .iter()
            .map(|a| a.name.as_deref().unwrap_or(""))
            .collect();
        progress.put("applications_seen", &seen);
        progress.put(
            "applications_unanswered",
            all.iter().filter(|a| a.name.is_none()).count(),
        );
        // An application that did not give its name may be the wanted one:
        // its silence is a timeout, not proof of absence.
        let unanswered = all.iter().filter(|a| a.name.is_none()).count();
        if unanswered > 0 {
            return Err(Finish::with_error(
                Outcome::Timeout,
                format!(
                    "no application matching {wanted:?} was found and {unanswered} of {} \
                     registered applications did not answer within {} s",
                    all.len(),
                    per_application.as_secs_f64()
                ),
            ));
        }
        return Err(Finish::with_error(
            Outcome::AppNotFound,
            format!("no registered application matches {wanted:?}"),
        ));
    }
    progress.put(TARGET_KEY, &matched);
    Ok(matched)
}
