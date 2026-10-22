use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui;
use egui_commonmark::CommonMarkCache;

use crate::outline;
use crate::run::{Run, RunStatus};
use crate::runner::{self, RunMsg, Stopper};
use crate::session::Session;
use crate::store::{self, IndexEntry, Store};
use crate::transcript::{self, View};

pub struct App {
    dir: PathBuf,
    claude: String,
    store: Store,
    history: Vec<IndexEntry>,
    prompt: String,
    session: Session,
    events: Option<Receiver<RunMsg>>,
    stopper: Option<Stopper>,
    markdown: CommonMarkCache,
    view: View,
    outline: Option<bool>,
}

impl App {
    pub fn new(dir: PathBuf, claude: String, store: Store) -> Self {
        App {
            dir,
            claude,
            history: store.history(),
            store,
            prompt: String::new(),
            session: Session::default(),
            events: None,
            stopper: None,
            markdown: CommonMarkCache::default(),
            view: View::default(),
            outline: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.session.is_running()
    }

    pub fn poll(&mut self) {
        let Some(events) = &self.events else { return };
        while let Ok(msg) = events.try_recv() {
            self.session.apply(msg);
        }
        if !self.session.is_running() {
            self.events = None;
            self.stopper = None;
            self.save_index();
        }
    }

    fn save_index(&mut self) {
        let (Some(id), Some(first), Some(last)) = (
            self.session.session_id(),
            self.session.runs.first(),
            self.session.current(),
        ) else {
            return;
        };
        let now = store::now();
        let entry = IndexEntry {
            session_id: id.to_string(),
            title: first.prompt.lines().next().unwrap_or_default().to_string(),
            started_at: first.started_at.unwrap_or(now),
            updated_at: now,
            cost_usd: self.session.totals().and_then(|t| t.cost_usd),
            status: format!("{:?}", last.status).to_lowercase(),
        };
        if let Err(error) = self.store.save_entry(entry) {
            eprintln!("turbo-harness: cannot save the session index: {error}");
        }
        self.history = self.store.history();
    }

    fn open_session(&mut self, session_id: &str) {
        self.session = Session::replay(&self.store.read_log(session_id));
        self.view = View::default();
        self.view.jump_to_latest();
    }

    fn new_session(&mut self) {
        self.session = Session::default();
        self.view = View::default();
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let mut open = None;
        ui.add_enabled_ui(!self.is_running(), |ui| {
            if ui.button("New session").clicked() {
                self.new_session();
            }
            ui.separator();
            let now = store::now();
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    for entry in &self.history {
                        let selected = self.session.session_id() == Some(&entry.session_id);
                        let title = egui::Button::selectable(selected, &entry.title).truncate();
                        if ui.add(title).clicked() {
                            open = Some(entry.session_id.clone());
                        }
                        let detail = format!(
                            "{} · {}",
                            ago(now, entry.updated_at),
                            transcript::cost_label(entry.cost_usd)
                        );
                        ui.label(egui::RichText::new(detail).small().weak());
                        ui.add_space(4.0);
                    }
                });
        });
        if let Some(id) = open {
            self.open_session(&id);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.is_running() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let show_outline = outline::show_outline(ui.max_rect().width(), self.outline);
        egui::Panel::left("history")
            .default_size(220.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.dir.display().to_string());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(show_outline, "Outline").clicked() {
                        self.outline = Some(!show_outline);
                    }
                    if let Some(run) = self.session.current() {
                        status_badge(ui, run.status);
                    }
                });
            });
        });
        if let Some(run) = self
            .session
            .current()
            .filter(|r| r.status != RunStatus::Running)
        {
            egui::Panel::bottom("footer").show(ui, |ui| {
                let mut line = format!("Last run: {}", transcript::stats_line(run.stats.as_ref()));
                if self.session.runs.len() > 1 {
                    let totals = self.session.totals();
                    line += &format!("   Session: {}", transcript::stats_line(totals.as_ref()));
                }
                ui.label(egui::RichText::new(line).small().weak());
            });
        }
        egui::Panel::bottom("composer").show(ui, |ui| {
            let id = egui::Id::unique("composer_text");
            let enter = ui.memory(|m| m.has_focus(id)) && ui.input_mut(take_send_key);
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.prompt)
                        .id(id)
                        .desired_rows(2)
                        .desired_width(ui.available_width() - 70.0)
                        .hint_text("Ask Claude… (Enter sends, Shift+Enter adds a line)"),
                );
                if self.is_running() {
                    if ui.button("⏹ Stop").clicked() {
                        self.stop();
                    }
                } else if enter || ui.button("Send").clicked() {
                    self.send(ui.ctx());
                }
            });
        });
        if show_outline {
            let clicked = egui::Panel::right("outline")
                .default_size(240.0)
                .show(ui, |ui| {
                    ui.strong("Outline");
                    ui.separator();
                    outline::show(ui, &outline::outline(&self.session.runs))
                })
                .inner;
            if let Some(target) = clicked {
                let now = ui.input(|i| i.time);
                self.view.reveal(&self.session.runs, target, now);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            transcript::show(ui, &self.session.runs, &mut self.markdown, &mut self.view);
        });
    }

    fn send(&mut self, ctx: &egui::Context) {
        if !self.session.can_send(&self.prompt) {
            return;
        }
        let prompt = self.prompt.trim().to_string();
        let cmd =
            runner::claude_command(&self.claude, &prompt, self.session.session_id(), &self.dir);
        let started_at = store::now();
        let mut log = self.store.log_writer(self.session.session_id());
        log.write(&store::prompt_line(&prompt, started_at));
        let ctx = ctx.clone();
        match runner::start(cmd, Some(log), move || ctx.request_repaint()) {
            Ok(handle) => {
                let mut run = Run::new(&prompt);
                run.started_at = Some(started_at);
                self.session.begin(run);
                self.events = Some(handle.events);
                self.stopper = Some(handle.stopper);
            }
            Err(error) => self.session.begin(Run::failed_to_start(&prompt, error)),
        }
        self.prompt.clear();
        self.view.jump_to_latest();
    }
}

impl App {
    fn stop(&mut self) {
        if let (Some(stopper), Some(run)) = (&self.stopper, self.session.current_mut()) {
            run.request_stop();
            stopper.stop();
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

fn ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..60 => "just now".to_string(),
        60..3600 => format!("{} min ago", seconds / 60),
        3600..86_400 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

fn take_send_key(input: &mut egui::InputState) -> bool {
    // consume_key ignores an extra Shift, so without this check Shift+Enter would also send.
    !input.modifiers.shift && input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
}

fn status_badge(ui: &mut egui::Ui, status: RunStatus) {
    let visuals = ui.visuals();
    let (text, color) = match status {
        RunStatus::Running => ("⏺ running", visuals.warn_fg_color),
        RunStatus::Done => ("✔ done", transcript::OK_GREEN),
        RunStatus::Failed => ("✖ failed", visuals.error_fg_color),
        RunStatus::Stopped => ("⏹ stopped", visuals.weak_text_color()),
    };
    ui.label(egui::RichText::new(text).color(color).strong());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enter_pressed(modifiers: egui::Modifiers) -> egui::InputState {
        let mut input = egui::InputState::default();
        input.modifiers = modifiers;
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        input
    }

    #[test]
    fn ago_picks_the_largest_whole_unit() {
        let now = 1_000_000;
        assert_eq!(ago(now, now - 59), "just now");
        assert_eq!(ago(now, now - 60), "1 min ago");
        assert_eq!(ago(now, now - 3599), "59 min ago");
        assert_eq!(ago(now, now - 3600), "1 h ago");
        assert_eq!(ago(now, now - 86_400 * 3), "3 d ago");
        assert_eq!(ago(now, now + 5), "just now");
    }

    #[test]
    fn enter_sends_and_is_consumed() {
        let mut input = enter_pressed(egui::Modifiers::NONE);
        assert!(take_send_key(&mut input));
        assert!(input.events.is_empty());
    }

    #[test]
    fn shift_enter_does_not_send_and_reaches_the_text_edit() {
        let mut input = enter_pressed(egui::Modifiers::SHIFT);
        assert!(!take_send_key(&mut input));
        assert_eq!(input.events.len(), 1);
    }
}
