//! Missing readings stay missing. Receipt-to-draw is never acceptance latency.
use crate::{
    Evidence,
    protocol::{Backend, Mode, Origin},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io,
    path::Path,
};

pub const MEMORY_LIMIT: u64 = 100_000_000;
pub const LATENCY_LIMIT_NS: u64 = 300_000_000;
pub const WARM_TRIALS: usize = 30;

pub fn monotonic_ns() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // CLOCK_MONOTONIC is available on the Linux-only experimental platform.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) },
        0
    );
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// A certificate must be supported by a current backend trace/source observation.
/// Closeness to receipt alone does not validate an undefined portal clock base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockMapping {
    pub portal_backend: String,
    pub backend_version: String,
    pub evidence_path: String,
    pub evidence_sha256: String,
    pub verified: bool,
    pub portal_clock_id: i32,
    pub presentation_clock_id: i32,
    pub portal_to_presentation_offset_ns: i64,
    pub uncertainty_ns: u64,
    pub boot_id: String,
    pub session_type: String,
    pub shell_version: String,
    pub portal_version: String,
    pub portal_timestamp_bits: u8,
}
impl ClockMapping {
    #[cfg(feature = "broker")]
    pub fn validate_environment(
        &self,
        environment: &addict_probe::record::Environment,
    ) -> Result<(), String> {
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
    pub fn latency(
        &self,
        portal_ms: u64,
        presentation_ns: u64,
        clock_id: i32,
        receipt_ns: u64,
    ) -> Result<u64, String> {
        if !self.verified
            || self.portal_backend.is_empty()
            || self.backend_version.is_empty()
            || self.evidence_path.is_empty()
            || self.evidence_sha256.len() != 64
            || !self.evidence_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || self.portal_clock_id != libc::CLOCK_MONOTONIC
            || self.presentation_clock_id != clock_id
            || clock_id != libc::CLOCK_MONOTONIC
            || self.uncertainty_ns > 1_000_000
        {
            return Err(
                "clock mapping is unverified/unsupported or uncertainty exceeds 1 ms".into(),
            );
        }
        let receipt_ms = receipt_ns / 1_000_000;
        let mapped_ms = match self.portal_timestamp_bits {
            64 => portal_ms,
            32 if portal_ms <= u32::MAX as u64 => {
                let epoch = receipt_ms & !u64::from(u32::MAX);
                let value = epoch | portal_ms;
                if value > receipt_ms.saturating_add(1) {
                    value
                        .checked_sub(1u64 << 32)
                        .ok_or("invalid wrapped timestamp")?
                } else {
                    value
                }
            }
            _ => return Err("unsupported portal timestamp width".into()),
        };
        let activation =
            i128::from(mapped_ms) * 1_000_000 + i128::from(self.portal_to_presentation_offset_ns);
        // Detect an invalid base and impossible order. Never reinterpret wall time.
        let receipt = i128::from(receipt_ns);
        if activation < 0
            || activation > receipt + i128::from(self.uncertainty_ns)
            || receipt - activation > 10_000_000_000
        {
            return Err("activation clock disagrees with the receipt clock".into());
        }
        let elapsed = i128::from(presentation_ns) - activation;
        if !(0..=30_000_000_000).contains(&elapsed) {
            return Err("invalid presentation order/latency".into());
        }
        // Upper bound includes uncertainty + millisecond timestamp quantization.
        u64::try_from(elapsed + i128::from(self.uncertainty_ns) + 1_000_000)
            .map_err(|_| "latency overflow".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMetric {
    pub pid: u32,
    pub name: Option<String>,
    pub pss_bytes: Option<u64>,
    pub rss_bytes: Option<u64>,
    pub cpu_ticks: Option<u64>,
    pub error: Option<String>,
}
pub fn parse_kib(text: &str, key: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|line| line.starts_with(&format!("{key}:")))?;
    let mut parts = line.split_whitespace();
    parts.next()?;
    let count: u64 = parts.next()?.parse().ok()?;
    (parts.next()? == "kB")
        .then(|| count.checked_mul(1024))
        .flatten()
}
pub fn process(pid: u32) -> ProcessMetric {
    let base = format!("/proc/{pid}");
    let memory = std::fs::read_to_string(format!("{base}/smaps_rollup"));
    let stat = std::fs::read_to_string(format!("{base}/stat")).ok();
    let cpu_ticks = stat
        .as_ref()
        .and_then(|s| s.rsplit_once(')'))
        .and_then(|(_, rest)| {
            let fields: Vec<_> = rest.split_whitespace().collect();
            fields
                .get(11)?
                .parse::<u64>()
                .ok()?
                .checked_add(fields.get(12)?.parse().ok()?)
        });
    let pss_bytes = memory.as_ref().ok().and_then(|s| parse_kib(s, "Pss"));
    let rss_bytes = memory.as_ref().ok().and_then(|s| parse_kib(s, "Rss"));
    ProcessMetric {
        pid,
        name: std::fs::read_to_string(format!("{base}/comm"))
            .ok()
            .map(|s| s.trim().into()),
        pss_bytes,
        rss_bytes,
        cpu_ticks,
        error: memory.err().map(|e| e.to_string()).or_else(|| {
            (pss_bytes.is_none() || rss_bytes.is_none() || cpu_ticks.is_none())
                .then(|| "missing process metric".into())
        }),
    }
}

/// Include all application descendants and any explicitly supplied resident helper.
pub fn descendants(roots: &[u32]) -> io::Result<Vec<u32>> {
    let mut parent = HashMap::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat"))
            && let Some((_, rest)) = stat.rsplit_once(')')
            && let Some(ppid) = rest
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u32>().ok())
        {
            parent.insert(pid, ppid);
        }
    }
    let mut included: HashSet<u32> = roots.iter().copied().collect();
    loop {
        let before = included.len();
        for (&pid, ppid) in &parent {
            if included.contains(ppid) {
                included.insert(pid);
            }
        }
        if before == included.len() {
            break;
        }
    }
    let mut result: Vec<_> = included.into_iter().collect();
    result.sort_unstable();
    Ok(result)
}
#[derive(Debug, Serialize, Deserialize)]
pub struct MemorySample {
    pub processes: Vec<ProcessMetric>,
    pub pss_bytes: Option<u64>,
    pub rss_bytes: Option<u64>,
    pub cpu_percent_one_core: Option<f64>,
    pub idle_seconds: u64,
    pub gpu_bytes: Option<u64>,
    pub gpu_limitation: String,
    pub inventory_complete: bool,
}
pub async fn sample(
    roots: &[u32],
    idle_seconds: u64,
    inventory_complete: bool,
) -> io::Result<MemorySample> {
    tokio::time::sleep(std::time::Duration::from_secs(idle_seconds)).await;
    let pids = descendants(roots)?;
    let before: Vec<_> = pids.iter().map(|&pid| process(pid)).collect();
    let start = monotonic_ns();
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let processes: Vec<_> = pids.iter().map(|&pid| process(pid)).collect();
    let inventory_complete = inventory_complete && descendants(roots)? == pids;
    let sum = |rss: bool| {
        processes.iter().try_fold(0u64, |sum, p| {
            sum.checked_add(if rss { p.rss_bytes? } else { p.pss_bytes? })
        })
    };
    let pss_bytes = sum(false);
    let rss_bytes = sum(true);
    let ticks = processes.iter().zip(before).try_fold(0u64, |sum, (a, b)| {
        sum.checked_add(a.cpu_ticks?.checked_sub(b.cpu_ticks?)?)
    });
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let cpu_percent_one_core = ticks
        .filter(|_| hz > 0)
        .map(|t| t as f64 / hz as f64 / ((monotonic_ns() - start) as f64 / 1e9) * 100.0);
    Ok(MemorySample {
        processes,
        pss_bytes,
        rss_bytes,
        cpu_percent_one_core,
        idle_seconds,
        gpu_bytes: None,
        gpu_limitation:
            "GPU process accounting is not available; collect driver-specific evidence separately"
                .into(),
        inventory_complete,
    })
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub backend: Backend,
    pub mode: Mode,
    pub origin: Origin,
    pub vulkan_drivers: Option<String>,
    pub warm_attempts: usize,
    pub extra_attempts: usize,
    pub max_extra_ms: Option<f64>,
    pub valid_latency_samples: usize,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub latency_status: String,
    pub memory_status: String,
    pub acceptance: String,
}
/// With a certificate, a presentation that carries no `acceptance_latency_ns`
/// gets one computed from its raw values; without one nothing is computed.
pub fn summarize(records: &[Evidence], mapping: Option<&ClockMapping>) -> Vec<Summary> {
    let mut result = Vec::new();
    for backend in [Backend::Gpui, Backend::Gtk4] {
        for mode in [Mode::Resident, Mode::OnDemand] {
            for origin in [Origin::Foliate, Origin::IntelliJ] {
                // Sessions with a restricted Vulkan loader are a separate row, never a replacement.
                let mut variants = vec![None];
                for started in records.iter().filter(|r| {
                    r.event == "broker-started"
                        && r.backend == backend
                        && r.mode == mode
                        && r.origin == origin
                }) {
                    let variant = started.details["vulkan_drivers"].as_str();
                    if !variants.contains(&variant) {
                        variants.push(variant);
                    }
                }
                for variant in variants {
                    let session = latest_session(records, |started, _| {
                        started.backend == backend
                            && started.mode == mode
                            && started.origin == origin
                            && started.details["vulkan_drivers"].as_str() == variant
                    });
                    // A certificate speaks for one boot: a session whose start names no
                    // boot id, or another one, gets no computed latency.
                    let certificate = mapping.filter(|mapping| {
                        session
                            .first()
                            .filter(|started| started.event == "broker-started")
                            .and_then(|started| started.details["boot_id"].as_str())
                            == Some(mapping.boot_id.as_str())
                    });
                    let rows: Vec<_> = session
                        .iter()
                        .filter(|r| {
                            r.schema == crate::SCHEMA
                                && r.backend == backend
                                && r.mode == mode
                                && r.origin == origin
                        })
                        .collect();
                    let mut attempts = Vec::new();
                    for id in rows
                        .iter()
                        .filter(|r| r.event == "invocation" && r.details["cold"] == false)
                        .filter_map(|r| r.invocation_id.as_deref())
                    {
                        if !attempts.contains(&id) {
                            attempts.push(id);
                        }
                    }
                    // The trial is the first 30 warm activations; later presses belong to the
                    // manual checks, but a violation observed there is still a violation.
                    let mut values = Vec::new();
                    let mut extra_values = Vec::new();
                    let mut duplicate = false;
                    for (index, id) in attempts.iter().enumerate() {
                        let samples: Vec<_> = rows
                            .iter()
                            .filter(|r| {
                                r.event == "presentation" && r.invocation_id.as_deref() == Some(*id)
                            })
                            .collect();
                        if samples.len() != 1 {
                            duplicate |= samples.len() > 1 && index < WARM_TRIALS;
                            continue;
                        }
                        // A latency already in the record is never recomputed, even a null one.
                        let latency = match samples[0].details.get("acceptance_latency_ns") {
                            Some(recorded) => recorded.as_u64(),
                            None => certificate.and_then(|mapping| {
                                computed_latency(mapping, &rows, id, samples[0])
                            }),
                        };
                        if samples[0].details["usable"] == true
                            && let Some(value) = latency
                        {
                            if index < WARM_TRIALS {
                                values.push(value);
                            } else {
                                extra_values.push(value);
                            }
                        }
                    }
                    values.sort_unstable();
                    let trial_attempts = attempts.len().min(WARM_TRIALS);
                    let max_extra = extra_values.iter().copied().max();
                    let percentile = |p: usize| {
                        if values.is_empty() {
                            None
                        } else {
                            Some(
                                values[(values.len() * p).div_ceil(100).saturating_sub(1)] as f64
                                    / 1e6,
                            )
                        }
                    };
                    let latency_status = if values
                        .iter()
                        .chain(&extra_values)
                        .any(|&v| v > LATENCY_LIMIT_NS)
                    {
                        "fail"
                    } else if duplicate
                        || trial_attempts != WARM_TRIALS
                        || values.len() != WARM_TRIALS
                    {
                        "incomplete"
                    } else {
                        "pass"
                    };
                    let mode_rows: Vec<_> = latest_session(records, |started, session| {
                        started.backend == backend
                            && started.mode == mode
                            && started.details["vulkan_drivers"].as_str() == variant
                            && session.iter().any(|r| r.event == "memory")
                    })
                    .iter()
                    .filter(|r| r.schema == crate::SCHEMA && r.backend == backend && r.mode == mode)
                    .collect();
                    let mut memory_status = "pass";
                    for phase in ["before-invocation", "after-close", "after-ocr"] {
                        let samples: Vec<_> = mode_rows
                            .iter()
                            .enumerate()
                            .filter(|(_, r)| r.event == "memory" && r.details["phase"] == phase)
                            .collect();
                        if samples.is_empty() {
                            if memory_status != "fail" {
                                memory_status = "incomplete";
                            }
                            continue;
                        }
                        for (index, row) in samples {
                            let sample = &row.details["sample"];
                            if sample["inventory_complete"] == true
                                && sample["idle_seconds"].as_u64().unwrap_or(0) >= 60
                                && sample["pss_bytes"]
                                    .as_u64()
                                    .is_some_and(|pss| pss > MEMORY_LIMIT)
                            {
                                memory_status = "fail";
                            } else if memory_status != "fail"
                                && ((phase == "after-ocr" && !ocr_precedes(&mode_rows, index))
                                || sample["inventory_complete"] != true
                                // A resident sample without its frontend measured the broker alone.
                                || (mode == Mode::Resident
                                    && sample["processes"].as_array().map_or(0, Vec::len) < 2)
                                || sample["idle_seconds"].as_u64().unwrap_or(0) < 60
                                || sample["pss_bytes"].as_u64().is_none()
                                || sample["rss_bytes"].as_u64().is_none()
                                || sample["cpu_percent_one_core"].as_f64().is_none())
                            {
                                memory_status = "incomplete";
                            }
                        }
                    }
                    let human = rows
                        .iter()
                        .any(|r| r.event == "human-scenarios" && r.details["all_passed"] == true);
                    let acceptance = if latency_status == "fail" || memory_status == "fail" {
                        "fail"
                    } else if latency_status == "pass" && memory_status == "pass" && human {
                        "pass"
                    } else {
                        "incomplete"
                    };
                    result.push(Summary {
                        backend,
                        mode,
                        origin,
                        vulkan_drivers: variant.map(str::to_owned),
                        warm_attempts: trial_attempts,
                        extra_attempts: attempts.len() - trial_attempts,
                        max_extra_ms: max_extra.map(|v| v as f64 / 1e6),
                        valid_latency_samples: values.len(),
                        p50_ms: percentile(50),
                        p95_ms: percentile(95),
                        max_ms: values.last().map(|v| *v as f64 / 1e6),
                        latency_status: latency_status.into(),
                        memory_status: memory_status.into(),
                        acceptance: acceptance.into(),
                    });
                }
            }
        }
    }
    result
}
/// The broker's acceptance rule applied to raw records: the token was present
/// and consumed, the frame has a `wp_presentation` time with no limitation, and
/// the certificate maps the portal timestamp. Anything missing gives no latency.
fn computed_latency(
    mapping: &ClockMapping,
    rows: &[&Evidence],
    id: &str,
    presentation: &Evidence,
) -> Option<u64> {
    let of_invocation = |event: &'static str| {
        rows.iter()
            .filter(move |r| r.event == event && r.invocation_id.as_deref() == Some(id))
    };
    let invocation = &of_invocation("invocation").next()?.details;
    let consumed = of_invocation("frontend-event")
        .any(|r| r.details["event"] == "activated" && r.details["token_consumed"] == true);
    if invocation["activation_token_present"] != true || !consumed {
        return None;
    }
    let frame = &presentation.details;
    if !frame["limitation"].is_null() {
        return None;
    }
    mapping
        .latency(
            invocation["portal_timestamp_ms"].as_u64()?,
            frame["time_ns"].as_u64()?,
            i32::try_from(frame["clock_id"].as_i64()?).ok()?,
            invocation["receipt_monotonic_ns"].as_u64()?,
        )
        .ok()
}
// A repeated session supersedes earlier ones for the same cell; evidence stays append-only.
// Records without any `broker-started` are treated as one session.
fn latest_session(
    records: &[Evidence],
    matches: impl Fn(&Evidence, &[Evidence]) -> bool,
) -> &[Evidence] {
    let starts: Vec<_> = records
        .iter()
        .enumerate()
        .filter(|(_, r)| r.event == "broker-started")
        .map(|(index, _)| index)
        .collect();
    if starts.is_empty() {
        return records;
    }
    for (position, &start) in starts.iter().enumerate().rev() {
        let end = starts.get(position + 1).copied().unwrap_or(records.len());
        if matches(&records[start], &records[start..end]) {
            return &records[start..end];
        }
    }
    &[]
}
pub fn read_evidence(path: &Path) -> io::Result<Vec<Evidence>> {
    let text = std::fs::read_to_string(path)?;
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|line| serde_json::from_str(line).map_err(io::Error::from))
        .collect()
}

// Evidence is appended in observation order; a memory sample must follow a
// completed OCR cycle for one invocation, including its worker start.
fn ocr_precedes(rows: &[&Evidence], sample_index: usize) -> bool {
    rows[..sample_index]
        .iter()
        .enumerate()
        .any(|(completed_index, completed)| {
            let Some(id) = completed.invocation_id.as_deref() else {
                return false;
            };
            completed.event == "frontend-event"
                && completed.details["event"] == "work"
                && completed.details["outcome"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("ocr-completed:"))
                && rows[..completed_index].iter().any(|started| {
                    started.invocation_id.as_deref() == Some(id)
                        && started.event == "frontend-event"
                        && started.details["event"] == "work"
                        && started.details["outcome"] == "ocr-started"
                        && started.details["child_pid"].as_u64().is_some()
                })
        })
}

#[cfg(test)]
mod certificate_tests {
    use super::*;
    use serde_json::{Value, json};

    fn mapping() -> ClockMapping {
        serde_json::from_value(json!({"portal_backend":"gnome","backend_version":"50","evidence_path":"proof","evidence_sha256":"a".repeat(64),"verified":true,"portal_clock_id":1,"presentation_clock_id":1,"portal_to_presentation_offset_ns":0,"uncertainty_ns":0,"boot_id":"boot","session_type":"wayland","shell_version":"50","portal_version":"1","portal_timestamp_bits":64})).unwrap()
    }
    fn row(event: &str, id: Option<&str>, details: Value) -> Evidence {
        Evidence::new(
            Backend::Gtk4,
            Mode::Resident,
            Origin::Foliate,
            id.map(str::to_owned),
            event,
            details,
        )
    }
    /// One session with one warm activation as the product records it: raw
    /// values only, no latency in the presentation.
    fn session(boot_id: Value, token: bool) -> Vec<Evidence> {
        vec![
            row("broker-started", None, json!({"pid":1,"boot_id":boot_id})),
            row(
                "invocation",
                Some("a"),
                json!({"cold":false,"portal_timestamp_ms":1000,"receipt_monotonic_ns":1_000_000_000u64,"activation_token_present":token}),
            ),
            row(
                "frontend-event",
                Some("a"),
                json!({"event":"activated","id":"a","token_consumed":token}),
            ),
            row(
                "presentation",
                Some("a"),
                json!({"event":"presented","id":"a","time_ns":1_100_000_000u64,"clock_id":1,"usable":true,"limitation":null}),
            ),
        ]
    }
    fn foliate(rows: &[Evidence], mapping: Option<&ClockMapping>) -> Summary {
        summarize(rows, mapping)
            .into_iter()
            .find(|s| {
                s.backend == Backend::Gtk4
                    && s.mode == Mode::Resident
                    && s.origin == Origin::Foliate
            })
            .unwrap()
    }

    #[test]
    fn a_certificate_of_the_same_boot_computes_the_latency() {
        let rows = session(json!("boot"), true);
        let s = foliate(&rows, Some(&mapping()));
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (1, 1));
        // 100 ms between activation and frame, plus 1 ms of quantization.
        assert_eq!(s.max_ms, Some(101.0));
        assert_eq!(foliate(&rows, None).valid_latency_samples, 0);
    }

    #[test]
    fn a_recorded_latency_is_never_recomputed() {
        let mut rows = session(json!("boot"), true);
        rows[3].details["acceptance_latency_ns"] = json!(7_000_000);
        assert_eq!(foliate(&rows, Some(&mapping())).max_ms, Some(7.0));
        // The broker records its refusal as null: it stays a refusal.
        rows[3].details["acceptance_latency_ns"] = Value::Null;
        assert_eq!(foliate(&rows, Some(&mapping())).valid_latency_samples, 0);
    }

    #[test]
    fn another_or_unknown_boot_gets_no_latency() {
        for boot_id in [json!("other"), Value::Null] {
            let s = foliate(&session(boot_id, true), Some(&mapping()));
            assert_eq!((s.warm_attempts, s.valid_latency_samples), (1, 0));
            assert_eq!(s.latency_status, "incomplete");
        }
        // Records with no session start at all name no boot either.
        let rows = &session(json!("boot"), true)[1..];
        assert_eq!(foliate(rows, Some(&mapping())).valid_latency_samples, 0);
    }

    #[test]
    fn a_token_that_is_absent_or_not_consumed_gets_no_latency() {
        let s = foliate(&session(json!("boot"), false), Some(&mapping()));
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (1, 0));
        let mut rows = session(json!("boot"), true);
        rows[2].details["token_consumed"] = json!(false);
        assert_eq!(foliate(&rows, Some(&mapping())).valid_latency_samples, 0);
    }

    #[test]
    fn a_frame_without_a_time_or_with_a_limitation_gets_no_latency() {
        let mut rows = session(json!("boot"), true);
        rows[3].details["limitation"] = json!("wp_presentation frame discarded");
        assert_eq!(foliate(&rows, Some(&mapping())).valid_latency_samples, 0);
        let mut rows = session(json!("boot"), true);
        rows[3].details["time_ns"] = Value::Null;
        assert_eq!(foliate(&rows, Some(&mapping())).valid_latency_samples, 0);
    }
}
