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
    let (gnome_shell, portal, portal_gnome, wl_paste) = tokio::join!(
        command_line(limit, "gnome-shell", &["--version"]),
        command_line(
            limit,
            "rpm",
            &["-q", "--qf", "%{VERSION}-%{RELEASE}", "xdg-desktop-portal"]
        ),
        command_line(
            limit,
            "rpm",
            &[
                "-q",
                "--qf",
                "%{VERSION}-%{RELEASE}",
                "xdg-desktop-portal-gnome"
            ]
        ),
        command_line(limit, "wl-paste", &["--version"]),
    );
    Environment {
        os: os_pretty_name(),
        kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|s| s.trim().to_owned()),
        session_type: env_var("XDG_SESSION_TYPE"),
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
async fn bounded<F: Future>(limit: Duration, future: F) -> Option<F::Output> {
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
