use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::run::Run;
use crate::trajectory::{Node, Tool};

#[derive(Debug, Default)]
pub struct Follow {
    jump: bool,
}

pub fn show(ui: &mut egui::Ui, run: &Run, markdown: &mut CommonMarkCache, follow: &mut Follow) {
    let jump = std::mem::take(&mut follow.jump);
    let output = egui::ScrollArea::vertical()
        .auto_shrink(false)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            prompt_bubble(ui, &run.prompt);
            for (id, node) in run.trajectory.nodes.iter().enumerate() {
                ui.push_id(id, |ui| item(ui, node, markdown));
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
            follow.jump = true;
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

fn item(ui: &mut egui::Ui, node: &Node, markdown: &mut CommonMarkCache) {
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
        Node::Tool(tool) => tool_card(ui, tool),
        Node::System(line) => {
            ui.label(egui::RichText::new(line).small().weak());
        }
    }
    ui.add_space(4.0);
}

fn tool_card(ui: &mut egui::Ui, tool: &Tool) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.strong(&tool.name);
            ui.add(
                egui::Label::new(egui::RichText::new(&tool.summary).monospace().weak()).truncate(),
            );
        });
    });
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
