use std::path::PathBuf;

use eframe::egui;
use turbo_harness::app::App;
use turbo_harness::store::Store;

fn main() -> eframe::Result {
    let dir = match working_dir(std::env::args_os().nth(1).map(PathBuf::from)) {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("turbo-harness: {message}");
            std::process::exit(2);
        }
    };
    let Some(data_dir) = Store::default_data_dir() else {
        eprintln!("turbo-harness: cannot find the app data directory");
        std::process::exit(2);
    };
    let store = Store::open(&data_dir, &dir);
    let claude = std::env::var("TURBO_HARNESS_CLAUDE").unwrap_or_else(|_| "claude".into());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("turbo-harness")
            .with_inner_size([1000.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "turbo-harness",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(dir, claude, store)))),
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
