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
                    let certificate = certificate_for(session, mapping);
                    let rows: Vec<_> = session
                        .iter()
                        .filter(|r| {
                            r.schema == crate::SCHEMA
                                && r.backend == backend
                                && r.mode == mode
                                && r.origin == origin
                        })
                        .collect();
                    let Latency {
                        attempts,
                        trial,
                        values,
                        max_extra,
                        status: latency_status,
                    } = latency(&rows, certificate);
                    let trial_attempts = trial.len();
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
                        extra_attempts: attempts - trial_attempts,
                        max_extra_ms: max_extra.map(|v| v as f64 / 1e6),
                        valid_latency_samples: values.len(),
                        p50_ms: percentile(&values, 50),
                        p95_ms: percentile(&values, 95),
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
/// A certificate speaks for one boot: a session whose start names no boot id,
/// or another one, gets no computed latency.
fn certificate_for<'a>(
    session: &[Evidence],
    mapping: Option<&'a ClockMapping>,
) -> Option<&'a ClockMapping> {
    mapping.filter(|mapping| {
        session
            .first()
            .filter(|started| started.event == "broker-started")
            .and_then(|started| started.details["boot_id"].as_str())
            == Some(mapping.boot_id.as_str())
    })
}
/// The warm activations of one session's rows, judged against the threshold.
struct Latency<'a> {
    /// Every warm activation, the trial and what followed it.
    attempts: usize,
    /// The ids of the trial: the first 30 warm activations.
    trial: Vec<&'a str>,
    /// The latencies of the trial, ascending.
    values: Vec<u64>,
    max_extra: Option<u64>,
    status: &'static str,
}
fn latency<'a>(rows: &[&'a Evidence], certificate: Option<&ClockMapping>) -> Latency<'a> {
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
            .filter(|r| r.event == "presentation" && r.invocation_id.as_deref() == Some(*id))
            .collect();
        if samples.len() != 1 {
            duplicate |= samples.len() > 1 && index < WARM_TRIALS;
            continue;
        }
        // A latency already in the record is never recomputed, even a null one.
        let latency = match samples[0].details.get("acceptance_latency_ns") {
            Some(recorded) => recorded.as_u64(),
            None => certificate.and_then(|mapping| computed_latency(mapping, rows, id, samples[0])),
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
    let status = if values
        .iter()
        .chain(&extra_values)
        .any(|&v| v > LATENCY_LIMIT_NS)
    {
        "fail"
    } else if duplicate || trial_attempts != WARM_TRIALS || values.len() != WARM_TRIALS {
        "incomplete"
    } else {
        "pass"
    };
    Latency {
        attempts: attempts.len(),
        trial: attempts[..trial_attempts].to_vec(),
        values,
        max_extra: extra_values.iter().copied().max(),
        status,
    }
}
/// Nearest-rank percentile of ascending `values`, in milliseconds.
fn percentile(values: &[u64], p: usize) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        Some(values[(values.len() * p).div_ceil(100).saturating_sub(1)] as f64 / 1e6)
    }
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

/// The product is one process: the GTK4 resident cell, pressed from these.
pub const PRODUCT_ORIGINS: [Origin; 2] = [Origin::Foliate, Origin::AndroidStudio];
/// The idle states the memory threshold is judged on. The state after an OCR
/// cycle belongs to the OCR epic and is reported as not measured.
pub const PRODUCT_PHASES: [&str; 2] = ["before-invocation", "after-close"];

#[derive(Debug, Serialize)]
pub struct ProductSummary {
    pub backend: Backend,
    pub mode: Mode,
    pub origin: Origin,
    pub warm_attempts: usize,
    pub extra_attempts: usize,
    pub max_extra_ms: Option<f64>,
    pub valid_latency_samples: usize,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub latency_status: String,
    /// The range over the samples that count, both idle states together.
    pub pss_min_bytes: Option<u64>,
    pub pss_max_bytes: Option<u64>,
    pub memory_status: String,
    pub idle_cpu_percent_max: Option<f64>,
    pub idle_cpu_status: String,
    /// The latest `gpu` record of the session; reported, not judged.
    pub gpu: Option<serde_json::Value>,
    pub not_measured: Vec<String>,
    pub acceptance: String,
}
/// The acceptance of the product, one row per application, from its latest
/// session. Unlike [`summarize`]: Android Studio has a row, one process is the
/// whole inventory, two idle states are judged, any idle CPU fails, and no
/// `human-scenarios` record is needed.
pub fn summarize_product(
    records: &[Evidence],
    mapping: Option<&ClockMapping>,
) -> Vec<ProductSummary> {
    let (backend, mode) = (Backend::Gtk4, Mode::Resident);
    PRODUCT_ORIGINS
        .into_iter()
        .map(|origin| {
            // The product names itself in its session start; a session of the
            // EXP-4 prototype in the same cell is not the product.
            let of_product = |r: &Evidence| {
                r.event == "broker-started"
                    && r.backend == backend
                    && r.mode == mode
                    && r.origin == origin
                    && r.details["application"] == "addict"
            };
            let session = latest_session(records, |started, _| of_product(started));
            // Records with no session start belong to no process of the product.
            let started = session.first().filter(|r| of_product(r));
            let pid = started.and_then(|r| r.details["pid"].as_u64());
            let rows: Vec<_> = session
                .iter()
                .filter(|r| {
                    started.is_some()
                        && r.schema == crate::SCHEMA
                        && r.backend == backend
                        && r.mode == mode
                        && r.origin == origin
                })
                .collect();
            let latency = latency(&rows, certificate_for(session, mapping));
            let warm = |r: &Evidence| r.event == "invocation" && r.details["cold"] == false;
            let first_warm = rows.iter().position(|r| warm(r));
            let trial_end = rows.iter().rposition(|r| {
                matches!(r.event.as_str(), "invocation" | "presentation")
                    && r.invocation_id
                        .as_deref()
                        .is_some_and(|id| latency.trial.contains(&id))
            });

            let (mut memory_status, mut cpu_status) = ("pass", "pass");
            let mut pss = Vec::new();
            let mut cpu_max: Option<f64> = None;
            for phase in PRODUCT_PHASES {
                let mut found = false;
                for (index, row) in rows
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.event == "memory" && r.details["phase"] == phase)
                {
                    found = true;
                    let sample = &row.details["sample"];
                    // The sample is of this session's process, after a full idle
                    // wait, on the side of the trial its phase names.
                    let counts = sample["inventory_complete"] == true
                        && sample["idle_seconds"].as_u64().unwrap_or(0) >= 60
                        && pid.is_some()
                        && sample["processes"]
                            .as_array()
                            .is_some_and(|all| all.iter().any(|p| p["pid"].as_u64() == pid))
                        && if phase == "before-invocation" {
                            first_warm.is_none_or(|first| index < first)
                        } else {
                            trial_end.is_some_and(|end| index > end)
                        };
                    let sample_pss = sample["pss_bytes"].as_u64().filter(|_| counts);
                    let sample_cpu = sample["cpu_percent_one_core"].as_f64().filter(|_| counts);
                    pss.extend(sample_pss);
                    if let Some(cpu) = sample_cpu {
                        cpu_max = Some(cpu_max.map_or(cpu, |max| max.max(cpu)));
                    }
                    if sample_pss.is_some_and(|pss| pss > MEMORY_LIMIT) {
                        memory_status = "fail";
                    } else if memory_status != "fail"
                        && (sample_pss.is_none() || sample["rss_bytes"].as_u64().is_none())
                    {
                        memory_status = "incomplete";
                    }
                    if sample_cpu.is_some_and(|cpu| cpu > 0.0) {
                        cpu_status = "fail";
                    } else if cpu_status != "fail" && sample_cpu.is_none() {
                        cpu_status = "incomplete";
                    }
                }
                if !found {
                    if memory_status != "fail" {
                        memory_status = "incomplete";
                    }
                    if cpu_status != "fail" {
                        cpu_status = "incomplete";
                    }
                }
            }

            let gpu = rows
                .iter()
                .rev()
                .find(|r| r.event == "gpu")
                .map(|r| r.details.clone());
            let mut not_measured =
                vec!["memory after an OCR cycle: left to the OCR epic".to_owned()];
            if gpu
                .as_ref()
                .is_none_or(|g| g["gpu_bytes"].as_u64().is_none())
            {
                not_measured.push("GPU memory of the process".to_owned());
            }
            let statuses = [latency.status, memory_status, cpu_status];
            let acceptance = if statuses.contains(&"fail") {
                "fail"
            } else if statuses.iter().all(|&s| s == "pass") {
                "pass"
            } else {
                "incomplete"
            };
            ProductSummary {
                backend,
                mode,
                origin,
                warm_attempts: latency.trial.len(),
                extra_attempts: latency.attempts - latency.trial.len(),
                max_extra_ms: latency.max_extra.map(|v| v as f64 / 1e6),
                valid_latency_samples: latency.values.len(),
                p50_ms: percentile(&latency.values, 50),
                p95_ms: percentile(&latency.values, 95),
                max_ms: latency.values.last().map(|v| *v as f64 / 1e6),
                latency_status: latency.status.into(),
                pss_min_bytes: pss.iter().copied().min(),
                pss_max_bytes: pss.iter().copied().max(),
                memory_status: memory_status.into(),
                idle_cpu_percent_max: cpu_max,
                idle_cpu_status: cpu_status.into(),
                gpu,
                not_measured,
                acceptance: acceptance.into(),
            }
        })
        .collect()
}

/// Presses a calibration needs at least.
pub const CALIBRATION_PRESSES: usize = 10;
/// The receipt follows the portal timestamp by more than 0 and at most this.
pub const CALIBRATION_LIMIT_NS: i128 = 10_000_000;
/// What a calibrated certificate states; `ClockMapping::latency` adds it.
pub const CERTIFICATE_UNCERTAINTY_NS: u64 = 1_000_000;

pub fn clock_ns(clock: libc::clockid_t) -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // The three clocks read here always exist on Linux.
    assert_eq!(unsafe { libc::clock_gettime(clock, &mut time) }, 0);
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// What the calibrating host is at the moment of the calibration.
#[derive(Debug, Clone)]
pub struct CalibrationHost {
    pub boot_id: String,
    pub session_type: String,
    pub backend_version: String,
    pub shell_version: String,
    pub portal_version: String,
    /// The captured environment as one line of JSON, for the evidence text.
    pub environment: String,
    pub monotonic_ns: u64,
    pub boottime_ns: u64,
    pub realtime_ns: u64,
    /// Where the evidence text will be written; the certificate names it.
    pub evidence_path: String,
}
#[derive(Debug)]
pub struct Calibration {
    pub evidence: String,
    pub mapping: ClockMapping,
}
/// The certificate of this boot from the latest recorded session, or the
/// reason there is none. Verified means: at least [`CALIBRATION_PRESSES`]
/// presses of this boot, every receipt within (0, 10] ms after its portal
/// timestamp read as `CLOCK_MONOTONIC` milliseconds, every frame on clock 1.
pub fn calibrate(records: &[Evidence], host: &CalibrationHost) -> Result<Calibration, String> {
    use sha2::{Digest, Sha256};
    if host.session_type != "wayland" {
        return Err(format!(
            "the session is {:?}, not wayland",
            host.session_type
        ));
    }
    let start = records
        .iter()
        .rposition(|r| r.event == "broker-started")
        .ok_or("the records hold no session start")?;
    let session = &records[start..];
    let started = &session[0];
    if started.details["boot_id"].as_str() != Some(host.boot_id.as_str()) {
        return Err(format!(
            "the latest session was recorded on boot {}, this boot is {}",
            started.details["boot_id"], host.boot_id
        ));
    }
    let frames: Vec<_> = session
        .iter()
        .filter(|r| r.event == "presentation")
        .collect();
    if let Some(frame) = frames
        .iter()
        .find(|r| r.details["clock_id"].as_i64() != Some(i64::from(libc::CLOCK_MONOTONIC)))
    {
        return Err(format!(
            "the frame of {} is on clock {}, not on clock 1",
            frame.invocation_id.as_deref().unwrap_or("an unnamed press"),
            frame.details["clock_id"]
        ));
    }
    let mut lines = Vec::new();
    let mut differences = Vec::new();
    for press in session.iter().filter(|r| r.event == "invocation") {
        let id = press
            .invocation_id
            .as_deref()
            .ok_or("a press has no invocation id")?;
        let portal_ms = press.details["portal_timestamp_ms"]
            .as_u64()
            .ok_or_else(|| format!("{id} has no portal timestamp"))?;
        let receipt_ns = press.details["receipt_monotonic_ns"]
            .as_u64()
            .ok_or_else(|| format!("{id} has no receipt time"))?;
        if receipt_ns > host.monotonic_ns {
            return Err(format!("{id} was received after now: not this boot"));
        }
        if !frames
            .iter()
            .any(|r| r.invocation_id.as_deref() == Some(id))
        {
            return Err(format!("{id} has no frame"));
        }
        // Mutter event times are 32-bit milliseconds: the same mapping as
        // `ClockMapping::latency` applies for `portal_timestamp_bits` 32.
        if portal_ms > u64::from(u32::MAX) {
            return Err(format!("the portal timestamp of {id} exceeds 32 bits"));
        }
        let receipt_ms = receipt_ns / 1_000_000;
        let mut mapped_ms = (receipt_ms & !u64::from(u32::MAX)) | portal_ms;
        if mapped_ms > receipt_ms.saturating_add(1) && mapped_ms >= 1u64 << 32 {
            mapped_ms -= 1u64 << 32;
        }
        let difference = i128::from(receipt_ns) - i128::from(mapped_ms) * 1_000_000;
        if difference <= 0 || difference > CALIBRATION_LIMIT_NS {
            return Err(format!(
                "the receipt of {id} is {:.6} ms after its portal timestamp, outside (0, 10] ms",
                difference as f64 / 1e6
            ));
        }
        lines.push(format!(
            "{id} {portal_ms} {receipt_ns} {:.3}",
            difference as f64 / 1e6
        ));
        differences.push(difference);
    }
    if differences.len() < CALIBRATION_PRESSES {
        return Err(format!(
            "{} presses recorded, {CALIBRATION_PRESSES} are needed",
            differences.len()
        ));
    }

    let seconds = |ns: u64| format!("{}.{:06}", ns / 1_000_000_000, ns % 1_000_000_000 / 1000);
    let apart = |ns: u64| host.monotonic_ns.abs_diff(ns) as f64 / 1e9;
    let suspended = apart(host.boottime_ns) >= 1.0;
    let (min, max) = (
        *differences.iter().min().unwrap_or(&0) as f64 / 1e6,
        *differences.iter().max().unwrap_or(&0) as f64 / 1e6,
    );
    let evidence = format!(
        "Acceptance portal clock evidence (runtime corroboration)\n\
         collected: {date} UTC, addict pid {pid}, backend gtk4, mode resident, origin {origin}\n\
         boot_id: {boot}\n\
         environment: {environment}\n\
         \n\
         Clock readings taken together at the calibration (seconds):\n\
         CLOCK_MONOTONIC={monotonic} CLOCK_BOOTTIME={boottime} CLOCK_REALTIME={realtime}\n\
         BOOTTIME-MONOTONIC={gap:.3} s ({gap_reading})\n\
         \n\
         Raw GlobalShortcuts Activated timestamp (ms) against the application's CLOCK_MONOTONIC reading at signal receipt:\n\
         invocation_id portal_timestamp_ms receipt_monotonic_ns receipt_minus_portal_ms\n\
         {lines}\n\
         samples={count} min={min:.3} ms max={max:.3} ms; every difference is positive and at most 10 ms\n\
         \n\
         Reading: the portal timestamp is CLOCK_MONOTONIC in milliseconds: it fits within 10 ms on all samples.\n\
         {boottime_reading}\n\
         Interpreted as CLOCK_REALTIME it would be about {realtime_gap:.0} s away from receipt.\n\
         wp_presentation reported clock id 1 (CLOCK_MONOTONIC) for every frame of this session ({frames} frames), so offset is 0.\n\
         Every value fits in 32 bits; Mutter event times are 32-bit milliseconds, hence portal_timestamp_bits=32.\n\
         Limit of this evidence: it is a runtime corroboration on this boot and these package versions, marked\n\
         verified by the calibration rule of `addict-exp4 calibrate`. The GNOME source chain\n\
         (libinput -> Mutter -> Shell -> portal backend) was not inspected.\n",
        date = utc_date(host.realtime_ns / 1_000_000_000),
        pid = started.details["pid"],
        origin = serde_json::to_string(&started.origin)
            .unwrap_or_default()
            .trim_matches('"'),
        boot = host.boot_id,
        environment = host.environment,
        monotonic = seconds(host.monotonic_ns),
        boottime = seconds(host.boottime_ns),
        realtime = seconds(host.realtime_ns),
        gap = apart(host.boottime_ns),
        gap_reading = if suspended {
            "the host suspended during this boot, so the two clocks are distinguishable"
        } else {
            "the host has not suspended during this boot, so the two clocks are not distinguishable"
        },
        lines = lines.join("\n"),
        count = differences.len(),
        frames = frames.len(),
        boottime_reading = if suspended {
            format!(
                "Interpreted as CLOCK_BOOTTIME it would be {:.0} s away from receipt, which does not fit.",
                apart(host.boottime_ns)
            )
        } else {
            "This evidence cannot tell CLOCK_MONOTONIC from CLOCK_BOOTTIME. If the timestamp is CLOCK_BOOTTIME, after a\n\
             suspend an activation lies after its receipt and gets no latency, not a wrong one."
                .to_owned()
        },
        realtime_gap = apart(host.realtime_ns),
    );
    let mapping = ClockMapping {
        portal_backend: "gnome".into(),
        backend_version: host.backend_version.clone(),
        evidence_path: host.evidence_path.clone(),
        evidence_sha256: format!("{:x}", Sha256::digest(evidence.as_bytes())),
        verified: true,
        portal_clock_id: libc::CLOCK_MONOTONIC,
        presentation_clock_id: libc::CLOCK_MONOTONIC,
        portal_to_presentation_offset_ns: 0,
        uncertainty_ns: CERTIFICATE_UNCERTAINTY_NS,
        boot_id: host.boot_id.clone(),
        session_type: host.session_type.clone(),
        shell_version: host.shell_version.clone(),
        portal_version: host.portal_version.clone(),
        portal_timestamp_bits: 32,
    };
    Ok(Calibration { evidence, mapping })
}
/// The UTC calendar date of a Unix time, as `YYYY-MM-DD`.
fn utc_date(unix_seconds: u64) -> String {
    // Civil-from-days, with the era starting on 0000-03-01.
    let days = unix_seconds / 86_400 + 719_468;
    let (era, day_of_era) = (days / 146_097, days % 146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// One graphics card and whether it is powered, read before anything that
/// could wake it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuCard {
    pub card: String,
    pub vendor: Option<String>,
    pub driver: Option<String>,
    pub runtime_status: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuProcessMemory {
    pub pid: u32,
    pub source: String,
    pub bytes: u64,
}
/// GPU memory is reported apart from PSS. Without a per-process value
/// `gpu_bytes` is absent and `status` says "not measured".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuRecord {
    pub cards: Vec<GpuCard>,
    pub processes: Vec<GpuProcessMemory>,
    pub gpu_bytes: Option<u64>,
    pub status: String,
    pub notes: Vec<String>,
}
/// One DRM client of a file descriptor, from its `fdinfo` text.
#[derive(Debug, Clone, PartialEq)]
pub struct DrmClient {
    pub driver: String,
    pub device: String,
    pub client_id: String,
    /// The sum of the `drm-total-*` keys, or of the older `drm-memory-*`
    /// ones; absent when the driver reports neither.
    pub bytes: Option<u64>,
}
pub fn parse_drm_fdinfo(text: &str) -> Option<DrmClient> {
    let mut fields = HashMap::new();
    let (mut total, mut legacy): (Option<u64>, Option<u64>) = (None, None);
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let sum = if key.starts_with("drm-total-") {
            &mut total
        } else if key.starts_with("drm-memory-") {
            &mut legacy
        } else {
            fields.insert(key, value);
            continue;
        };
        let mut parts = value.split_whitespace();
        let count: u64 = parts.next()?.parse().ok()?;
        let bytes = match parts.next() {
            None => Some(count),
            Some("KiB") => count.checked_mul(1 << 10),
            Some("MiB") => count.checked_mul(1 << 20),
            Some("GiB") => count.checked_mul(1 << 30),
            Some(_) => None,
        }?;
        *sum = Some(sum.unwrap_or(0).checked_add(bytes)?);
    }
    Some(DrmClient {
        driver: (*fields.get("drm-driver")?).to_owned(),
        device: fields.get("drm-pdev").copied().unwrap_or("").to_owned(),
        client_id: (*fields.get("drm-client-id")?).to_owned(),
        bytes: total.or(legacy),
    })
}
/// Lines of `nvidia-smi --query-compute-apps=pid,used_memory
/// --format=csv,noheader,nounits`: the memory is in MiB; `[N/A]` is missing.
pub fn parse_nvidia_apps(text: &str) -> Vec<(u32, Option<u64>)> {
    text.lines()
        .filter_map(|line| {
            let (pid, memory) = line.split_once(',')?;
            Some((
                pid.trim().parse().ok()?,
                memory
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .and_then(|mib| mib.checked_mul(1 << 20)),
            ))
        })
        .collect()
}
/// The GPU memory of `roots` and their descendants, and the power state of
/// every card. A suspended NVIDIA card is not queried: the query would wake it.
pub fn gpu(roots: &[u32]) -> io::Result<GpuRecord> {
    let mut cards = Vec::new();
    for entry in std::fs::read_dir("/sys/class/drm")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix("card")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let device = entry.path().join("device");
        let read = |file: &str| {
            std::fs::read_to_string(device.join(file))
                .ok()
                .map(|s| s.trim().to_owned())
        };
        cards.push(GpuCard {
            card: name,
            vendor: read("vendor"),
            driver: std::fs::read_link(device.join("driver"))
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
            runtime_status: read("power/runtime_status"),
        });
    }
    cards.sort_by(|a, b| a.card.cmp(&b.card));

    let pids = descendants(roots)?;
    let mut processes = Vec::new();
    let mut notes = Vec::new();
    let mut seen = HashSet::new();
    let mut clients = 0;
    for &pid in &pids {
        let Ok(directory) = std::fs::read_dir(format!("/proc/{pid}/fdinfo")) else {
            notes.push(format!("fdinfo of {pid} is not readable"));
            continue;
        };
        for entry in directory.flatten() {
            let Some(client) = std::fs::read_to_string(entry.path())
                .ok()
                .as_deref()
                .and_then(parse_drm_fdinfo)
            else {
                continue;
            };
            // Duplicated descriptors show the same client again.
            if !seen.insert((
                client.driver.clone(),
                client.device.clone(),
                client.client_id.clone(),
            )) {
                continue;
            }
            clients += 1;
            let source = format!(
                "fdinfo {} {} client {}",
                client.driver, client.device, client.client_id
            );
            match client.bytes {
                Some(bytes) => processes.push(GpuProcessMemory { pid, source, bytes }),
                None => notes.push(format!("{source} of {pid} reports no memory keys")),
            }
        }
    }
    if clients == 0 {
        notes.push("the processes hold no DRM client with fdinfo".into());
    }
    for card in cards
        .iter()
        .filter(|c| c.vendor.as_deref() == Some("0x10de"))
    {
        if card.runtime_status.as_deref() != Some("active") {
            notes.push(format!(
                "{} (NVIDIA) is {}: nvidia-smi not run, it would wake the card",
                card.card,
                card.runtime_status
                    .as_deref()
                    .unwrap_or("in an unknown state")
            ));
            continue;
        }
        let output = std::process::Command::new("nvidia-smi")
            .args([
                "--query-compute-apps=pid,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let apps = parse_nvidia_apps(&String::from_utf8_lossy(&output.stdout));
                let mut listed = false;
                for (pid, bytes) in apps.into_iter().filter(|(pid, _)| pids.contains(pid)) {
                    listed = true;
                    match bytes {
                        Some(bytes) => processes.push(GpuProcessMemory {
                            pid,
                            source: "nvidia-smi compute apps".into(),
                            bytes,
                        }),
                        None => notes.push(format!("nvidia-smi gives no memory value for {pid}")),
                    }
                }
                if !listed {
                    notes.push("nvidia-smi compute apps do not list the processes".into());
                }
            }
            Ok(output) => notes.push(format!("nvidia-smi failed: {}", output.status)),
            Err(e) => notes.push(format!("nvidia-smi could not run: {e}")),
        }
        break;
    }
    let gpu_bytes = (!processes.is_empty())
        .then(|| {
            processes
                .iter()
                .try_fold(0u64, |sum, p| sum.checked_add(p.bytes))
        })
        .flatten();
    Ok(GpuRecord {
        cards,
        processes,
        gpu_bytes,
        status: if gpu_bytes.is_some() {
            "measured"
        } else {
            "not measured"
        }
        .into(),
        notes,
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

#[cfg(test)]
mod product_tests {
    use super::*;
    use serde_json::{Value, json};

    const PID: u64 = 4242;

    fn mapping() -> ClockMapping {
        serde_json::from_value(json!({"portal_backend":"gnome","backend_version":"50","evidence_path":"proof","evidence_sha256":"a".repeat(64),"verified":true,"portal_clock_id":1,"presentation_clock_id":1,"portal_to_presentation_offset_ns":0,"uncertainty_ns":1_000_000,"boot_id":"boot","session_type":"wayland","shell_version":"50","portal_version":"1","portal_timestamp_bits":32})).unwrap()
    }
    fn row(origin: Origin, event: &str, id: Option<&str>, details: Value) -> Evidence {
        Evidence::new(
            Backend::Gtk4,
            Mode::Resident,
            origin,
            id.map(str::to_owned),
            event,
            details,
        )
    }
    /// The records of press `count` as the product writes them: received 5 ms
    /// after the portal timestamp, on screen `shown_ms` after it.
    fn press(origin: Origin, count: u64, cold: bool, shown_ms: u64) -> Vec<Evidence> {
        let portal_ms = 1000 * count;
        let id = format!("{PID}-{count}");
        vec![
            row(
                origin,
                "invocation",
                Some(&id),
                json!({"cold":cold,"portal_timestamp_ms":portal_ms,"receipt_monotonic_ns":portal_ms * 1_000_000 + 5_000_000,"activation_token_present":true}),
            ),
            row(
                origin,
                "frontend-event",
                Some(&id),
                json!({"event":"activated","id":id,"token_consumed":true}),
            ),
            row(
                origin,
                "presentation",
                Some(&id),
                json!({"event":"presented","id":id,"time_ns":(portal_ms + shown_ms) * 1_000_000,"clock_id":1,"usable":true,"limitation":null}),
            ),
        ]
    }
    fn memory(origin: Origin, phase: &str, pss: u64, cpu: f64) -> Evidence {
        row(
            origin,
            "memory",
            None,
            json!({"phase":phase,"sample":{"pss_bytes":pss,"rss_bytes":120_000_000,"cpu_percent_one_core":cpu,"idle_seconds":60,"inventory_complete":true,"processes":[{"pid":PID}]}}),
        )
    }
    /// A whole session as the runbook drives it: 298 ms to every frame, which
    /// is 300 ms with the certificate's uncertainty and the quantization.
    fn session(origin: Origin) -> Vec<Evidence> {
        let mut rows = vec![row(
            origin,
            "broker-started",
            None,
            json!({"pid":PID,"boot_id":"boot","application":"addict"}),
        )];
        rows.extend(press(origin, 1, true, 900));
        rows.push(memory(origin, "before-invocation", 40_000_000, 0.0));
        for count in 2..=31 {
            rows.extend(press(origin, count, false, 298));
        }
        rows.push(memory(origin, "after-close", MEMORY_LIMIT, 0.0));
        rows.push(row(
            origin,
            "gpu",
            None,
            json!({"cards":[],"processes":[],"gpu_bytes":null,"status":"not measured","notes":[]}),
        ));
        rows
    }
    fn product(rows: &[Evidence], origin: Origin) -> ProductSummary {
        summarize_product(rows, Some(&mapping()))
            .into_iter()
            .find(|s| s.origin == origin)
            .unwrap()
    }
    fn sample_mut<'a>(rows: &'a mut [Evidence], phase: &str) -> &'a mut Value {
        &mut rows
            .iter_mut()
            .find(|r| r.event == "memory" && r.details["phase"] == phase)
            .unwrap()
            .details["sample"]
    }

    #[test]
    fn all_within_limits_passes_for_both_applications() {
        let mut rows = session(Origin::Foliate);
        rows.extend(session(Origin::AndroidStudio));
        let all = summarize_product(&rows, Some(&mapping()));
        assert_eq!(
            all.iter().map(|s| s.origin).collect::<Vec<_>>(),
            PRODUCT_ORIGINS
        );
        for s in all {
            assert_eq!(s.acceptance, "pass", "{s:?}");
            assert_eq!((s.warm_attempts, s.valid_latency_samples), (30, 30));
            assert_eq!(
                (s.p50_ms, s.p95_ms, s.max_ms),
                (Some(300.0), Some(300.0), Some(300.0))
            );
            assert_eq!(
                (s.pss_min_bytes, s.pss_max_bytes),
                (Some(40_000_000), Some(MEMORY_LIMIT))
            );
            assert_eq!(s.idle_cpu_percent_max, Some(0.0));
            assert_eq!(s.gpu.unwrap()["status"], "not measured");
            assert_eq!(s.not_measured.len(), 2);
        }
    }

    #[test]
    fn one_application_does_not_speak_for_the_other() {
        let rows = session(Origin::Foliate);
        assert_eq!(product(&rows, Origin::Foliate).acceptance, "pass");
        let other = product(&rows, Origin::AndroidStudio);
        assert_eq!(other.acceptance, "incomplete");
        assert_eq!((other.warm_attempts, other.pss_max_bytes), (0, None));
    }

    #[test]
    fn a_session_that_is_not_the_product_is_not_summarised() {
        let mut rows = session(Origin::Foliate);
        rows[0].details = json!({"pid":PID,"boot_id":"boot"});
        let s = product(&rows, Origin::Foliate);
        assert_eq!((s.warm_attempts, s.acceptance.as_str()), (0, "incomplete"));
        // Records with no session start at all are of no known process.
        let s = product(&session(Origin::Foliate)[1..], Origin::Foliate);
        assert_eq!((s.warm_attempts, s.acceptance.as_str()), (0, "incomplete"));
        // A prototype session after the product's does not supersede it.
        let mut both = session(Origin::Foliate);
        both.extend(rows);
        assert_eq!(product(&both, Origin::Foliate).acceptance, "pass");
    }

    #[test]
    fn a_latency_over_the_limit_fails_also_beyond_the_thirty() {
        let mut rows = session(Origin::Foliate);
        rows.extend(press(Origin::Foliate, 20, false, 299));
        // The duplicate id makes that press unusable, not a failure.
        assert_ne!(product(&rows, Origin::Foliate).latency_status, "fail");
        let mut rows = session(Origin::Foliate);
        rows.extend(press(Origin::Foliate, 32, false, 299));
        let s = product(&rows, Origin::Foliate);
        assert_eq!((s.extra_attempts, s.max_extra_ms), (1, Some(301.0)));
        assert_eq!(
            (s.latency_status.as_str(), s.acceptance.as_str()),
            ("fail", "fail")
        );
    }

    #[test]
    fn pss_over_the_limit_fails() {
        let mut rows = session(Origin::Foliate);
        sample_mut(&mut rows, "before-invocation")["pss_bytes"] = json!(MEMORY_LIMIT + 1);
        let s = product(&rows, Origin::Foliate);
        assert_eq!(
            (s.memory_status.as_str(), s.acceptance.as_str()),
            ("fail", "fail")
        );
        assert_eq!(s.pss_max_bytes, Some(MEMORY_LIMIT + 1));
    }

    #[test]
    fn any_idle_cpu_fails() {
        let mut rows = session(Origin::Foliate);
        sample_mut(&mut rows, "after-close")["cpu_percent_one_core"] = json!(1.0);
        let s = product(&rows, Origin::Foliate);
        assert_eq!(s.idle_cpu_status, "fail");
        assert_eq!(
            (s.memory_status.as_str(), s.acceptance.as_str()),
            ("pass", "fail")
        );
        assert_eq!(s.idle_cpu_percent_max, Some(1.0));
    }

    #[test]
    fn fewer_than_thirty_latencies_is_incomplete() {
        let mut rows = session(Origin::Foliate);
        let last = rows
            .iter()
            .rposition(|r| r.event == "presentation")
            .unwrap();
        rows[last].details["time_ns"] = Value::Null;
        let s = product(&rows, Origin::Foliate);
        assert_eq!((s.warm_attempts, s.valid_latency_samples), (30, 29));
        assert_eq!(s.acceptance, "incomplete");
        // Without a certificate, or with one of another boot, nothing is measured.
        let rows = session(Origin::Foliate);
        let none = &summarize_product(&rows, None)[0];
        assert_eq!(
            (none.valid_latency_samples, none.acceptance.as_str()),
            (0, "incomplete")
        );
        let mut other = mapping();
        other.boot_id = "other".into();
        assert_eq!(
            summarize_product(&rows, Some(&other))[0].valid_latency_samples,
            0
        );
    }

    #[test]
    fn a_missing_or_unusable_sample_is_incomplete() {
        for phase in PRODUCT_PHASES {
            let mut rows = session(Origin::Foliate);
            rows.retain(|r| !(r.event == "memory" && r.details["phase"] == phase));
            let s = product(&rows, Origin::Foliate);
            assert_eq!(s.memory_status, "incomplete", "{phase}");
            assert_eq!(s.idle_cpu_status, "incomplete");
            assert_eq!(s.acceptance, "incomplete");
        }
        let unusable: [fn(&mut Value); 6] = [
            |s| s["idle_seconds"] = json!(59),
            |s| s["inventory_complete"] = json!(false),
            |s| s["processes"] = json!([{"pid":1}]),
            |s| s["pss_bytes"] = Value::Null,
            |s| s["rss_bytes"] = Value::Null,
            |s| s["cpu_percent_one_core"] = Value::Null,
        ];
        for (index, change) in unusable.into_iter().enumerate() {
            let mut rows = session(Origin::Foliate);
            let sample = sample_mut(&mut rows, "after-close");
            // Values over the limits in a sample that does not count are not a failure.
            if index < 3 {
                sample["pss_bytes"] = json!(MEMORY_LIMIT + 1);
                sample["cpu_percent_one_core"] = json!(5.0);
            }
            change(sample);
            let s = product(&rows, Origin::Foliate);
            assert_eq!(s.acceptance, "incomplete", "{index}");
            if index < 3 {
                assert_eq!(
                    (s.pss_max_bytes, s.idle_cpu_percent_max),
                    (Some(40_000_000), Some(0.0))
                );
            }
        }
        // A known violation survives a missing secondary value.
        let mut rows = session(Origin::Foliate);
        let sample = sample_mut(&mut rows, "after-close");
        sample["pss_bytes"] = json!(MEMORY_LIMIT + 1);
        sample["cpu_percent_one_core"] = Value::Null;
        assert_eq!(product(&rows, Origin::Foliate).acceptance, "fail");
    }

    #[test]
    fn a_sample_on_the_wrong_side_of_the_trial_does_not_count() {
        let mut rows = session(Origin::Foliate);
        let after = rows
            .iter()
            .position(|r| r.event == "memory" && r.details["phase"] == "after-close")
            .unwrap();
        let sample = rows.remove(after);
        rows.insert(8, sample);
        assert_eq!(product(&rows, Origin::Foliate).memory_status, "incomplete");
        let mut rows = session(Origin::Foliate);
        let before = rows.remove(4);
        assert_eq!(before.details["phase"], "before-invocation");
        rows.insert(10, before);
        assert_eq!(product(&rows, Origin::Foliate).memory_status, "incomplete");
    }

    #[test]
    fn a_repeated_session_supersedes_the_earlier_one() {
        let mut rows = session(Origin::Foliate);
        sample_mut(&mut rows, "after-close")["pss_bytes"] = json!(MEMORY_LIMIT + 1);
        rows.extend(session(Origin::AndroidStudio));
        assert_eq!(product(&rows, Origin::Foliate).acceptance, "fail");
        rows.extend(session(Origin::Foliate));
        assert_eq!(product(&rows, Origin::Foliate).acceptance, "pass");
        assert_eq!(product(&rows, Origin::AndroidStudio).acceptance, "pass");
    }

    #[test]
    fn the_matrix_summary_has_no_product_fields() {
        let rows = session(Origin::Foliate);
        let text = serde_json::to_string(&summarize(&rows, Some(&mapping()))).unwrap();
        for field in [
            "pss_min_bytes",
            "idle_cpu",
            "\"gpu\"",
            "not_measured",
            "android-studio",
        ] {
            assert!(!text.contains(field), "{field}");
        }
    }

    fn host() -> CalibrationHost {
        CalibrationHost {
            boot_id: "boot".into(),
            session_type: "wayland".into(),
            backend_version: "50.0-1.fc44".into(),
            shell_version: "GNOME Shell 50.5".into(),
            portal_version: "1.22.1-1.fc44".into(),
            environment: "{}".into(),
            monotonic_ns: 100_000_000_000,
            boottime_ns: 100_000_000_100,
            realtime_ns: 1_791_676_800_000_000_000,
            evidence_path: "/evidence.txt".into(),
        }
    }
    fn presses(count: u64) -> Vec<Evidence> {
        let mut rows = vec![row(
            Origin::Local,
            "broker-started",
            None,
            json!({"pid":PID,"boot_id":"boot"}),
        )];
        for n in 1..=count {
            rows.extend(press(Origin::Local, n, n == 1, 100));
        }
        rows
    }

    #[test]
    fn ten_presses_of_this_boot_give_a_verified_certificate() {
        use sha2::{Digest, Sha256};
        let calibration = calibrate(&presses(10), &host()).unwrap();
        let m = &calibration.mapping;
        assert!(m.verified);
        assert_eq!((m.uncertainty_ns, m.portal_timestamp_bits), (1_000_000, 32));
        assert_eq!((m.portal_clock_id, m.presentation_clock_id), (1, 1));
        assert_eq!(m.portal_to_presentation_offset_ns, 0);
        assert_eq!(
            (
                m.boot_id.as_str(),
                m.backend_version.as_str(),
                m.shell_version.as_str(),
                m.portal_version.as_str()
            ),
            ("boot", "50.0-1.fc44", "GNOME Shell 50.5", "1.22.1-1.fc44")
        );
        assert_eq!(m.evidence_path, "/evidence.txt");
        assert_eq!(
            m.evidence_sha256,
            format!("{:x}", Sha256::digest(calibration.evidence.as_bytes()))
        );
        let text = &calibration.evidence;
        assert!(text.contains("collected: 2026-10-11 UTC, addict pid 4242"));
        assert!(text.contains("origin local"));
        assert!(text.contains("4242-3 3000 3005000000 5.000"));
        assert!(text.contains("samples=10 min=5.000 ms max=5.000 ms"));
        assert!(text.contains("cannot tell CLOCK_MONOTONIC from CLOCK_BOOTTIME"));
        // The certificate measures the product's own records.
        assert_eq!(
            m.latency(3000, 3_100_000_000, 1, 3_005_000_000),
            Ok(102_000_000)
        );
        let mut suspended = host();
        suspended.boottime_ns += 3_600_000_000_000;
        let text = calibrate(&presses(10), &suspended).unwrap().evidence;
        assert!(text.contains("BOOTTIME-MONOTONIC=3600.000 s"));
        assert!(text.contains("would be 3600 s away from receipt"));
    }

    #[test]
    fn a_calibration_outside_the_rule_is_refused() {
        let refused = |rows: &[Evidence], host: &CalibrationHost, reason: &str| {
            let error = calibrate(rows, host).unwrap_err();
            assert!(error.contains(reason), "{error}");
        };
        refused(&presses(9), &host(), "9 presses recorded");
        refused(&[], &host(), "no session start");
        let mut other = host();
        other.boot_id = "other".into();
        refused(&presses(10), &other, "this boot is other");
        let mut x11 = host();
        x11.session_type = "x11".into();
        refused(&presses(10), &x11, "not wayland");
        // Only the latest session counts.
        let mut rows = presses(10);
        rows.extend(presses(2));
        refused(&rows, &host(), "2 presses recorded");

        let invocation = |rows: &mut [Evidence], n: usize| {
            rows.iter_mut()
                .filter(|r| r.event == "invocation")
                .nth(n)
                .unwrap()
                .details
                .clone()
        };
        let set = |rows: &mut [Evidence], n: usize, key: &str, value: Value| {
            rows.iter_mut()
                .filter(|r| r.event == "invocation")
                .nth(n)
                .unwrap()
                .details[key] = value;
        };
        let mut rows = presses(10);
        let portal = invocation(&mut rows, 4)["portal_timestamp_ms"]
            .as_u64()
            .unwrap();
        for (receipt_ns, reason) in [
            (portal * 1_000_000, "is 0.000000 ms after"),
            (portal * 1_000_000 - 1, "is -0.000001 ms after"),
            (portal * 1_000_000 + 10_000_001, "is 10.000001 ms after"),
        ] {
            set(&mut rows, 4, "receipt_monotonic_ns", json!(receipt_ns));
            refused(&rows, &host(), reason);
        }
        set(
            &mut rows,
            4,
            "receipt_monotonic_ns",
            json!(portal * 1_000_000 + 10_000_000),
        );
        calibrate(&rows, &host()).unwrap();
        set(&mut rows, 4, "portal_timestamp_ms", Value::Null);
        refused(&rows, &host(), "no portal timestamp");

        let frame = |rows: &mut [Evidence], clock: Value| {
            rows.iter_mut()
                .filter(|r| r.event == "presentation")
                .nth(6)
                .unwrap()
                .details["clock_id"] = clock;
        };
        let mut rows = presses(10);
        frame(&mut rows, json!(7));
        refused(&rows, &host(), "on clock 7");
        frame(&mut rows, Value::Null);
        refused(&rows, &host(), "on clock null");
        let mut rows = presses(10);
        let last = rows
            .iter()
            .rposition(|r| r.event == "presentation")
            .unwrap();
        rows.remove(last);
        refused(&rows, &host(), "has no frame");
    }

    #[test]
    fn a_wrapped_32_bit_timestamp_calibrates() {
        let mut rows = presses(10);
        let mut late = host();
        late.monotonic_ns = ((1u64 << 32) + 60_000) * 1_000_000;
        for (n, row) in rows
            .iter_mut()
            .filter(|r| r.event == "invocation")
            .enumerate()
        {
            let ms = (1u64 << 32) + 1000 * n as u64;
            row.details["portal_timestamp_ms"] = json!(ms & u64::from(u32::MAX));
            row.details["receipt_monotonic_ns"] = json!(ms * 1_000_000 + 3_000_000);
        }
        let text = calibrate(&rows, &late).unwrap().evidence;
        assert!(text.contains("min=3.000 ms max=3.000 ms"));
    }

    #[test]
    fn dates_follow_the_calendar() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_791_195_380), "2026-10-05");
        assert_eq!(utc_date(1_798_761_599), "2026-12-31");
    }

    #[test]
    fn drm_fdinfo_sums_the_totals_of_one_client() {
        let text = "pos:\t0\ndrm-driver:\ti915\ndrm-client-id:\t47\ndrm-pdev:\t0000:00:02.0\n\
                    drm-total-system0:\t256880 KiB\ndrm-shared-system0:\t20 MiB\n\
                    drm-resident-system0:\t256880 KiB\ndrm-total-stolen-system0:\t0\n\
                    drm-engine-render:\t9453775862 ns\n";
        assert_eq!(
            parse_drm_fdinfo(text),
            Some(DrmClient {
                driver: "i915".into(),
                device: "0000:00:02.0".into(),
                client_id: "47".into(),
                bytes: Some(256_880 * 1024),
            })
        );
        let legacy = "drm-driver:\tamdgpu\ndrm-client-id:\t3\ndrm-memory-vram:\t2 MiB\ndrm-memory-gtt:\t512 KiB\n";
        assert_eq!(parse_drm_fdinfo(legacy).unwrap().bytes, Some(2_621_440));
        // A client with no memory keys has no value, not zero.
        let bare = "drm-driver:\tnvidia-drm\ndrm-client-id:\t9\n";
        assert_eq!(parse_drm_fdinfo(bare).unwrap().bytes, None);
        assert_eq!(parse_drm_fdinfo("pos:\t0\nflags:\t02\n"), None);
        assert_eq!(
            parse_drm_fdinfo("drm-driver:\tx\ndrm-client-id:\t1\ndrm-total-a:\t1 furlong\n"),
            None
        );
    }

    #[test]
    fn nvidia_apps_are_mebibytes_and_a_missing_value_stays_missing() {
        assert_eq!(
            parse_nvidia_apps("1234, 56\n77, [N/A]\n\nheader\n"),
            [(1234, Some(56 << 20)), (77, None)]
        );
    }
}
