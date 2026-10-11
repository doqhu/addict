use addict_exp4::{
    Evidence,
    metrics::{self, ClockMapping, MEMORY_LIMIT},
    protocol::{self, Backend, Command, Invocation, Message, Mode, Origin},
    work,
};
use serde_json::json;
use std::{
    io::Cursor,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
fn temp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "exp4-test-{name}-{}-{}",
        std::process::id(),
        metrics::monotonic_ns()
    ))
}
fn row(event: &str, id: Option<&str>, details: serde_json::Value) -> Evidence {
    Evidence::new(
        Backend::Gtk4,
        Mode::Resident,
        Origin::Foliate,
        id.map(str::to_owned),
        event,
        details,
    )
}
fn mapping() -> ClockMapping {
    ClockMapping {
        portal_backend: "gnome".into(),
        backend_version: "50.0-1.fc44".into(),
        evidence_path: "evidence.txt".into(),
        evidence_sha256: "a".repeat(64),
        verified: true,
        portal_clock_id: 1,
        presentation_clock_id: 1,
        portal_to_presentation_offset_ns: 0,
        uncertainty_ns: 0,
        boot_id: "test-boot".into(),
        session_type: "wayland".into(),
        shell_version: "GNOME Shell 50.5".into(),
        portal_version: "1.22.1-1.fc44".into(),
        portal_timestamp_bits: 64,
    }
}
fn complete() -> Vec<Evidence> {
    let mut rows = Vec::new();
    for n in 0..30 {
        let id = n.to_string();
        rows.push(row("invocation", Some(&id), json!({"cold":false})));
        rows.push(row(
            "presentation",
            Some(&id),
            json!({"usable":true,"acceptance_latency_ns":100_000_000}),
        ));
    }
    for phase in ["before-invocation", "after-close"] {
        rows.push(row("memory",None,json!({"phase":phase,"sample":{"pss_bytes":MEMORY_LIMIT,"rss_bytes":120_000_000,"cpu_percent_one_core":0.0,"idle_seconds":60,"inventory_complete":true,"processes":[{},{}]}})));
    }
    rows.push(row(
        "frontend-event",
        Some("29"),
        json!({"event":"work","outcome":"ocr-started","child_pid":123}),
    ));
    rows.push(row(
        "frontend-event",
        Some("29"),
        json!({"event":"work","outcome":"ocr-completed: fixture","child_pid":null}),
    ));
    rows.push(row("memory",None,json!({"phase":"after-ocr","sample":{"pss_bytes":MEMORY_LIMIT,"rss_bytes":120_000_000,"cpu_percent_one_core":0.0,"idle_seconds":60,"inventory_complete":true,"processes":[{},{}]}})));
    rows.push(row("human-scenarios", None, json!({"all_passed":true})));
    rows
}
fn summary(rows: &[Evidence]) -> metrics::Summary {
    metrics::summarize(rows, None)
        .into_iter()
        .find(|s| {
            s.backend == Backend::Gtk4 && s.mode == Mode::Resident && s.origin == Origin::Foliate
        })
        .unwrap()
}
#[test]
fn malformed_frames_and_unknown_fields_are_rejected() {
    for bytes in [
        vec![0, 0, 0, 0],
        u32::MAX.to_be_bytes().to_vec(),
        vec![0, 0, 0, 3, b'{', b'x', b'}'],
        vec![0, 0, 0, 4, b'{'],
    ] {
        assert!(protocol::read(&mut Cursor::new(bytes)).is_err());
    }
    let body = br#"{"type":"control","origin":"local","quit":false,"token":"unexpected"}"#;
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend(body);
    assert!(protocol::read(&mut Cursor::new(bytes)).is_err());
}
#[test]
fn token_and_invocation_routing_round_trip() {
    let invocation = Invocation {
        id: "run-2".into(),
        origin: Origin::IntelliJ,
        receipt_monotonic_ns: 10,
        portal_timestamp_ms: Some(9),
        activation_token: Some("fresh-token-2".into()),
        cold: false,
    };
    invocation.validate().unwrap();
    let mut bytes = Vec::new();
    protocol::write(
        &mut bytes,
        &Message::Command {
            value: Command::Invoke { invocation },
        },
    )
    .unwrap();
    let Message::Command {
        value: Command::Invoke { invocation },
    } = protocol::read(&mut Cursor::new(bytes)).unwrap()
    else {
        panic!("wrong routing")
    };
    assert_eq!(invocation.id, "run-2");
    assert_eq!(
        invocation.activation_token.as_deref(),
        Some("fresh-token-2")
    );
    assert_eq!(invocation.origin, Origin::IntelliJ);
    let mut bad = invocation;
    bad.activation_token = Some(String::new());
    assert!(bad.validate().is_err());
}
#[test]
fn memory_units_are_kib_but_limit_is_decimal() {
    assert_eq!(MEMORY_LIMIT, 100_000_000);
    assert_eq!(
        metrics::parse_kib("Pss: 97656 kB\n", "Pss"),
        Some(99_999_744)
    );
    assert_eq!(metrics::parse_kib("Pss: 1 MB", "Pss"), None);
    assert_eq!(metrics::parse_kib("", "Pss"), None);
}
#[test]
fn unverified_missing_or_wrong_clock_never_qualifies() {
    let mut m = mapping();
    assert_eq!(
        m.latency(1_000, 1_100_000_000, 1, 1_010_000_000).unwrap(),
        101_000_000
    );
    m.verified = false;
    assert!(m.latency(1_000, 1_100_000_000, 1, 1_010_000_000).is_err());
    m.verified = true;
    assert!(m.latency(1_000, 1_100_000_000, 0, 1_010_000_000).is_err());
    assert!(m.latency(1_000, 900_000_000, 1, 1_010_000_000).is_err());
    assert!(m.latency(1_000, 1_100_000_000, 1, 99_000_000_000).is_err());
}
#[test]
fn monotonic_u32_rollover_maps_nearest_past_epoch() {
    let mut m = mapping();
    m.portal_timestamp_bits = 32;
    let receipt = ((1u64 << 32) + 50) * 1_000_000;
    assert_eq!(
        m.latency(u32::MAX as u64 - 9, receipt + 1_000_000, 1, receipt)
            .unwrap(),
        62_000_000
    );
    assert!(m.latency(u32::MAX as u64 + 1, receipt, 1, receipt).is_err());
}
#[test]
fn certificate_rejects_stale_environment_and_changed_hash() {
    use sha2::{Digest, Sha256};
    let mut m = mapping();
    let path = temp("certificate");
    std::fs::write(&path, "validated runtime/source trace").unwrap();
    m.evidence_path = path.to_string_lossy().into();
    m.evidence_sha256 = format!("{:x}", Sha256::digest(b"validated runtime/source trace"));
    m.boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .unwrap()
        .trim()
        .into();
    let environment = addict_probe::record::Environment {
        xdg_desktop_portal_gnome: Some(m.backend_version.clone()),
        gnome_shell: Some(m.shell_version.clone()),
        xdg_desktop_portal: Some(m.portal_version.clone()),
        session_type: Some("wayland".into()),
        ..Default::default()
    };
    m.validate_environment(&environment).unwrap();
    std::fs::write(&path, "changed").unwrap();
    assert!(m.validate_environment(&environment).is_err());
    std::fs::remove_file(path).unwrap();
}
#[test]
fn missing_samples_and_feedback_do_not_pass() {
    let rows = complete();
    assert_eq!(summary(&rows).acceptance, "pass");
    let mut rows = rows.clone();
    rows.retain(|r| r.invocation_id.as_deref() != Some("29"));
    assert_eq!(summary(&rows).acceptance, "incomplete");
    let mut rows = complete();
    rows[1].details["acceptance_latency_ns"] = serde_json::Value::Null;
    assert_eq!(summary(&rows).latency_status, "incomplete");
    let mut rows = complete();
    rows.push(rows[1].clone());
    assert_eq!(summary(&rows).acceptance, "incomplete");
}
#[test]
fn failures_are_sticky_even_with_missing_later_memory_phase() {
    let mut rows = complete();
    for row in &mut rows {
        if row.event == "memory" && row.details["phase"] == "before-invocation" {
            row.details["sample"]["pss_bytes"] = json!(MEMORY_LIMIT + 1);
        }
    }
    rows.retain(|r| r.details["phase"] != "after-ocr");
    assert_eq!(summary(&rows).memory_status, "fail");
    assert_eq!(summary(&rows).acceptance, "fail");
}
#[test]
fn every_sample_must_meet_latency_threshold() {
    let mut rows = complete();
    rows[1].details["acceptance_latency_ns"] = json!(300_000_001);
    let s = summary(&rows);
    assert_eq!(s.latency_status, "fail");
    assert_eq!(s.max_ms, Some(300.000001));
    assert_eq!(s.p95_ms, Some(100.0));
}
#[test]
fn absent_process_metric_is_missing_not_zero() {
    let metric = metrics::process(u32::MAX);
    assert_eq!(metric.pss_bytes, None);
    assert_eq!(metric.rss_bytes, None);
    assert!(metric.error.is_some());
}
#[test]
fn append_only_records_preserve_previous_bytes() {
    let path = temp("append");
    let first = row("first", None, json!({"activation_token_present":true}));
    addict_exp4::append(&path, &first).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    addict_exp4::append(&path, &row("second", None, json!({}))).unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(&bytes));
    assert_eq!(metrics::read_evidence(&path).unwrap().len(), 2);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn desktop_entries_refuse_unrelated_files_and_symlinks() {
    use addict_exp4::desktop;
    let dir = temp("desktop");
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join(format!("{}.desktop", Backend::Gtk4.app_id()));
    std::fs::write(&path, "unrelated").unwrap();
    assert!(desktop::install(&dir, Backend::Gtk4, &std::env::current_exe().unwrap()).is_err());
    assert!(desktop::uninstall(&dir, Backend::Gtk4).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "unrelated");
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(desktop::install(&dir, Backend::Gtk4, &std::env::current_exe().unwrap()).is_err());
    assert!(desktop::uninstall(&dir, Backend::Gtk4).is_err());
    std::fs::remove_file(&path).unwrap();
    desktop::install(&dir, Backend::Gtk4, &std::env::current_exe().unwrap()).unwrap();
    desktop::uninstall(&dir, Backend::Gtk4).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
#[test]
fn real_tesseract_fixture_completes_and_releases_child() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../experiments/exp4/ocr-fixture.png");
    let mut pid = 0;
    let text = work::run_tesseract(&fixture, Duration::from_secs(15), |child| pid = child).unwrap();
    assert!(text.contains("The bank is beside the river."));
    assert!(pid > 0);
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}
#[test]
fn quit_during_ocr_kills_and_reaps_child() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../experiments/exp4/ocr-fixture.png");
    let cancel = AtomicBool::new(false);
    let mut pid = 0;
    let error = work::run_child(&fixture, Duration::from_secs(15), &cancel, |child| {
        pid = child;
        cancel.store(true, Ordering::Relaxed);
    })
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}
#[tokio::test]
async fn partial_async_frame_has_a_deadline() {
    use tokio::io::AsyncWriteExt;
    let (mut writer, mut reader) = tokio::io::duplex(32);
    writer.write_all(&[0]).await.unwrap();
    let error = protocol::read_async(&mut reader).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[tokio::test]
async fn broker_local_mode_correlates_and_reaps_a_real_frontend_process() {
    use addict_exp4::broker;
    use std::os::unix::fs::PermissionsExt;
    let dir = temp("broker");
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let frontend = dir.join("frontend");
    std::fs::write(&frontend,r#"#!/usr/bin/python3
import socket,struct,json,os,sys
s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[sys.argv.index('--socket')+1])
def send(v):
 b=json.dumps(v).encode();s.sendall(struct.pack('!I',len(b))+b)
def readn(n):
 b=b''
 while len(b)<n:
  x=s.recv(n-len(b))
  if not x: raise EOFError()
  b+=x
 return b
def receive():return json.loads(readn(struct.unpack('!I',readn(4))[0]))
send({'type':'hello','version':1,'backend':'gtk4','pid':os.getpid()})
while True:
 c=receive()['value']
 if c['command']=='quit':break
 i=c['invocation']['id']
 send({'type':'event','value':{'event':'presented','id':i,'time_ns':None,'clock_id':None,'usable':True,'diagnostic_draw_ns':0,'limitation':'mock has no display'}})
"#).unwrap();
    std::fs::set_permissions(&frontend, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("broker.sock");
    let results = dir.join("results.jsonl");
    let args = broker::Args {
        backend: Backend::Gtk4,
        mode: Mode::Resident,
        origin: Origin::Local,
        frontend,
        socket: socket.clone(),
        results: results.clone(),
        trigger: None,
        timeout: Duration::from_secs(10),
        binding_timeout: Duration::from_secs(1),
        local_only: true,
        clock_mapping: None,
        fixture: PathBuf::from("unused"),
        vulkan_drivers: None,
    };
    let task = tokio::spawn(broker::run(args));
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    broker::control(&socket, Origin::Local, false)
        .await
        .unwrap();
    for _ in 0..200 {
        if results.exists()
            && metrics::read_evidence(&results)
                .unwrap()
                .iter()
                .any(|r| r.event == "presentation")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    broker::control(&socket, Origin::Local, true).await.unwrap();
    task.await.unwrap().unwrap();
    let rows = metrics::read_evidence(&results).unwrap();
    let present = rows
        .iter()
        .find(|r| r.event == "presentation")
        .expect("correlated presentation");
    assert!(present.invocation_id.is_some());
    assert!(present.details["acceptance_latency_ns"].is_null());
    let pid = rows
        .iter()
        .find(|r| r.event == "frontend-connected")
        .unwrap()
        .details["pid"]
        .as_u64()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert!(!socket.exists());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn observed_latency_failure_survives_incomplete_trials() {
    let mut rows = complete();
    rows[1].details["acceptance_latency_ns"] = json!(300_000_001);
    rows.retain(|r| r.invocation_id.as_deref() != Some("28"));
    assert_eq!(summary(&rows).latency_status, "fail");
    assert_eq!(summary(&rows).acceptance, "fail");
}

fn worker_transport(
    name: &str,
) -> (
    PathBuf,
    protocol::Client,
    protocol::Emitter,
    std::sync::mpsc::Receiver<protocol::FrontendEvent>,
    std::os::unix::net::UnixStream,
) {
    let directory = temp(name);
    std::fs::create_dir(&directory).unwrap();
    let socket = directory.join("worker.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let client = protocol::Client::connect(&socket, Backend::Gtk4).unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    assert!(matches!(
        protocol::read(&mut peer).unwrap(),
        Message::Hello {
            backend: Backend::Gtk4,
            ..
        }
    ));
    let emitter = client.emitter();
    let outcomes = emitter.subscribe();
    (directory, client, emitter, outcomes, peer)
}
fn editable_action(id: &str) -> Message {
    Message::Command {
        value: Command::Invoke {
            invocation: Invocation {
                id: id.into(),
                origin: Origin::Local,
                receipt_monotonic_ns: metrics::monotonic_ns(),
                portal_timestamp_ms: None,
                activation_token: None,
                cold: false,
            },
        },
    }
}
#[test]
fn delayed_worker_delivers_visible_completion_without_blocking_another_action() {
    use protocol::FrontendEvent;
    let (directory, client, emitter, outcomes, mut peer) = worker_transport("delayed-observer");
    let workers = work::Workers::default();
    workers.delayed(
        emitter.clone(),
        "waiting-delay".into(),
        Duration::from_secs(1),
    );
    protocol::write(&mut peer, &editable_action("edit-while-waiting")).unwrap();
    let Command::Invoke { invocation } = client
        .commands
        .recv_timeout(Duration::from_millis(500))
        .unwrap()
    else {
        panic!("editing action lost")
    };
    assert_eq!(invocation.id, "edit-while-waiting");
    emitter.send(FrontendEvent::Focus {
        id: invocation.id,
        focused: true,
    });
    assert!(matches!(
        outcomes.recv_timeout(Duration::from_millis(500)).unwrap(),
        FrontendEvent::Focus { focused: true, .. }
    ));
    let visible = outcomes.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        matches!(&visible,FrontendEvent::Work {id,outcome,child_pid:None} if id=="waiting-delay"&&outcome=="delayed-work-completed")
    );
    // The same completed outcome also arrives on the broker's evidence channel.
    let mut broker_completed = false;
    for _ in 0..2 {
        if let Message::Event {
            value: FrontendEvent::Work { outcome, .. },
        } = protocol::read(&mut peer).unwrap()
        {
            broker_completed |= outcome == "delayed-work-completed";
        }
    }
    assert!(broker_completed);
    workers.shutdown();
    drop(peer);
    drop(client);
    drop(emitter);
    std::fs::remove_dir_all(directory).unwrap();
}
#[test]
fn invalid_ocr_fixture_delivers_visible_error_and_keeps_actions_available() {
    use protocol::FrontendEvent;
    let (directory, client, emitter, outcomes, mut peer) = worker_transport("ocr-error-observer");
    let workers = work::Workers::default();
    workers.ocr(
        &directory.join("missing-fixture.png"),
        Duration::from_secs(3),
        emitter.clone(),
        "waiting-ocr".into(),
    );
    protocol::write(&mut peer, &editable_action("manual-after-ocr-error")).unwrap();
    let Command::Invoke { invocation } = client
        .commands
        .recv_timeout(Duration::from_millis(500))
        .unwrap()
    else {
        panic!("manual action lost")
    };
    assert_eq!(invocation.id, "manual-after-ocr-error");
    let mut child = None;
    let mut visible_error = false;
    for _ in 0..2 {
        match outcomes.recv_timeout(Duration::from_secs(3)).unwrap() {
            FrontendEvent::Work {
                id,
                outcome,
                child_pid,
            } => {
                assert_eq!(id, "waiting-ocr");
                if outcome == "ocr-started" {
                    child = child_pid;
                } else {
                    assert!(outcome.starts_with("ocr-error:"));
                    assert_eq!(child_pid, None);
                    visible_error = true;
                }
            }
            other => panic!("unexpected visible event {other:?}"),
        }
    }
    assert!(visible_error);
    workers.shutdown();
    assert_eq!(
        unsafe { libc::kill(child.expect("real OCR child") as i32, 0) },
        -1
    );
    let mut broker_error = false;
    for _ in 0..2 {
        if let Message::Event {
            value: FrontendEvent::Work { outcome, .. },
        } = protocol::read(&mut peer).unwrap()
        {
            broker_error |= outcome.starts_with("ocr-error:");
        }
    }
    assert!(broker_error);
    drop(peer);
    drop(client);
    drop(emitter);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn after_ocr_requires_same_invocation_start_then_completion_before_sample() {
    let rows = complete();
    let started = rows
        .iter()
        .position(|r| r.details["outcome"] == "ocr-started")
        .unwrap();
    let completed = started + 1;
    let sample = completed + 1;
    let mut reversed = rows.clone();
    reversed.swap(started, completed);
    assert_eq!(summary(&reversed).memory_status, "incomplete");
    let mut unrelated = rows.clone();
    unrelated[completed].invocation_id = Some("other".into());
    assert_eq!(summary(&unrelated).memory_status, "incomplete");
    let mut early = rows;
    let memory = early.remove(sample);
    early.insert(started, memory);
    assert_eq!(summary(&early).memory_status, "incomplete");
}
#[test]
fn known_pss_violation_survives_missing_secondary_metrics() {
    let mut rows = complete();
    let sample = rows.iter_mut().find(|r| r.event == "memory").unwrap();
    sample.details["sample"]["pss_bytes"] = json!(MEMORY_LIMIT + 1);
    sample.details["sample"]["rss_bytes"] = serde_json::Value::Null;
    sample.details["sample"]["cpu_percent_one_core"] = serde_json::Value::Null;
    assert_eq!(summary(&rows).memory_status, "fail");
}

#[test]
fn high_pss_without_complete_idle_inventory_is_incomplete() {
    for field in ["idle_seconds", "inventory_complete"] {
        let mut rows = complete();
        let sample = rows.iter_mut().find(|r| r.event == "memory").unwrap();
        sample.details["sample"]["pss_bytes"] = json!(MEMORY_LIMIT + 1);
        sample.details["sample"][field] = if field == "idle_seconds" {
            json!(59)
        } else {
            json!(false)
        };
        assert_eq!(summary(&rows).memory_status, "incomplete");
    }
}

#[test]
fn presses_after_the_thirty_trials_do_not_dilute_but_violations_stay() {
    let mut rows = complete();
    rows.push(row("invocation", Some("manual"), json!({"cold":false})));
    rows.push(row(
        "presentation",
        Some("manual"),
        json!({"usable":true,"acceptance_latency_ns":200_000_000}),
    ));
    let s = summary(&rows);
    assert_eq!((s.warm_attempts, s.extra_attempts), (30, 1));
    assert_eq!(s.max_ms, Some(100.0));
    assert_eq!(s.max_extra_ms, Some(200.0));
    assert_eq!(s.acceptance, "pass");
    rows.last_mut().unwrap().details["acceptance_latency_ns"] = json!(300_000_001);
    assert_eq!(summary(&rows).latency_status, "fail");
}
#[test]
fn resident_memory_without_the_frontend_process_is_incomplete() {
    let mut rows = complete();
    let sample = rows.iter_mut().find(|r| r.event == "memory").unwrap();
    sample.details["sample"]["processes"] = json!([{}]);
    assert_eq!(summary(&rows).memory_status, "incomplete");
}
#[test]
fn a_repeated_session_supersedes_the_earlier_one() {
    let mut rows = vec![row("broker-started", None, json!({}))];
    rows.extend(complete());
    for row in &mut rows {
        if row.event == "presentation" {
            row.details["acceptance_latency_ns"] = json!(2_000_000_000u64);
        }
    }
    assert_eq!(summary(&rows).acceptance, "fail");
    rows.push(row("broker-started", None, json!({})));
    rows.extend(complete());
    assert_eq!(summary(&rows).acceptance, "pass");
}

#[test]
fn restricted_vulkan_sessions_are_summarized_separately() {
    let mut rows = vec![row("broker-started", None, json!({}))];
    rows.extend(complete());
    rows.push(row(
        "broker-started",
        None,
        json!({"vulkan_drivers":"intel_icd*"}),
    ));
    let summaries: Vec<_> = metrics::summarize(&rows, None)
        .into_iter()
        .filter(|s| s.backend == Backend::Gtk4 && s.mode == Mode::Resident)
        .filter(|s| s.origin == Origin::Foliate)
        .collect();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].acceptance, "pass");
    assert_eq!(summaries[1].vulkan_drivers.as_deref(), Some("intel_icd*"));
    assert_eq!(summaries[1].acceptance, "incomplete");
}
