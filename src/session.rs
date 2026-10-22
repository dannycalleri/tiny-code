use crate::run::{Run, RunStatus};
use crate::runner::RunMsg;

#[derive(Debug, Default)]
pub struct Session {
    pub current: Option<Run>,
    session_id: Option<String>,
}

impl Session {
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn is_running(&self) -> bool {
        self.current
            .as_ref()
            .is_some_and(|run| run.status == RunStatus::Running)
    }

    pub fn can_send(&self, prompt: &str) -> bool {
        !prompt.trim().is_empty() && !self.is_running()
    }

    pub fn begin(&mut self, run: Run) {
        self.current = Some(run);
    }

    pub fn apply(&mut self, msg: RunMsg) {
        let Some(run) = &mut self.current else { return };
        run.apply(msg);
        if let Some(id) = &run.session_id {
            self.session_id = Some(id.clone());
        }
    }
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
