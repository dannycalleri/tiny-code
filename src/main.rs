use std::path::PathBuf;

use eframe::egui;

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
        Box::new(|_cc| Ok(Box::new(App { dir }))),
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.label(self.dir.display().to_string());
        });
        egui::CentralPanel::default().show(ui, |_ui| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
