use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Projects {
    pub entries: Vec<ProjectEntry>,
}

pub fn file(data_dir: &Path) -> PathBuf {
    data_dir.join("projects.json")
}

impl Projects {
    pub fn load(path: &Path) -> Projects {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, path)
    }

    pub fn add(&mut self, dir: &Path) -> usize {
        let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if let Some(index) = self.entries.iter().position(|e| e.path == dir) {
            self.entries[index].collapsed = false;
            return index;
        }
        self.entries.push(ProjectEntry {
            path: dir,
            collapsed: false,
        });
        self.entries.len() - 1
    }

    pub fn toggle(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.collapsed = !entry.collapsed;
        }
    }

    pub fn labels(&self) -> Vec<String> {
        let names: Vec<Vec<String>> = self.entries.iter().map(|e| folder_names(&e.path)).collect();
        names
            .iter()
            .enumerate()
            .map(|(i, own)| {
                let mut depth = 1;
                while depth < own.len()
                    && names
                        .iter()
                        .enumerate()
                        .any(|(j, other)| j != i && suffix(other, depth) == suffix(own, depth))
                {
                    depth += 1;
                }
                let label = suffix(own, depth);
                if label.is_empty() { "/".into() } else { label }
            })
            .collect()
    }
}

fn folder_names(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn suffix(names: &[String], depth: usize) -> String {
    names[names.len().saturating_sub(depth)..].join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "turbo-harness-projects-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root.join("projects.json")
    }

    fn projects(paths: &[&str]) -> Projects {
        let mut projects = Projects::default();
        for path in paths {
            projects.add(Path::new(path));
        }
        projects
    }

    #[test]
    fn add_appends_a_new_directory_and_returns_its_index() {
        let mut projects = projects(&["/work/a"]);
        assert_eq!(projects.add(Path::new("/work/b")), 1);
        let paths: Vec<&Path> = projects.entries.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/work/a"), Path::new("/work/b")]);
    }

    #[test]
    fn add_of_a_known_directory_returns_its_index_and_adds_nothing() {
        let mut projects = projects(&["/work/a", "/work/b"]);
        assert_eq!(projects.add(Path::new("/work/a")), 0);
        assert_eq!(projects.entries.len(), 2);
    }

    #[test]
    fn add_stores_the_canonical_path_so_two_spellings_are_one_project() {
        let dir = temp_file("canonical").parent().unwrap().join("app");
        fs::create_dir_all(&dir).unwrap();
        let mut projects = Projects::default();
        assert_eq!(projects.add(&dir.join("..").join("app").join(".")), 0);
        assert_eq!(projects.entries[0].path, dir.canonicalize().unwrap());
        assert_eq!(projects.add(&dir.canonicalize().unwrap()), 0);
        assert_eq!(projects.entries.len(), 1);
    }

    #[test]
    fn add_expands_a_collapsed_project() {
        let mut projects = projects(&["/work/a"]);
        projects.toggle(0);
        assert!(projects.entries[0].collapsed);
        projects.add(Path::new("/work/a"));
        assert!(!projects.entries[0].collapsed);
    }

    #[test]
    fn a_missing_or_damaged_file_gives_an_empty_list() {
        let path = temp_file("damaged");
        assert_eq!(Projects::load(&path), Projects::default());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{\"entries\": [").unwrap();
        assert_eq!(Projects::load(&path), Projects::default());
    }

    #[test]
    fn save_then_load_gives_the_same_list() {
        let path = temp_file("roundtrip");
        let mut projects = projects(&["/work/a", "/work/b"]);
        projects.toggle(1);
        projects.save(&path).unwrap();
        assert_eq!(Projects::load(&path), projects);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn labels_are_the_folder_names() {
        assert_eq!(projects(&["/work/a", "/work/b"]).labels(), ["a", "b"]);
    }

    #[test]
    fn two_folders_with_the_same_name_get_their_parents_in_the_label() {
        assert_eq!(
            projects(&["/x/app", "/y/app", "/z/other"]).labels(),
            ["x/app", "y/app", "other"]
        );
    }

    #[test]
    fn the_root_directory_is_labelled_with_a_slash() {
        assert_eq!(projects(&["/"]).labels(), ["/"]);
    }
}
