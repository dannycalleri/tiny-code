use std::collections::HashSet;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::run::{Run, RunStats};
use crate::trajectory::{Node, NodeId, Tool, ToolResult, ToolStatus};

pub(crate) const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 170, 90);

type ItemKey = (usize, NodeId);

#[derive(Debug, Default)]
pub struct View {
    jump: bool,
    show_all: HashSet<ItemKey>,
}

impl View {
    pub fn jump_to_latest(&mut self) {
        self.jump = true;
    }
}

pub fn show(ui: &mut egui::Ui, runs: &[Run], markdown: &mut CommonMarkCache, view: &mut View) {
    let jump = std::mem::take(&mut view.jump);
    let output = egui::ScrollArea::vertical()
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for (index, run) in runs.iter().enumerate() {
                if index > 0 {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                }
                prompt_bubble(ui, &run.prompt);
                for (id, node) in run.trajectory.nodes.iter().enumerate() {
                    let key = (index, id);
                    ui.push_id(key, |ui| item(ui, key, node, markdown, &mut view.show_all));
                }
                if let Some(error) = &run.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
            }
            if jump {
                ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
            }
        });

    let viewport = output.inner_rect.height();
    if !is_at_bottom(
        output.state.clamped_offset().y,
        viewport,
        output.content_size.y,
    ) {
        let rect = output.inner_rect;
        let at = egui::Rect::from_center_size(
            egui::pos2(rect.center().x, rect.bottom() - 24.0),
            egui::vec2(130.0, 28.0),
        );
        if ui.put(at, egui::Button::new("⬇ Jump to latest")).clicked() {
            view.jump = true;
            ui.ctx().request_repaint();
        }
    }
}

pub fn stats_line(stats: Option<&RunStats>) -> String {
    let stats = stats.copied().unwrap_or(RunStats {
        cost_usd: None,
        duration_ms: None,
        turns: None,
    });
    let cost = match stats.cost_usd {
        Some(cost) if cost > 0.0 && cost < 0.01 => "<$0.01".to_string(),
        Some(cost) => format!("${cost:.2}"),
        None => "—".to_string(),
    };
    let duration = stats.duration_ms.map_or("—".to_string(), duration);
    let turns = match stats.turns {
        Some(1) => "1 turn".to_string(),
        Some(n) => format!("{n} turns"),
        None => "— turns".to_string(),
    };
    format!("{cost} · {duration} · {turns}")
}

fn duration(ms: u64) -> String {
    let tenths = (ms + 50) / 100;
    if tenths < 600 {
        return format!("{}.{} s", tenths / 10, tenths % 10);
    }
    let seconds = (ms + 500) / 1000;
    match seconds {
        0..3600 => format!("{} m {:02} s", seconds / 60, seconds % 60),
        _ => format!("{} h {:02} m", seconds / 3600, seconds % 3600 / 60),
    }
}

fn is_at_bottom(offset: f32, viewport: f32, content: f32) -> bool {
    offset + viewport >= content - 2.0
}

fn prompt_bubble(ui: &mut egui::Ui, prompt: &str) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(8)
        .inner_margin(10)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(prompt).strong());
        });
    ui.add_space(6.0);
}

fn item(
    ui: &mut egui::Ui,
    key: ItemKey,
    node: &Node,
    markdown: &mut CommonMarkCache,
    show_all: &mut HashSet<ItemKey>,
) {
    match node {
        Node::Text(text) => {
            CommonMarkViewer::new().show_with_id(ui.scope_id(), ui, markdown, text);
        }
        Node::UserText(text) => {
            ui.label(egui::RichText::new(format!("↪ {text}")).weak());
        }
        Node::Thinking(text) => {
            egui::CollapsingHeader::new(egui::RichText::new("Thinking").weak().italics())
                .default_open(false)
                .show(ui, |ui| ui.label(egui::RichText::new(text).weak()));
        }
        Node::Tool(tool) => tool_card(ui, key, tool, show_all),
        Node::System(line) => {
            ui.label(egui::RichText::new(line).small().weak());
        }
    }
    ui.add_space(4.0);
}

fn tool_card(ui: &mut egui::Ui, key: ItemKey, tool: &Tool, show_all: &mut HashSet<ItemKey>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            status_icon(ui, tool.status);
            ui.strong(&tool.name);
            ui.add(
                egui::Label::new(egui::RichText::new(&tool.summary).monospace().weak()).truncate(),
            );
        });
        if let Some(result) = &tool.result {
            tool_result(ui, key, result, show_all);
        }
    });
}

fn tool_result(
    ui: &mut egui::Ui,
    key: ItemKey,
    result: &ToolResult,
    show_all: &mut HashSet<ItemKey>,
) {
    let kind = if result.is_error { "Error" } else { "Output" };
    let amount = match (&result.saved, result.line_count) {
        (Some(saved), _) => format!("{} · preview", saved.size),
        (None, 1) => "1 line".to_string(),
        (None, n) => format!("{n} lines"),
    };
    let open = egui::CollapsingHeader::new(format!("{kind} · {amount}"))
        .id_salt("result")
        .default_open(false)
        .show(ui, |ui| {
            let (text, button) = result.expanded(show_all.contains(&key));
            ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
            if let Some(count) = button
                && ui.button(format!("Show all {count} lines")).clicked()
            {
                show_all.insert(key);
            }
        });
    if let Some(saved) = &result.saved {
        let url = format!("file://{}", saved.path.display()).replace(' ', "%20");
        ui.hyperlink_to("Open full output", url);
    }
    if open.body_returned.is_none() && !result.preview.is_empty() {
        let color = if result.is_error {
            ui.visuals().error_fg_color
        } else {
            ui.visuals().weak_text_color()
        };
        ui.label(
            egui::RichText::new(&result.preview)
                .monospace()
                .color(color),
        );
    }
}

fn status_icon(ui: &mut egui::Ui, status: ToolStatus) {
    let visuals = ui.visuals();
    let (glyph, color) = match status {
        ToolStatus::Running => return quiet_spinner(ui),
        ToolStatus::Ok => ("✔", OK_GREEN),
        ToolStatus::Error => ("✖", visuals.error_fg_color),
        ToolStatus::Interrupted => ("⏸", visuals.weak_text_color()),
    };
    ui.label(egui::RichText::new(glyph).color(color));
}

// egui::Spinner requests a repaint every frame; this one relies on App's 100 ms repaint instead.
fn quiet_spinner(ui: &mut egui::Ui) {
    let size = ui.text_style_height(&egui::TextStyle::Body);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let radius = rect.height() * 0.5 - 2.0;
    let start = ui.input(|i| i.time) * std::f64::consts::TAU;
    let points: Vec<egui::Pos2> = (0..12)
        .map(|i| {
            let (sin, cos) = (start + f64::from(i) * 0.4).sin_cos();
            rect.center() + radius * egui::vec2(cos as f32, sin as f32)
        })
        .collect();
    let stroke = egui::Stroke::new(2.0, ui.visuals().strong_text_color());
    ui.painter().add(egui::Shape::line(points, stroke));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(cost: Option<f64>, ms: Option<u64>, turns: Option<u32>) -> RunStats {
        RunStats {
            cost_usd: cost,
            duration_ms: ms,
            turns,
        }
    }

    #[test]
    fn stats_line_formats_cost_duration_and_turns() {
        let line = stats_line(Some(&stats(Some(0.1938), Some(2771), Some(1))));
        assert_eq!(line, "$0.19 · 2.8 s · 1 turn");
        let line = stats_line(Some(&stats(Some(1.5), Some(65_000), Some(4))));
        assert_eq!(line, "$1.50 · 1 m 05 s · 4 turns");
        let line = stats_line(Some(&stats(Some(0.0), Some(59_960), Some(2))));
        assert_eq!(line, "$0.00 · 1 m 00 s · 2 turns");
        let line = stats_line(Some(&stats(Some(0.0), Some(3_725_000), Some(0))));
        assert_eq!(line, "$0.00 · 1 h 02 m · 0 turns");
    }

    #[test]
    fn a_cost_under_a_cent_does_not_look_free() {
        assert!(stats_line(Some(&stats(Some(0.004), None, None))).starts_with("<$0.01 "));
    }

    #[test]
    fn missing_values_show_a_dash() {
        assert_eq!(stats_line(None), "— · — · — turns");
        assert_eq!(
            stats_line(Some(&stats(Some(0.2), None, Some(2)))),
            "$0.20 · — · 2 turns"
        );
    }

    #[test]
    fn at_bottom_when_the_viewport_reaches_the_end() {
        assert!(is_at_bottom(600.0, 400.0, 1000.0));
        assert!(is_at_bottom(0.0, 400.0, 300.0));
    }

    #[test]
    fn not_at_bottom_after_scrolling_up() {
        assert!(!is_at_bottom(100.0, 400.0, 1000.0));
    }
}
