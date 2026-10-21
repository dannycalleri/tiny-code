use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use eframe::egui;
use turbo_harness::run::{Run, RunStatus};
use turbo_harness::runner::{self, RunMsg};

fn main() -> eframe::Result {
    let dir = match working_dir(std::env::args_os().nth(1).map(PathBuf::from)) {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("turbo-harness: {message}");
            std::process::exit(2);
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("turbo-harness")
            .with_inner_size([1000.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "turbo-harness",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(dir)))),
    )
}

fn working_dir(arg: Option<PathBuf>) -> Result<PathBuf, String> {
    let path = match arg {
        Some(path) => path,
        None => std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?,
    };
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if !path.is_dir() {
        return Err(format!("{} is not a directory", path.display()));
    }
    Ok(path)
}

struct App {
    dir: PathBuf,
    claude: String,
    prompt: String,
    run: Option<Run>,
    events: Option<Receiver<RunMsg>>,
}

impl App {
    fn new(dir: PathBuf) -> Self {
        App {
            dir,
            claude: std::env::var("TURBO_HARNESS_CLAUDE").unwrap_or_else(|_| "claude".into()),
            prompt: String::new(),
            run: None,
            events: None,
        }
    }

    fn is_running(&self) -> bool {
        self.run
            .as_ref()
            .is_some_and(|r| r.status == RunStatus::Running)
    }

    fn send(&mut self, ctx: &egui::Context) {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() || self.is_running() {
            return;
        }
        let cmd = runner::claude_command(&self.claude, &prompt, &self.dir);
        let ctx = ctx.clone();
        match runner::start(cmd, move || ctx.request_repaint()) {
            Ok(events) => {
                self.run = Some(Run::new(&prompt));
                self.events = Some(events);
            }
            Err(error) => self.run = Some(Run::failed_to_start(&prompt, error)),
        }
        self.prompt.clear();
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let (Some(events), Some(run)) = (&self.events, &mut self.run) else {
            return;
        };
        while let Ok(msg) = events.try_recv() {
            run.apply(msg);
        }
        if run.status != RunStatus::Running {
            self.events = None;
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.dir.display().to_string());
                if let Some(run) = &self.run {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(status_label(run.status));
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
                let button = ui.add_enabled(!self.is_running(), egui::Button::new("Send"));
                if enter || button.clicked() {
                    self.send(ui.ctx());
                }
            });
        });
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(run) = &self.run else { return };
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.strong(&run.prompt);
                    for line in &run.lines {
                        ui.monospace(line);
                    }
                    if let Some(error) = &run.error {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    }
                });
        });
    }
}

fn take_send_key(input: &mut egui::InputState) -> bool {
    // consume_key ignores an extra Shift, so without this check Shift+Enter would also send.
    !input.modifiers.shift && input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
}

fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "● running",
        RunStatus::Done => "✔ done",
        RunStatus::Failed => "✖ failed",
    }
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

    #[test]
    fn working_dir_defaults_to_current_dir() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(working_dir(None).unwrap(), expected);
    }

    #[test]
    fn working_dir_rejects_a_file() {
        let err = working_dir(Some(PathBuf::from("Cargo.toml"))).unwrap_err();
        assert!(err.ends_with("is not a directory"), "{err}");
    }

    #[test]
    fn working_dir_rejects_a_missing_path() {
        assert!(working_dir(Some(PathBuf::from("no/such/dir"))).is_err());
    }
}
