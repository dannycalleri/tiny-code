use crate::event::Event;
use crate::runner::RunMsg;
use crate::trajectory::Trajectory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Done,
    Failed,
}

#[derive(Debug)]
pub struct Run {
    pub prompt: String,
    pub lines: Vec<String>,
    pub trajectory: Trajectory,
    pub status: RunStatus,
    pub error: Option<String>,
    result_is_error: Option<bool>,
}

impl Run {
    pub fn new(prompt: &str) -> Self {
        Run {
            prompt: prompt.to_string(),
            lines: Vec::new(),
            trajectory: Trajectory::default(),
            status: RunStatus::Running,
            error: None,
            result_is_error: None,
        }
    }

    pub fn failed_to_start(prompt: &str, error: String) -> Self {
        Run {
            status: RunStatus::Failed,
            error: Some(error),
            ..Run::new(prompt)
        }
    }

    pub fn apply(&mut self, msg: RunMsg) {
        match msg {
            RunMsg::Event(event) => {
                if let Event::Result(result) = &event {
                    self.result_is_error = Some(result.is_error);
                }
                self.lines.push(event_line(&event));
                self.trajectory.push(&event);
            }
            RunMsg::Exited { code, stderr } => {
                self.status = final_status(self.result_is_error, code);
                if self.status == RunStatus::Failed && !stderr.is_empty() {
                    self.error = Some(stderr.join("\n"));
                }
            }
        }
    }
}

fn final_status(result_is_error: Option<bool>, exit_code: Option<i32>) -> RunStatus {
    match (result_is_error, exit_code) {
        (Some(false), Some(0)) => RunStatus::Done,
        (None, Some(0)) => RunStatus::Done,
        _ => RunStatus::Failed,
    }
}

fn event_line(event: &Event) -> String {
    const MAX: usize = 200;
    let line = match event {
        Event::System(s) => format!("system/{}", s.subtype),
        Event::Assistant(m) => format!("assistant {:?}", m.message.content),
        Event::User(m) => format!("user {:?}", m.message.content),
        Event::RateLimit => "rate_limit_event".to_string(),
        Event::Result(r) => format!("result/{} {:?}", r.subtype, r.result),
        Event::Unknown(v) => format!("unknown {}", v["type"]),
        Event::Unparsed(line) => format!("unparsed {line}"),
    };
    match line.char_indices().nth(MAX) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse_line;

    fn finished(fixture_lines: &[&str], code: Option<i32>, stderr: &[&str]) -> Run {
        let mut run = Run::new("hi");
        for line in fixture_lines {
            run.apply(RunMsg::Event(parse_line(line)));
        }
        run.apply(RunMsg::Exited {
            code,
            stderr: stderr.iter().map(|s| s.to_string()).collect(),
        });
        run
    }

    const OK_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":false}"#;
    const ERROR_RESULT: &str =
        r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#;

    #[test]
    fn a_new_run_is_running() {
        assert_eq!(Run::new("hi").status, RunStatus::Running);
    }

    #[test]
    fn successful_result_and_exit_zero_is_done() {
        let run = finished(&[OK_RESULT], Some(0), &[]);
        assert_eq!(run.status, RunStatus::Done);
        assert_eq!(run.error, None);
    }

    #[test]
    fn error_result_is_failed_even_with_exit_zero() {
        assert_eq!(
            finished(&[ERROR_RESULT], Some(0), &[]).status,
            RunStatus::Failed
        );
    }

    #[test]
    fn non_zero_exit_is_failed_with_the_stderr_text() {
        let run = finished(&[OK_RESULT], Some(1), &["boom", "trace"]);
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.error.as_deref(), Some("boom\ntrace"));
    }

    #[test]
    fn clean_exit_without_a_result_is_done() {
        assert_eq!(finished(&[], Some(0), &[]).status, RunStatus::Done);
    }

    #[test]
    fn a_signal_is_failed_even_after_a_successful_result() {
        assert_eq!(finished(&[OK_RESULT], None, &[]).status, RunStatus::Failed);
    }

    #[test]
    fn apply_builds_the_trajectory() {
        let run = finished(
            &[r#"{"type":"assistant","message":{"content":"hi"}}"#],
            Some(0),
            &[],
        );
        assert_eq!(
            run.trajectory.nodes,
            [crate::trajectory::Node::Text("hi".into())]
        );
    }

    #[test]
    fn each_event_adds_one_line() {
        let run = finished(
            &[r#"{"type":"system","subtype":"init"}"#, OK_RESULT],
            Some(0),
            &[],
        );
        assert_eq!(run.lines[0], "system/init");
        assert!(run.lines[1].starts_with("result/success"));
    }

    #[test]
    fn long_lines_are_cut_on_a_char_boundary() {
        let line = format!(
            r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
            "é".repeat(300)
        );
        let shown = event_line(&parse_line(&line));
        assert_eq!(shown.chars().count(), 201);
        assert!(shown.ends_with('…'));
    }
}
