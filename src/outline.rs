use eframe::egui;

use crate::run::Run;
use crate::trajectory::{Node, NodeId, ToolStatus, Trajectory};

pub const BREAKPOINT: f32 = 1100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Run(usize),
    Call(usize, NodeId),
}

#[derive(Debug, PartialEq)]
pub struct Entry<'a> {
    pub label: &'a str,
    pub status: Option<ToolStatus>,
    pub target: Target,
    pub children: Vec<Entry<'a>>,
}

pub fn outline(runs: &[Run]) -> Vec<Entry<'_>> {
    runs.iter()
        .enumerate()
        .map(|(index, run)| Entry {
            label: run.prompt.lines().next().unwrap_or_default(),
            status: None,
            target: Target::Run(index),
            children: calls(index, &run.trajectory, &run.trajectory.roots),
        })
        .collect()
}

fn calls<'a>(run: usize, trajectory: &'a Trajectory, ids: &[NodeId]) -> Vec<Entry<'a>> {
    ids.iter()
        .filter_map(|&id| match &trajectory.nodes[id] {
            Node::Tool(tool) => Some(Entry {
                label: &tool.name,
                status: Some(tool.status),
                target: Target::Call(run, id),
                children: calls(run, trajectory, &tool.children),
            }),
            _ => None,
        })
        .collect()
}

fn flatten<'e, 'a>(entries: &'e [Entry<'a>], depth: usize, out: &mut Vec<(usize, &'e Entry<'a>)>) {
    for entry in entries {
        out.push((depth, entry));
        flatten(&entry.children, depth + 1, out);
    }
}

pub fn show(ui: &mut egui::Ui, entries: &[Entry]) -> Option<Target> {
    let mut lines = Vec::new();
    flatten(entries, 0, &mut lines);
    let row_height = ui.spacing().interact_size.y;
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        ui,
        row_height,
        lines.len(),
        |ui, range| {
            for &(depth, entry) in &lines[range] {
                ui.horizontal(|ui| {
                    ui.add_space(depth as f32 * ui.spacing().indent);
                    if let Some(status) = entry.status {
                        crate::transcript::status_icon(ui, status);
                    }
                    let text = match entry.target {
                        Target::Run(_) => egui::RichText::new(entry.label).strong(),
                        Target::Call(..) => egui::RichText::new(entry.label),
                    };
                    if ui
                        .add(egui::Button::new(text).frame(false).truncate())
                        .clicked()
                    {
                        clicked = Some(entry.target);
                    }
                });
            }
        },
    );
    clicked
}

pub fn show_outline(width: f32, toggle: Option<bool>) -> bool {
    toggle.unwrap_or(width >= BREAKPOINT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse_line;

    fn run(prompt: &str, jsonl: &str) -> Run {
        let mut run = Run::new(prompt);
        for line in jsonl.lines() {
            run.trajectory.push(&parse_line(line));
        }
        run
    }

    fn shape(entries: &[Entry]) -> Vec<String> {
        let mut lines = Vec::new();
        flatten(entries, 0, &mut lines);
        lines
            .iter()
            .map(|(depth, entry)| format!("{}{}", "  ".repeat(*depth), entry.label))
            .collect()
    }

    #[test]
    fn the_outline_lists_run_prompts_and_calls_with_their_nesting() {
        let runs = [
            run(
                "Start a nested subagent\nwith details",
                include_str!("../tests/fixtures/subagents.jsonl"),
            ),
            run("Read notes", include_str!("../tests/fixtures/tools.jsonl")),
        ];
        assert_eq!(
            shape(&outline(&runs)),
            [
                "Start a nested subagent",
                "  Agent",
                "    Agent",
                "    Read",
                "    SubagentHandback",
                "Read notes",
                "  Read",
                "  Bash",
                "  Read",
            ]
        );
    }

    #[test]
    fn outline_entries_carry_the_call_status_and_target() {
        let runs = [run("x", include_str!("../tests/fixtures/tools.jsonl"))];
        let entries = outline(&runs);
        assert_eq!(
            (entries[0].status, entries[0].target),
            (None, Target::Run(0))
        );
        let missing = &entries[0].children[2];
        assert_eq!(missing.status, Some(ToolStatus::Error));
        let Target::Call(0, id) = missing.target else {
            panic!()
        };
        assert!(matches!(&runs[0].trajectory.nodes[id], Node::Tool(t) if t.name == "Read"));
    }

    #[test]
    fn the_outline_shows_on_a_wide_window_only() {
        assert!(!show_outline(1099.0, None));
        assert!(show_outline(BREAKPOINT, None));
    }

    #[test]
    fn a_user_toggle_wins_over_the_width() {
        assert!(!show_outline(1600.0, Some(false)));
        assert!(show_outline(600.0, Some(true)));
    }
}
