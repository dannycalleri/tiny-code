use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use eframe::egui;
use egui_commonmark::CommonMarkCache;

use crate::run::{Run, RunStatus};
use crate::runner::{self, RunMsg, Stopper};
use crate::transcript::{self, View};

pub struct App {
    dir: PathBuf,
    claude: String,
    prompt: String,
    run: Option<Run>,
    events: Option<Receiver<RunMsg>>,
    stopper: Option<Stopper>,
    markdown: CommonMarkCache,
    view: View,
}

impl App {
    pub fn new(dir: PathBuf, claude: String) -> Self {
        App {
            dir,
            claude,
            prompt: String::new(),
            run: None,
            events: None,
            stopper: None,
            markdown: CommonMarkCache::default(),
            view: View::default(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|r| r.status == RunStatus::Running)
    }

    pub fn poll(&mut self) {
        let (Some(events), Some(run)) = (&self.events, &mut self.run) else {
            return;
        };
        while let Ok(msg) = events.try_recv() {
            run.apply(msg);
        }
        if run.status != RunStatus::Running {
            self.events = None;
            self.stopper = None;
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.is_running() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.dir.display().to_string());
                if let Some(run) = &self.run {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        status_badge(ui, run.status);
                    });
                }
            });
        });
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
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(run) = &self.run {
                transcript::show(ui, run, &mut self.markdown, &mut self.view);
            }
        });
    }

    fn send(&mut self, ctx: &egui::Context) {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() || self.is_running() {
            return;
        }
        let cmd = runner::claude_command(&self.claude, &prompt, &self.dir);
        let ctx = ctx.clone();
        match runner::start(cmd, move || ctx.request_repaint()) {
            Ok(handle) => {
                self.run = Some(Run::new(&prompt));
                self.events = Some(handle.events);
                self.stopper = Some(handle.stopper);
            }
            Err(error) => self.run = Some(Run::failed_to_start(&prompt, error)),
        }
        self.prompt.clear();
        self.view = View::default();
    }
}

impl App {
    fn stop(&mut self) {
        if let (Some(stopper), Some(run)) = (&self.stopper, &mut self.run) {
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
