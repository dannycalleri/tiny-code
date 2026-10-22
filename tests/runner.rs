use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use turbo_harness::event::{Event, parse_line};
use turbo_harness::run::{Run, RunStatus};
use turbo_harness::runner::{RunMsg, claude_command, start};

const TIMEOUT: Duration = Duration::from_secs(5);

fn fake_claude(fixture: &str) -> Command {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut cmd = Command::new("sh");
    cmd.arg(dir.join("fake_claude.sh"))
        .env("FAKE_FIXTURE", dir.join("fixtures").join(fixture));
    cmd
}

fn collect(rx: &Receiver<RunMsg>) -> (Vec<Event>, Option<i32>, Vec<String>) {
    let mut events = Vec::new();
    loop {
        match rx.recv_timeout(TIMEOUT).expect("the run did not end") {
            RunMsg::Event(e) => events.push(e),
            RunMsg::Exited { code, stderr } => return (events, code, stderr),
        }
    }
}

fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("turbo-harness-{}-{name}", std::process::id()))
}

#[test]
fn first_event_arrives_while_the_process_still_runs() {
    let gate = temp_path("gate");
    let _ = std::fs::remove_file(&gate);
    let mut cmd = fake_claude("hello.jsonl");
    cmd.env("FAKE_GATE", &gate);
    let rx = start(cmd, || {}).unwrap().events;

    let RunMsg::Event(first) = rx
        .recv_timeout(TIMEOUT)
        .expect("no event before the gate opened")
    else {
        panic!("the run ended before the gate opened");
    };

    std::fs::write(&gate, "").unwrap();
    let (rest, code, _) = collect(&rx);
    let _ = std::fs::remove_file(&gate);

    let expected: Vec<Event> = include_str!("fixtures/hello.jsonl")
        .lines()
        .map(parse_line)
        .collect();
    let received: Vec<Event> = std::iter::once(first).chain(rest).collect();
    assert_eq!(received, expected);
    assert_eq!(code, Some(0));
}

#[test]
fn non_zero_exit_reports_the_code_and_stderr() {
    let mut cmd = fake_claude("hello.jsonl");
    cmd.env("FAKE_EXIT", "3").env("FAKE_STDERR", "API error");
    let rx = start(cmd, || {}).unwrap().events;

    let (_, code, stderr) = collect(&rx);
    assert_eq!(code, Some(3));
    assert_eq!(stderr, ["API error"]);
}

#[test]
fn missing_binary_is_reported_as_not_on_path() {
    let err = start(Command::new("claude-does-not-exist"), || {}).unwrap_err();
    assert_eq!(err, "claude-does-not-exist was not found on PATH");
}

#[test]
fn missing_working_directory_is_not_reported_as_missing_binary() {
    let cmd = claude_command("sh", "hi", None, Path::new("/no/such/turbo-harness/dir"));
    let err = start(cmd, || {}).unwrap_err();
    assert_eq!(
        err,
        "working directory /no/such/turbo-harness/dir does not exist"
    );
}

fn is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn stop_a_hanging_run(ignore_term: Option<&str>) -> Duration {
    let pid_file = temp_path(&format!("pid-{}", ignore_term.unwrap_or("none")));
    let _ = std::fs::remove_file(&pid_file);
    let mut cmd = fake_claude("hello.jsonl");
    cmd.env("FAKE_HANG", &pid_file);
    if let Some(who) = ignore_term {
        cmd.env("FAKE_IGNORE_TERM", who);
    }
    let handle = start(cmd, || {}).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let grandchild = loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the fake never hung");
        std::thread::sleep(Duration::from_millis(10));
    };

    let mut run = Run::new("hang");
    let stopped_at = Instant::now();
    run.request_stop();
    handle.stopper.stop();
    loop {
        let msg = handle
            .events
            .recv_timeout(TIMEOUT)
            .expect("the run did not end");
        let exited = matches!(msg, RunMsg::Exited { .. });
        run.apply(msg);
        if exited {
            break;
        }
    }
    let took = stopped_at.elapsed();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(run.error, None);
    let gone_by = stopped_at + Duration::from_secs(3);
    while is_alive(grandchild) {
        assert!(Instant::now() < gone_by, "the grandchild survived");
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(&pid_file);
    took
}

#[test]
fn stop_ends_the_process_and_its_children() {
    assert!(stop_a_hanging_run(None) < Duration::from_secs(1));
}

#[test]
fn stop_kills_a_process_that_ignores_sigterm() {
    let took = stop_a_hanging_run(Some("leader"));
    assert!(took >= Duration::from_secs(2), "{took:?}");
}

#[test]
fn stop_kills_a_child_that_ignores_sigterm_after_claude_exits() {
    assert!(stop_a_hanging_run(Some("grandchild")) < Duration::from_secs(1));
}
