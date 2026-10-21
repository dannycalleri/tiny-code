use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use turbo_harness::event::{Event, parse_line};
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
    let rx = start(cmd, || {}).unwrap();

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
    let rx = start(cmd, || {}).unwrap();

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
    let cmd = claude_command("sh", "hi", Path::new("/no/such/turbo-harness/dir"));
    let err = start(cmd, || {}).unwrap_err();
    assert_eq!(
        err,
        "working directory /no/such/turbo-harness/dir does not exist"
    );
}
