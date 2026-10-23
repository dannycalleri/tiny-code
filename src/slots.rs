use std::sync::mpsc::Receiver;

use crate::run::RunStatus;
use crate::runner::{RunMsg, Stopper};
use crate::session::Session;
use crate::store::IndexEntry;
use crate::transcript::View;

#[derive(Debug)]
pub struct Slot {
    pub project: usize,
    pub session: Session,
    pub events: Option<Receiver<RunMsg>>,
    pub stopper: Option<Stopper>,
    pub view: View,
    pub prompt: String,
    pub unseen_failure: bool,
}

impl Slot {
    pub fn new(project: usize, session: Session) -> Slot {
        Slot {
            project,
            session,
            events: None,
            stopper: None,
            view: View::default(),
            prompt: String::new(),
            unseen_failure: false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Row<'a> {
    Live {
        slot: usize,
        title: String,
    },
    Entry {
        entry: &'a IndexEntry,
        slot: Option<usize>,
    },
}

pub fn keep(slot: &Slot, selected: bool) -> bool {
    selected || slot.session.is_running() || !slot.prompt.trim().is_empty() || slot.unseen_failure
}

pub fn run_ended(slot: &mut Slot, selected: bool) {
    let failed = slot
        .session
        .current()
        .is_some_and(|run| run.status == RunStatus::Failed);
    slot.unseen_failure = failed && !selected;
}

pub fn prune(slots: &mut Vec<Slot>, selected: usize) -> usize {
    let mut index = 0;
    let mut kept = 0;
    let mut now_selected = 0;
    slots.retain(|slot| {
        let keep = keep(slot, index == selected);
        if index == selected {
            now_selected = kept;
        }
        kept += usize::from(keep);
        index += 1;
        keep
    });
    now_selected
}

pub fn select(slots: &mut Vec<Slot>, next: usize) -> usize {
    slots[next].unseen_failure = false;
    prune(slots, next)
}

pub fn find(slots: &[Slot], project: usize, session_id: &str) -> Option<usize> {
    slots
        .iter()
        .position(|s| s.project == project && s.session.session_id() == Some(session_id))
}

pub fn open(
    slots: &mut Vec<Slot>,
    project: usize,
    session_id: &str,
    read_log: impl FnOnce() -> Vec<String>,
) -> usize {
    if let Some(index) = find(slots, project, session_id) {
        return index;
    }
    let mut slot = Slot::new(project, Session::replay(&read_log()));
    slot.view.jump_to_latest();
    slots.push(slot);
    slots.len() - 1
}

pub fn new_session(slots: &mut Vec<Slot>, project: usize) -> usize {
    if let Some(index) = slots
        .iter()
        .position(|s| s.project == project && s.session.runs.is_empty())
    {
        return index;
    }
    slots.push(Slot::new(project, Session::default()));
    slots.len() - 1
}

pub fn rows<'a>(slots: &[Slot], project: usize, history: &'a [IndexEntry]) -> Vec<Row<'a>> {
    let live = slots.iter().enumerate().filter(|(_, slot)| {
        slot.project == project
            && !slot.session.runs.is_empty()
            && !history
                .iter()
                .any(|e| slot.session.session_id() == Some(&e.session_id))
    });
    let live = live.map(|(index, slot)| Row::Live {
        slot: index,
        title: slot.session.runs[0]
            .prompt
            .lines()
            .next()
            .unwrap_or_default()
            .to_string(),
    });
    let entries = history.iter().map(|entry| Row::Entry {
        entry,
        slot: find(slots, project, &entry.session_id),
    });
    live.chain(entries).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse_line;
    use crate::run::Run;

    fn idle(project: usize) -> Slot {
        Slot::new(project, Session::default())
    }

    fn running(project: usize, prompt: &str, session_id: Option<&str>) -> Slot {
        let mut session = Session::default();
        session.begin(Run::new(prompt));
        if let Some(id) = session_id {
            let init = format!(r#"{{"type":"system","subtype":"init","session_id":"{id}"}}"#);
            session.apply(RunMsg::Event(parse_line(&init)));
        }
        Slot::new(project, session)
    }

    fn exit(slot: &mut Slot, code: i32) {
        slot.session.apply(RunMsg::Exited {
            code: Some(code),
            stderr: vec!["boom".into()],
        });
    }

    fn entry(id: &str) -> IndexEntry {
        IndexEntry {
            session_id: id.into(),
            title: id.into(),
            started_at: 1,
            updated_at: 1,
            cost_usd: None,
            status: "done".into(),
        }
    }

    #[test]
    fn a_slot_with_no_reason_to_stay_is_not_kept() {
        assert!(!keep(&idle(0), false));
    }

    #[test]
    fn each_keep_condition_alone_keeps_the_slot() {
        assert!(keep(&idle(0), true));
        assert!(keep(&running(0, "p", None), false));
        let mut draft = idle(0);
        draft.prompt = "half a thought".into();
        assert!(keep(&draft, false));
        let mut failed = idle(0);
        failed.unseen_failure = true;
        assert!(keep(&failed, false));
    }

    #[test]
    fn a_blank_draft_does_not_keep_the_slot() {
        let mut slot = idle(0);
        slot.prompt = "  \n".into();
        assert!(!keep(&slot, false));
    }

    #[test]
    fn a_failure_while_the_slot_is_selected_counts_as_seen() {
        let mut seen = running(0, "p", None);
        exit(&mut seen, 1);
        run_ended(&mut seen, true);
        assert!(!keep(&seen, false));

        let mut unseen = running(0, "p", None);
        exit(&mut unseen, 1);
        run_ended(&mut unseen, false);
        assert!(keep(&unseen, false));
    }

    #[test]
    fn a_successful_run_leaves_nothing_unseen() {
        let mut slot = running(0, "p", None);
        exit(&mut slot, 0);
        run_ended(&mut slot, false);
        assert!(!keep(&slot, false));
    }

    #[test]
    fn selecting_a_slot_marks_its_failure_seen_and_removes_the_idle_one_left() {
        let mut failed = running(0, "p", None);
        exit(&mut failed, 1);
        run_ended(&mut failed, false);
        let mut slots = vec![idle(0), failed];
        assert_eq!(select(&mut slots, 1), 0);
        assert_eq!(slots.len(), 1);
        assert!(!slots[0].unseen_failure);
    }

    #[test]
    fn prune_keeps_the_selected_slot_and_gives_its_new_index() {
        let mut slots = vec![idle(0), running(0, "a", None), idle(1), idle(1)];
        assert_eq!(prune(&mut slots, 2), 1);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[1].project, 1);
    }

    #[test]
    fn opening_a_session_that_has_a_slot_selects_it_without_a_replay() {
        let mut slots = vec![idle(0), running(1, "p", Some("abc"))];
        let index = open(&mut slots, 1, "abc", || panic!("replayed the log"));
        assert_eq!(index, 1);
        assert_eq!(slots.len(), 2);
    }

    #[test]
    fn opening_a_session_without_a_slot_replays_its_log_into_a_new_slot() {
        let mut slots = vec![running(0, "p", Some("abc"))];
        let log = vec![
            crate::store::prompt_line("old prompt", 1),
            r#"{"type":"system","subtype":"init","session_id":"abc"}"#.to_string(),
        ];
        let index = open(&mut slots, 1, "abc", || log);
        assert_eq!(index, 1);
        assert_eq!(slots[1].project, 1);
        assert_eq!(slots[1].session.runs[0].prompt, "old prompt");
    }

    #[test]
    fn new_session_makes_an_empty_slot_for_the_project() {
        let mut slots = vec![idle(0)];
        let index = new_session(&mut slots, 3);
        assert_eq!(index, 1);
        assert_eq!(slots[1].project, 3);
        assert_eq!(slots[1].session, Session::default());
    }

    #[test]
    fn rows_put_a_running_slot_without_an_entry_on_top() {
        let slots = vec![idle(0), running(0, "Fix the bug\nand more", Some("new"))];
        let history = [entry("old")];
        assert_eq!(
            rows(&slots, 0, &history),
            [
                Row::Live {
                    slot: 1,
                    title: "Fix the bug".into()
                },
                Row::Entry {
                    entry: &history[0],
                    slot: None
                },
            ]
        );
    }

    #[test]
    fn new_session_returns_the_empty_slot_the_project_already_has() {
        let mut draft = idle(3);
        draft.prompt = "unsent".into();
        let mut slots = vec![running(3, "p", None), draft];
        assert_eq!(new_session(&mut slots, 3), 1);
        assert_eq!(slots.len(), 2);
    }

    #[test]
    fn rows_list_an_unseen_failure_that_has_no_entry() {
        let mut failed = running(0, "Crash early", None);
        exit(&mut failed, 1);
        run_ended(&mut failed, false);
        assert_eq!(
            rows(&[failed], 0, &[]),
            [Row::Live {
                slot: 0,
                title: "Crash early".into()
            }]
        );
    }

    #[test]
    fn rows_list_a_draft_after_a_run_that_never_reported_a_session() {
        let mut slot = idle(0);
        slot.session
            .begin(Run::failed_to_start("First try", "not found".into()));
        slot.prompt = "Second try".into();
        assert!(keep(&slot, false));
        assert_eq!(
            rows(&[slot], 0, &[]),
            [Row::Live {
                slot: 0,
                title: "First try".into()
            }]
        );
    }

    #[test]
    fn rows_list_a_slot_with_an_entry_only_once() {
        let slots = vec![running(0, "p", Some("old"))];
        let history = [entry("old")];
        assert_eq!(
            rows(&slots, 0, &history),
            [Row::Entry {
                entry: &history[0],
                slot: Some(0)
            }]
        );
    }

    #[test]
    fn rows_show_only_the_slots_of_their_project() {
        let slots = vec![running(1, "elsewhere", None)];
        assert_eq!(rows(&slots, 0, &[]), []);
    }
}
