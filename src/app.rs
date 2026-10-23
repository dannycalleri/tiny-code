use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui;
use egui_commonmark::CommonMarkCache;

use crate::outline;
use crate::projects::{self, Projects};
use crate::run::{Run, RunStatus};
use crate::runner;
use crate::session::Session;
use crate::slots::{self, Row, Slot};
use crate::store::{self, IndexEntry, Store};
use crate::transcript;

struct ProjectStore {
    store: Store,
    history: Vec<IndexEntry>,
}

enum SidebarClick {
    Toggle(usize),
    NewSession(usize),
    Open(usize, String),
    Select(usize),
}

pub type FolderPicker = Box<dyn FnMut() -> Receiver<Option<PathBuf>>>;

pub struct App {
    data_dir: PathBuf,
    claude: String,
    projects: Projects,
    stores: Vec<ProjectStore>,
    slots: Vec<Slot>,
    selected: usize,
    markdown: CommonMarkCache,
    outline: Option<bool>,
    pick_folder: FolderPicker,
    picking: Option<Receiver<Option<PathBuf>>>,
}

impl ProjectStore {
    fn open(data_dir: &Path, dir: &Path) -> Self {
        let store = Store::open(data_dir, dir);
        ProjectStore {
            history: store.history(),
            store,
        }
    }
}

impl App {
    pub fn new(
        data_dir: PathBuf,
        claude: String,
        projects: Projects,
        launch: usize,
        pick_folder: FolderPicker,
    ) -> Self {
        let stores = projects
            .entries
            .iter()
            .map(|entry| ProjectStore::open(&data_dir, &entry.path))
            .collect();
        App {
            data_dir,
            claude,
            projects,
            stores,
            slots: vec![Slot::new(launch, Session::default())],
            selected: 0,
            markdown: CommonMarkCache::default(),
            outline: None,
            pick_folder,
            picking: None,
        }
    }

    fn slot(&self) -> &Slot {
        &self.slots[self.selected]
    }

    fn dir(&self) -> &Path {
        &self.projects.entries[self.slot().project].path
    }

    pub fn is_running(&self) -> bool {
        self.running_sessions() > 0
    }

    pub fn running_sessions(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.session.is_running())
            .count()
    }

    pub fn poll(&mut self) {
        if let Some(picking) = &self.picking {
            match picking.try_recv() {
                Ok(folder) => {
                    self.picking = None;
                    if let Some(folder) = folder {
                        self.add_project(&folder);
                    }
                }
                Err(TryRecvError::Disconnected) => self.picking = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        let mut reported = Vec::new();
        let mut ended = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let Some(events) = &slot.events else { continue };
            let had_id = run_has_session_id(&slot.session);
            while let Ok(msg) = events.try_recv() {
                slot.session.apply(msg);
            }
            if !had_id && run_has_session_id(&slot.session) {
                reported.push(index);
            }
            if !slot.session.is_running() {
                slot.events = None;
                slot.stopper = None;
                ended.push(index);
            }
        }
        for &index in &reported {
            self.save_index(index);
        }
        for &index in &ended {
            self.save_index(index);
            slots::run_ended(&mut self.slots[index], index == self.selected);
        }
        if !ended.is_empty() {
            self.selected = slots::prune(&mut self.slots, self.selected);
        }
    }

    fn save_index(&mut self, slot: usize) {
        let Slot {
            project, session, ..
        } = &self.slots[slot];
        let (Some(id), Some(first), Some(last)) = (
            session.session_id(),
            session.runs.first(),
            session.current(),
        ) else {
            return;
        };
        let now = store::now();
        let entry = IndexEntry {
            session_id: id.to_string(),
            title: first.prompt.lines().next().unwrap_or_default().to_string(),
            started_at: first.started_at.unwrap_or(now),
            updated_at: now,
            cost_usd: session.totals().and_then(|t| t.cost_usd),
            status: format!("{:?}", last.status).to_lowercase(),
        };
        let project = &mut self.stores[*project];
        if let Err(error) = project.store.save_entry(entry) {
            eprintln!("turbo-harness: cannot save the session index: {error}");
        }
        project.history = project.store.history();
    }

    fn select(&mut self, slot: usize) {
        self.selected = slots::select(&mut self.slots, slot);
    }

    fn open_session(&mut self, project: usize, session_id: &str) {
        let store = &self.stores[project].store;
        let slot = slots::open(&mut self.slots, project, session_id, || {
            store.read_log(session_id)
        });
        self.select(slot);
    }

    fn new_session(&mut self, project: usize) {
        let slot = slots::new_session(&mut self.slots, project);
        self.select(slot);
    }

    fn add_project(&mut self, dir: &Path) {
        let index = self.projects.add(dir);
        if index == self.stores.len() {
            let path = &self.projects.entries[index].path;
            self.stores.push(ProjectStore::open(&self.data_dir, path));
        }
        self.save_projects();
    }

    fn toggle_project(&mut self, project: usize) {
        self.projects.toggle(project);
        self.save_projects();
    }

    fn save_projects(&self) {
        if let Err(error) = self.projects.save(&projects::file(&self.data_dir)) {
            eprintln!("turbo-harness: cannot save the project list: {error}");
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let mut click = None;
        let labels = self.projects.labels();
        let add = egui::Button::new("Add project…");
        if ui.add_enabled(self.picking.is_none(), add).clicked() {
            self.picking = Some((self.pick_folder)());
        }
        ui.separator();
        let now = store::now();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                for (index, (project, label)) in
                    self.projects.entries.iter().zip(&labels).enumerate()
                {
                    let path = project.path.display().to_string();
                    let (text, hover) = if project.path.is_dir() {
                        (label.clone(), path)
                    } else {
                        (format!("⚠ {label}"), format!("{path} does not exist"))
                    };
                    let header = egui::Button::new(egui::RichText::new(text).strong())
                        .frame(false)
                        .truncate();
                    let running = self
                        .slots
                        .iter()
                        .filter(|s| s.project == index && s.session.is_running())
                        .count();
                    ui.horizontal(|ui| {
                        let status = projects::running_label(running).map(|status| {
                            egui::WidgetText::from(
                                egui::RichText::new(status)
                                    .small()
                                    .color(ui.visuals().warn_fg_color),
                            )
                            .into_galley(
                                ui,
                                Some(egui::TextWrapMode::Extend),
                                f32::INFINITY,
                                egui::TextStyle::Small,
                            )
                        });
                        let reserved = status
                            .as_ref()
                            .map_or(0.0, |g| g.size().x + ui.spacing().item_spacing.x);
                        let size = egui::vec2(
                            ui.available_width() - reserved,
                            ui.spacing().interact_size.y,
                        );
                        let header = ui.allocate_ui(size, |ui| ui.add(header)).inner;
                        if header.on_hover_text(hover).clicked() {
                            click = Some(SidebarClick::Toggle(index));
                        }
                        if let Some(status) = status {
                            ui.label(status);
                        }
                    });
                    if project.collapsed {
                        continue;
                    }
                    ui.indent(index, |ui| {
                        if ui.button("New session").clicked() {
                            click = Some(SidebarClick::NewSession(index));
                        }
                        let history = &self.stores[index].history;
                        for row in slots::rows(&self.slots, index, history) {
                            let (title, slot, detail) = match row {
                                Row::Live { slot, title } => (title, Some(slot), None),
                                Row::Entry { entry, slot } => {
                                    let detail = format!(
                                        "{} · {}",
                                        ago(now, entry.updated_at),
                                        transcript::cost_label(entry.cost_usd)
                                    );
                                    (entry.title.clone(), slot, Some((entry, detail)))
                                }
                            };
                            let selected = slot == Some(self.selected);
                            let button = egui::Button::selectable(selected, &title).truncate();
                            if ui.add(button).clicked() {
                                click = match (slot, &detail) {
                                    (_, Some((entry, _))) => {
                                        Some(SidebarClick::Open(index, entry.session_id.clone()))
                                    }
                                    (Some(slot), None) => Some(SidebarClick::Select(slot)),
                                    (None, None) => None,
                                };
                            }
                            let slot = slot.map(|s| &self.slots[s]);
                            if slot.is_some_and(|s| s.session.is_running()) {
                                let text = egui::RichText::new("⏺ running")
                                    .small()
                                    .color(ui.visuals().warn_fg_color);
                                ui.label(text);
                            } else if slot.is_some_and(|s| s.unseen_failure) {
                                let text = egui::RichText::new("✖ failed")
                                    .small()
                                    .color(ui.visuals().error_fg_color);
                                ui.label(text);
                            } else if let Some((_, detail)) = detail {
                                ui.label(egui::RichText::new(detail).small().weak());
                            }
                            ui.add_space(4.0);
                        }
                    });
                    ui.add_space(8.0);
                }
            });
        match click {
            Some(SidebarClick::Toggle(project)) => self.toggle_project(project),
            Some(SidebarClick::NewSession(project)) => self.new_session(project),
            Some(SidebarClick::Open(project, id)) => self.open_session(project, &id),
            Some(SidebarClick::Select(slot)) => self.select(slot),
            None => {}
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
                ui.label(self.dir().display().to_string());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(show_outline, "Outline").clicked() {
                        self.outline = Some(!show_outline);
                    }
                    if let Some(run) = self.slot().session.current() {
                        status_badge(ui, run.status);
                    }
                });
            });
        });
        let session = &self.slot().session;
        if let Some(run) = session.current().filter(|r| r.status != RunStatus::Running) {
            egui::Panel::bottom("footer").show(ui, |ui| {
                let mut line = format!("Last run: {}", transcript::stats_line(run.stats.as_ref()));
                if session.runs.len() > 1 {
                    let totals = session.totals();
                    line += &format!("   Session: {}", transcript::stats_line(totals.as_ref()));
                }
                ui.label(egui::RichText::new(line).small().weak());
            });
        }
        egui::Panel::bottom("composer").show(ui, |ui| {
            let id = egui::Id::unique("composer_text");
            let enter = ui.memory(|m| m.has_focus(id)) && ui.input_mut(take_send_key);
            let can_run = self.dir().is_dir();
            let slot = &mut self.slots[self.selected];
            let (stop, send) = ui
                .horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut slot.prompt)
                            .id(id)
                            .desired_rows(2)
                            .desired_width(ui.available_width() - 70.0)
                            .hint_text("Ask Claude… (Enter sends, Shift+Enter adds a line)"),
                    );
                    if slot.session.is_running() {
                        (ui.button("⏹ Stop").clicked(), false)
                    } else {
                        let send = ui.add_enabled(can_run, egui::Button::new("Send"));
                        (false, enter || send.clicked())
                    }
                })
                .inner;
            if stop {
                self.stop();
            } else if send {
                self.send(ui.ctx());
            }
        });
        let slot = &mut self.slots[self.selected];
        if show_outline {
            let clicked = egui::Panel::right("outline")
                .default_size(240.0)
                .show(ui, |ui| {
                    ui.strong("Outline");
                    ui.separator();
                    outline::show(ui, &outline::outline(&slot.session.runs))
                })
                .inner;
            if let Some(target) = clicked {
                let now = ui.input(|i| i.time);
                slot.view.reveal(&slot.session.runs, target, now);
            }
        }
        egui::CentralPanel::default().show(ui, |ui| {
            transcript::show(ui, &slot.session.runs, &mut self.markdown, &mut slot.view);
        });
    }

    fn send(&mut self, ctx: &egui::Context) {
        if !self.slot().session.can_send(&self.slot().prompt) || !self.dir().is_dir() {
            return;
        }
        let slot = &self.slots[self.selected];
        let prompt = slot.prompt.trim().to_string();
        let session_id = slot.session.session_id();
        let cmd = runner::claude_command(&self.claude, &prompt, session_id, self.dir());
        let started_at = store::now();
        let mut log = self.stores[slot.project].store.log_writer(session_id);
        log.write(&store::prompt_line(&prompt, started_at));
        let ctx = ctx.clone();
        let slot = &mut self.slots[self.selected];
        match runner::start(cmd, Some(log), move || ctx.request_repaint()) {
            Ok(handle) => {
                let mut run = Run::new(&prompt);
                run.started_at = Some(started_at);
                slot.session.begin(run);
                slot.events = Some(handle.events);
                slot.stopper = Some(handle.stopper);
            }
            Err(error) => slot.session.begin(Run::failed_to_start(&prompt, error)),
        }
        slot.prompt.clear();
        slot.view.jump_to_latest();
    }

    fn stop(&mut self) {
        let slot = &mut self.slots[self.selected];
        if let (Some(stopper), Some(run)) = (&slot.stopper, slot.session.current_mut()) {
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

fn run_has_session_id(session: &Session) -> bool {
    session
        .current()
        .is_some_and(|run| run.session_id.is_some())
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
