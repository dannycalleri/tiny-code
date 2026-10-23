use crate::event::Event;
use crate::runner::RunMsg;
use crate::trajectory::Trajectory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunStats {
    pub cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub turns: Option<u32>,
}

#[derive(Debug, PartialEq)]
pub struct Run {
    pub prompt: String,
    pub trajectory: Trajectory,
    pub status: RunStatus,
    pub error: Option<String>,
    pub stats: Option<RunStats>,
    pub session_id: Option<String>,
    pub started_at: Option<u64>,
    result_is_error: Option<bool>,
    stop_requested: bool,
}

impl Run {
    pub fn new(prompt: &str) -> Self {
        Run {
            prompt: prompt.to_string(),
            trajectory: Trajectory::default(),
            status: RunStatus::Running,
            error: None,
            stats: None,
            session_id: None,
            started_at: None,
            result_is_error: None,
            stop_requested: false,
        }
    }

    pub fn failed_to_start(prompt: &str, error: String) -> Self {
        Run {
            status: RunStatus::Failed,
            error: Some(error),
            ..Run::new(prompt)
        }
    }

    pub fn request_stop(&mut self) {
        self.stop_requested = true;
    }

    pub fn apply(&mut self, msg: RunMsg) {
        match msg {
            RunMsg::Event(event) => {
                let session_id = match &event {
                    Event::System(s) => s.session_id.as_ref(),
                    Event::Result(r) => r.session_id.as_ref(),
                    _ => None,
                };
                if let Some(id) = session_id {
                    self.session_id = Some(id.clone());
                }
                if let Event::Result(result) = &event {
                    self.result_is_error = Some(result.is_error);
                    self.stats = Some(RunStats {
                        cost_usd: result.total_cost_usd,
                        duration_ms: result.duration_ms,
                        turns: result.num_turns,
                    });
                }
                self.trajectory.push(&event);
            }
            RunMsg::Exited { code, stderr } => {
                self.status = if self.stop_requested {
                    RunStatus::Stopped
                } else {
                    final_status(self.result_is_error, code)
                };
                self.trajectory.interrupt_open_tools();
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
    fn stats_come_from_the_result_event() {
        let hello: Vec<&str> = include_str!("../tests/fixtures/hello.jsonl")
            .lines()
            .collect();
        let run = finished(&hello, Some(0), &[]);
        let stats = run.stats.unwrap();
        assert_eq!((stats.duration_ms, stats.turns), (Some(2771), Some(1)));
        assert!((stats.cost_usd.unwrap() - 0.1938238).abs() < 1e-9);
    }

    #[test]
    fn a_run_without_a_result_has_no_stats() {
        assert_eq!(finished(&[], Some(0), &[]).stats, None);
    }

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
    fn a_run_asked_to_stop_ends_as_stopped() {
        let mut run = Run::new("hi");
        run.request_stop();
        run.apply(RunMsg::Exited {
            code: None,
            stderr: vec!["Terminated".into()],
        });
        assert_eq!(run.status, RunStatus::Stopped);
        assert_eq!(run.error, None);
    }

    #[test]
    fn open_tool_calls_are_interrupted_when_the_run_ends() {
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}"#;
        let run = finished(&[call], Some(1), &[]);
        let crate::trajectory::Node::Tool(tool) = &run.trajectory.nodes[0] else {
            panic!()
        };
        assert_eq!(tool.status, crate::trajectory::ToolStatus::Interrupted);
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
}
