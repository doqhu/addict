//! What the host reports about itself, and the check that a clock certificate
//! was made on this host as it is now.
use super::metrics::ClockMapping;
use serde::Serialize;
use std::{future::Future, time::Duration};

/// What the machine reported about itself. A value the machine did not give
/// is `null`, never a guess.
#[derive(Debug, Clone, Serialize)]
pub struct Environment {
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub session_type: Option<String>,
    pub current_desktop: Option<String>,
    pub wayland_display: Option<String>,
    pub gnome_shell: Option<String>,
    pub xdg_desktop_portal: Option<String>,
    pub xdg_desktop_portal_gnome: Option<String>,
    pub wl_paste: Option<String>,
}

/// Reads the environment. Every external command is bounded by `limit`; a
/// command that is missing, fails or overruns leaves its field `null`.
pub async fn capture_environment(limit: Duration) -> Environment {
    let (gnome_shell, portal, portal_gnome, wl_paste, session_type) = tokio::join!(
        command_line(limit, "gnome-shell", &["--version"]),
        package_version(limit, "xdg-desktop-portal"),
        package_version(limit, "xdg-desktop-portal-gnome"),
        command_line(limit, "wl-paste", &["--version"]),
        session_type(limit),
    );
    Environment {
        os: os_pretty_name(),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_owned()),
        session_type,
        current_desktop: env_var("XDG_CURRENT_DESKTOP"),
        wayland_display: env_var("WAYLAND_DISPLAY"),
        gnome_shell,
        xdg_desktop_portal: portal,
        xdg_desktop_portal_gnome: portal_gnome,
        wl_paste,
    }
}

impl ClockMapping {
    /// The certificate speaks for this boot, these package versions and a
    /// Wayland session, and its evidence text is the one it was made from.
    pub fn validate_environment(&self, environment: &Environment) -> Result<(), String> {
        use sha2::{Digest, Sha256};
        if environment.xdg_desktop_portal_gnome.as_deref() != Some(self.backend_version.as_str())
            || environment.gnome_shell.as_deref() != Some(self.shell_version.as_str())
            || environment.xdg_desktop_portal.as_deref() != Some(self.portal_version.as_str())
            || environment.session_type.as_deref() != Some(self.session_type.as_str())
            || self.session_type != "wayland"
            || std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map_err(|e| e.to_string())?
                .trim()
                != self.boot_id
        {
            return Err(
                "clock certificate does not match current boot/backend/portal/Shell/session".into(),
            );
        }
        let evidence = std::fs::read(&self.evidence_path).map_err(|e| e.to_string())?;
        if evidence.is_empty() || format!("{:x}", Sha256::digest(evidence)) != self.evidence_sha256
        {
            return Err("clock evidence hash does not match certificate".into());
        }
        Ok(())
    }
}

/// The installed version of `package`, from the first package manager that
/// knows it: rpm, then dpkg, then pacman. Each is asked as its own command,
/// so a missing one costs nothing.
async fn package_version(limit: Duration, package: &str) -> Option<String> {
    let rpm = ["-q", "--qf", "%{VERSION}-%{RELEASE}", package];
    let dpkg = ["-W", "-f", "${Version}", package];
    let pacman = ["-Q", package];
    first_version(
        limit,
        package,
        &[("rpm", &rpm), ("dpkg-query", &dpkg), ("pacman", &pacman)],
    )
    .await
}

/// The version of `package` the first of `queries` gives.
async fn first_version(
    limit: Duration,
    package: &str,
    queries: &[(&str, &[&str])],
) -> Option<String> {
    for (program, args) in queries {
        if let Some(version) = command_line(limit, program, args)
            .await
            .and_then(|line| version_of(&line, package))
        {
            return Some(version);
        }
    }
    None
}

/// The version in a line that is a version alone (rpm, dpkg) or the name of
/// `package` and its version (pacman); a line of another package gives none.
fn version_of(line: &str, package: &str) -> Option<String> {
    let version = line
        .strip_prefix(package)
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or(line)
        .trim();
    (!version.is_empty() && !version.contains(' ')).then(|| version.to_owned())
}

/// `XDG_SESSION_TYPE`, or else the `Type` logind gives the session of
/// `XDG_SESSION_ID`, or of the user's display session without it: a shell
/// started outside the session lacks both variables.
async fn session_type(limit: Duration) -> Option<String> {
    session_type_of(
        limit,
        env_var("XDG_SESSION_TYPE"),
        env_var("XDG_SESSION_ID"),
    )
    .await
}

/// The session type `variable` names, or else logind's of session `id`
/// (`auto`, the caller's or the user's display session, without one).
async fn session_type_of(
    limit: Duration,
    variable: Option<String>,
    id: Option<String>,
) -> Option<String> {
    if variable.is_some() {
        return variable;
    }
    let id = id.unwrap_or_else(|| "auto".into());
    command_line(
        limit,
        "loginctl",
        &["show-session", &id, "--property=Type", "--value"],
    )
    .await
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn os_pretty_name() -> Option<String> {
    let text = std::fs::read_to_string("/etc/os-release").ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .filter(|value| !value.is_empty())
}

/// Runs `future` for at most `limit`; `None` when the limit passed. The
/// future is dropped then, so nothing keeps waiting behind the caller's back.
pub(super) async fn bounded<F: Future>(limit: Duration, future: F) -> Option<F::Output> {
    tokio::time::timeout(limit, future).await.ok()
}

/// First line of a successful command's stdout.
async fn command_line(limit: Duration, program: &str, args: &[&str]) -> Option<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    match bounded(limit, command.output()).await {
        Some(Ok(output)) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// The host as the certificate below names it.
    fn host() -> Environment {
        Environment {
            os: None,
            kernel: None,
            session_type: Some("wayland".into()),
            current_desktop: None,
            wayland_display: None,
            gnome_shell: Some("GNOME Shell 50".into()),
            xdg_desktop_portal: Some("1.20".into()),
            xdg_desktop_portal_gnome: Some("50".into()),
            wl_paste: None,
        }
    }

    /// A certificate of this boot and of `host()`, made from `evidence`.
    fn certificate(evidence: &std::path::Path) -> ClockMapping {
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
        ClockMapping {
            portal_backend: "gnome".into(),
            backend_version: "50".into(),
            evidence_path: evidence.to_string_lossy().into_owned(),
            evidence_sha256: format!("{:x}", Sha256::digest(std::fs::read(evidence).unwrap())),
            verified: true,
            portal_clock_id: 1,
            presentation_clock_id: 1,
            portal_to_presentation_offset_ns: 0,
            uncertainty_ns: 0,
            boot_id: boot.trim().to_owned(),
            session_type: "wayland".into(),
            shell_version: "GNOME Shell 50".into(),
            portal_version: "1.20".into(),
            portal_timestamp_bits: 64,
        }
    }

    #[test]
    fn a_line_gives_the_version_of_its_package_only() {
        let portal = "xdg-desktop-portal";
        assert_eq!(
            version_of("1.20.3-1.fc44", portal).as_deref(),
            Some("1.20.3-1.fc44")
        );
        assert_eq!(
            version_of("xdg-desktop-portal 1.20.3-1", portal).as_deref(),
            Some("1.20.3-1")
        );
        assert_eq!(version_of("xdg-desktop-portal-gnome 50.0-1", portal), None);
        assert_eq!(version_of("xdg-desktop-portal ", portal), None);
        assert_eq!(
            version_of("package xdg-desktop-portal is not installed", portal),
            None
        );
    }

    /// A missing or failing package manager passes to the next one; when
    /// none knows the package, the version is unknown.
    #[test]
    fn the_first_package_manager_that_knows_the_package_gives_its_version() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limit = Duration::from_secs(2);
        let portal = "xdg-desktop-portal";
        let found = runtime.block_on(first_version(
            limit,
            portal,
            &[
                ("addict-no-such-program", &[]),
                ("false", &[]),
                ("echo", &["xdg-desktop-portal 1.20.3-1"]),
                ("echo", &["9.9"]),
            ],
        ));
        assert_eq!(found.as_deref(), Some("1.20.3-1"));
        let none = runtime.block_on(first_version(
            limit,
            portal,
            &[("addict-no-such-program", &[]), ("false", &[])],
        ));
        assert_eq!(none, None);
    }

    /// The variable wins; a session logind does not know gives no type.
    #[test]
    fn the_session_type_falls_back_to_logind() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let limit = Duration::from_secs(2);
        let of = |variable: Option<&str>, id: Option<&str>| {
            runtime.block_on(session_type_of(
                limit,
                variable.map(str::to_owned),
                id.map(str::to_owned),
            ))
        };
        assert_eq!(of(Some("x11"), Some("1")).as_deref(), Some("x11"));
        assert_eq!(of(None, Some("no-such-session")), None);
    }

    #[test]
    fn a_certificate_of_this_boot_and_host_validates_and_no_other() {
        let path =
            std::env::temp_dir().join(format!("addict-harness-clock-{}", std::process::id()));
        std::fs::write(&path, "clock evidence\n").unwrap();
        let mapping = certificate(&path);
        let same = mapping.validate_environment(&host());
        let other_boot = ClockMapping {
            boot_id: "another-boot".into(),
            ..mapping.clone()
        }
        .validate_environment(&host());
        let other_shell = mapping.validate_environment(&Environment {
            gnome_shell: Some("GNOME Shell 51".into()),
            ..host()
        });
        std::fs::write(&path, "clock evidence, changed\n").unwrap();
        let changed = mapping.validate_environment(&host());
        std::fs::remove_file(&path).unwrap();

        assert_eq!(same, Ok(()));
        assert!(other_boot.is_err());
        assert!(other_shell.is_err());
        assert_eq!(
            changed,
            Err("clock evidence hash does not match certificate".into())
        );
    }
}
