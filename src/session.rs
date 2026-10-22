use crate::run::{Run, RunStats, RunStatus};
use crate::runner::RunMsg;

#[derive(Debug, Default)]
pub struct Session {
    pub runs: Vec<Run>,
    session_id: Option<String>,
}

impl Session {
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
