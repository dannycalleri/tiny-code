use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, ErrorKind, Read};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use crate::event::{Event, parse_line};

const STDERR_LINES: usize = 200;
const KILL_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum RunMsg {
    Event(Event),
    Exited {
        code: Option<i32>,
        stderr: Vec<String>,
    },
}

#[derive(Debug)]
pub struct RunHandle {
    pub events: Receiver<RunMsg>,
    pub stopper: Stopper,
}

#[derive(Debug, Clone, Copy)]
pub struct Stopper {
    process_group: i32,
}

impl Stopper {
    pub fn stop(&self) {
        let group = self.process_group;
        signal_group(group, libc::SIGTERM);
        thread::spawn(move || {
            thread::sleep(KILL_AFTER);
            signal_group(group, libc::SIGKILL);
        });
    }
}

fn signal_group(process_group: i32, signal: libc::c_int) {
    unsafe {
        libc::kill(-process_group, signal);
    }
}

pub fn build_args(prompt: &str, session_id: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        prompt,
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "auto",
    ]
    .map(String::from)
    .to_vec();
    if let Some(id) = session_id {
        args.extend(["--resume".to_string(), id.to_string()]);
    }
    args
}

pub fn claude_command(
    program: &str,
    prompt: &str,
    session_id: Option<&str>,
    cwd: &Path,
) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(build_args(prompt, session_id)).current_dir(cwd);
    cmd
}

pub fn start(mut cmd: Command, notify: impl Fn() + Send + 'static) -> Result<RunHandle, String> {
    let mut child = cmd
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_error(&cmd, e))?;
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let stderr_tail = thread::spawn(move || tail_lines(stderr, STDERR_LINES));
    let stopper = Stopper {
        process_group: child.id() as i32,
    };

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).split(b'\n') {
            let Ok(line) = line else { break };
            let line = String::from_utf8_lossy(&line);
            if line.trim().is_empty() {
                continue;
            }
            if tx.send(RunMsg::Event(parse_line(&line))).is_err() {
                break;
            }
            notify();
        }
        let code = child.wait().ok().and_then(|status| status.code());
        let stderr = stderr_tail.join().unwrap_or_default();
        let _ = tx.send(RunMsg::Exited { code, stderr });
        notify();
    });
    Ok(RunHandle {
        events: rx,
        stopper,
    })
}

fn spawn_error(cmd: &Command, error: io::Error) -> String {
    if let Some(dir) = cmd.get_current_dir()
        && !dir.is_dir()
    {
        return format!("working directory {} does not exist", dir.display());
    }
    match error.kind() {
        ErrorKind::NotFound => format!(
            "{} was not found on PATH",
            cmd.get_program().to_string_lossy()
        ),
        _ => error.to_string(),
    }
}

fn tail_lines(reader: impl Read, max: usize) -> Vec<String> {
    let mut tail = VecDeque::with_capacity(max);
    for line in BufReader::new(reader).split(b'\n') {
        let Ok(line) = line else { break };
        if tail.len() == max {
            tail.pop_front();
        }
        tail.push_back(String::from_utf8_lossy(&line).into_owned());
    }
    tail.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_args_puts_the_prompt_after_p_and_streams_json() {
        assert_eq!(
            build_args("hi there", None),
            [
                "-p",
                "hi there",
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-mode",
                "auto"
            ]
        );
    }

    #[test]
    fn claude_command_runs_in_the_session_directory_with_the_built_args() {
        let cmd = claude_command("claude", "hi", Some("abc"), Path::new("/some/dir"));
        assert_eq!(cmd.get_program(), "claude");
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/some/dir")));
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, build_args("hi", Some("abc")));
    }

    #[test]
    fn build_args_resumes_a_known_session() {
        let args = build_args("next", Some("abc"));
        assert_eq!(args[args.len() - 2..], ["--resume", "abc"]);
    }

    #[test]
    fn tail_lines_keeps_only_the_last_lines() {
        let text = (1..=5).map(|n| format!("line {n}\n")).collect::<String>();
        assert_eq!(tail_lines(text.as_bytes(), 2), ["line 4", "line 5"]);
    }
}
