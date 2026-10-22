use std::collections::HashSet;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::run::Run;
use crate::trajectory::{Node, NodeId, Tool, ToolResult, ToolStatus};

pub(crate) const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 170, 90);

#[derive(Debug, Default)]
pub struct View {
    jump: bool,
    show_all: HashSet<NodeId>,
}

pub fn show(ui: &mut egui::Ui, run: &Run, markdown: &mut CommonMarkCache, view: &mut View) {
    let jump = std::mem::take(&mut view.jump);
    let output = egui::ScrollArea::vertical()
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            prompt_bubble(ui, &run.prompt);
            for (id, node) in run.trajectory.nodes.iter().enumerate() {
                ui.push_id(id, |ui| item(ui, id, node, markdown, &mut view.show_all));
            }
            if let Some(error) = &run.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
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
    id: NodeId,
    node: &Node,
    markdown: &mut CommonMarkCache,
    show_all: &mut HashSet<NodeId>,
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
        Node::Tool(tool) => tool_card(ui, id, tool, show_all),
        Node::System(line) => {
            ui.label(egui::RichText::new(line).small().weak());
        }
    }
    ui.add_space(4.0);
}

fn tool_card(ui: &mut egui::Ui, id: NodeId, tool: &Tool, show_all: &mut HashSet<NodeId>) {
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
            tool_result(ui, id, result, show_all);
        }
    });
}

fn tool_result(ui: &mut egui::Ui, id: NodeId, result: &ToolResult, show_all: &mut HashSet<NodeId>) {
    let kind = if result.is_error { "Error" } else { "Output" };
    let lines = match result.line_count {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    };
    let open = egui::CollapsingHeader::new(format!("{kind} · {lines}"))
        .id_salt("result")
        .default_open(false)
        .show(ui, |ui| {
            let (text, button) = result.expanded(show_all.contains(&id));
            ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
            if let Some(count) = button
                && ui.button(format!("Show all {count} lines")).clicked()
            {
                show_all.insert(id);
            }
        });
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
