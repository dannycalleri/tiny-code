use serde_json::Value;

use crate::event::parse_line;
use crate::run::{Run, RunStats, RunStatus};
use crate::runner::RunMsg;

const NO_EXIT: &str = "The log ends before this run did; the app may have closed during the run.";

#[derive(Debug, Default, PartialEq)]
pub struct Session {
    pub runs: Vec<Run>,
    session_id: Option<String>,
}

impl Session {
    pub fn replay(lines: &[String]) -> Session {
        let mut session = Session::default();
        for line in lines {
            let raw: Value = serde_json::from_str(line).unwrap_or_default();
            match raw["type"].as_str() {
                Some("turbo_prompt") => {
                    session.close_unfinished_run();
                    session.begin(Run::new(raw["prompt"].as_str().unwrap_or_default()));
                }
                Some("turbo_exit") => {
                    if raw["stopped"] == true
                        && let Some(run) = session.current_mut()
                    {
                        run.request_stop();
                    }
                    session.apply(RunMsg::Exited {
                        code: raw["code"].as_i64().map(|c| c as i32),
                        stderr: serde_json::from_value(raw["stderr"].clone()).unwrap_or_default(),
                    });
                }
                _ => session.apply(RunMsg::Event(parse_line(line))),
            }
        }
        session.close_unfinished_run();
        session
    }

    fn close_unfinished_run(&mut self) {
        if self.is_running() {
            self.apply(RunMsg::Exited {
                code: None,
                stderr: vec![NO_EXIT.to_string()],
            });
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn current(&self) -> Option<&Run> {
        self.runs.last()
    }

    pub fn current_mut(&mut self) -> Option<&mut Run> {
        self.runs.last_mut()
    }

    pub fn is_running(&self) -> bool {
        self.current()
            .is_some_and(|run| run.status == RunStatus::Running)
    }

    pub fn totals(&self) -> Option<RunStats> {
        let stats: Vec<RunStats> = self.runs.iter().filter_map(|run| run.stats).collect();
        if stats.is_empty() {
            return None;
        }
        Some(RunStats {
            cost_usd: sum(stats.iter().map(|s| s.cost_usd)),
            duration_ms: sum(stats.iter().map(|s| s.duration_ms)),
            turns: sum(stats.iter().map(|s| s.turns)),
        })
    }

    pub fn can_send(&self, prompt: &str) -> bool {
        !prompt.trim().is_empty() && !self.is_running()
    }

    pub fn begin(&mut self, run: Run) {
        self.runs.push(run);
    }

    pub fn apply(&mut self, msg: RunMsg) {
        let Some(run) = self.runs.last_mut() else {
            return;
        };
        run.apply(msg);
        if let Some(id) = &run.session_id {
            self.session_id = Some(id.clone());
        }
    }
}

fn sum<T: std::iter::Sum<T>>(values: impl Iterator<Item = Option<T>>) -> Option<T> {
    let mut present = values.flatten().peekable();
    present.peek()?;
    Some(present.sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse_line;

    fn event(line: &str) -> RunMsg {
        RunMsg::Event(parse_line(line))
    }

    fn log(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"abc"}"#;
    const OK: &str =
        r#"{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.5}"#;

    #[test]
    fn replay_splits_the_log_into_runs_at_each_prompt() {
        let ok = crate::store::exit_line(Some(0), &[], false);
        let lines = [
            crate::store::prompt_line("first", 1),
            INIT.into(),
            OK.into(),
            ok.clone(),
            crate::store::prompt_line("second", 2),
            INIT.into(),
            OK.into(),
            ok,
        ];
        let session = Session::replay(&lines);
        let prompts: Vec<&str> = session.runs.iter().map(|r| r.prompt.as_str()).collect();
        assert_eq!(prompts, ["first", "second"]);
        assert!(session.runs.iter().all(|r| r.status == RunStatus::Done));
        assert_eq!(session.session_id(), Some("abc"));
        assert_eq!(session.totals().unwrap().cost_usd, Some(1.0));
    }

    #[test]
    fn replay_keeps_a_stopped_run_stopped() {
        let lines = [
            crate::store::prompt_line("hang", 1),
            INIT.into(),
            crate::store::exit_line(None, &[], true),
        ];
        assert_eq!(Session::replay(&lines).runs[0].status, RunStatus::Stopped);
    }

    #[test]
    fn replay_fails_a_run_whose_exit_is_missing_so_the_session_can_continue() {
        let lines = log(&[r#"{"type":"turbo_prompt","prompt":"cut"}"#, INIT]);
        let session = Session::replay(&lines);
        assert_eq!(session.runs[0].status, RunStatus::Failed);
        assert_eq!(session.runs[0].error.as_deref(), Some(NO_EXIT));
        assert!(session.can_send("next"));

        let mut lines = lines;
        lines.extend(log(&[r#"{"type":"turbo_prompt","prompt":"next"}"#]));
        let session = Session::replay(&lines);
        assert_eq!(session.runs[0].status, RunStatus::Failed);
        assert_eq!(session.runs.len(), 2);
    }

    #[test]
    fn a_new_session_has_no_id() {
        assert_eq!(Session::default().session_id(), None);
    }

    #[test]
    fn the_id_comes_from_init_and_outlives_the_run() {
        let mut session = Session::default();
        session.begin(Run::new("hi"));
        session.apply(event(
            r#"{"type":"system","subtype":"init","session_id":"abc"}"#,
        ));
        session.apply(RunMsg::Exited {
            code: Some(0),
            stderr: vec![],
        });
        session.begin(Run::new("again"));
        assert_eq!(session.session_id(), Some("abc"));
    }

    #[test]
    fn the_id_also_comes_from_the_result() {
        let mut session = Session::default();
        session.begin(Run::new("hi"));
        session.apply(event(
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"xyz"}"#,
        ));
        assert_eq!(session.session_id(), Some("xyz"));
    }

    fn finished_run(result: &str) -> Run {
        let mut run = Run::new("p");
        run.apply(event(result));
        run.apply(RunMsg::Exited {
            code: Some(0),
            stderr: vec![],
        });
        run
    }

    #[test]
    fn earlier_runs_are_kept_in_order() {
        let mut session = Session::default();
        session.begin(Run::new("first"));
        session.apply(RunMsg::Exited {
            code: Some(0),
            stderr: vec![],
        });
        session.begin(Run::new("second"));
        let prompts: Vec<&str> = session.runs.iter().map(|r| r.prompt.as_str()).collect();
        assert_eq!(prompts, ["first", "second"]);
        assert_eq!(session.current().unwrap().prompt, "second");
    }

    #[test]
    fn totals_sum_two_runs_and_skip_a_run_without_stats() {
        let mut session = Session::default();
        session.runs.push(finished_run(
            r#"{"type":"result","subtype":"success","total_cost_usd":0.25,"duration_ms":1000,"num_turns":2}"#,
        ));
        session
            .runs
            .push(Run::failed_to_start("p", "no claude".into()));
        session.runs.push(finished_run(
            r#"{"type":"result","subtype":"success","total_cost_usd":0.5,"num_turns":3}"#,
        ));
        let totals = session.totals().unwrap();
        assert_eq!(totals.cost_usd, Some(0.75));
        assert_eq!(totals.duration_ms, Some(1000));
        assert_eq!(totals.turns, Some(5));
    }

    #[test]
    fn no_totals_without_any_stats() {
        let mut session = Session::default();
        session
            .runs
            .push(Run::failed_to_start("p", "no claude".into()));
        assert_eq!(session.totals(), None);
    }

    #[test]
    fn the_composer_cannot_send_while_a_run_is_active() {
        let mut session = Session::default();
        assert!(session.can_send("hi"));
        assert!(!session.can_send("   "));
        session.begin(Run::new("hi"));
        assert!(!session.can_send("next"));
        session.apply(RunMsg::Exited {
            code: Some(0),
            stderr: vec![],
        });
        assert!(session.can_send("next"));
    }
}
