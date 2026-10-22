use std::collections::{HashMap, HashSet};
use std::ops::Range;

use eframe::egui;
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use crate::run::{Run, RunStats};
use crate::trajectory::{Node, NodeId, Tool, ToolResult, ToolStatus, Trajectory};

pub(crate) const OK_GREEN: egui::Color32 = egui::Color32::from_rgb(80, 170, 90);

type ItemKey = (usize, NodeId);

#[derive(Debug, Default)]
pub struct View {
    jump: bool,
    show_all: HashSet<ItemKey>,
    heights: HashMap<Row, f32>,
    scroll_to: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Row {
    Separator(usize),
    Prompt(usize),
    Item(ItemKey),
    Error(usize),
}

fn rows(runs: &[Run]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        if index > 0 {
            rows.push(Row::Separator(index));
        }
        rows.push(Row::Prompt(index));
        rows.extend(
            run.trajectory
                .roots
                .iter()
                .map(|&id| Row::Item((index, id))),
        );
        if run.error.is_some() {
            rows.push(Row::Error(index));
        }
    }
    rows
}

impl View {
    pub fn jump_to_latest(&mut self) {
        self.jump = true;
    }
}

pub fn show(ui: &mut egui::Ui, runs: &[Run], markdown: &mut CommonMarkCache, view: &mut View) {
    let jump = std::mem::take(&mut view.jump);
    let rows = rows(runs);
    let mut area = egui::ScrollArea::vertical()
        .auto_shrink(false)
        .stick_to_bottom(true);
    if let Some(offset) = view.scroll_to.take() {
        area = area.vertical_scroll_offset(offset);
    }
    let output = area.show_viewport(ui, |ui, viewport| {
        let known: Vec<Option<f32>> = rows
            .iter()
            .map(|row| view.heights.get(row).copied())
            .collect();
        let heights: Vec<f32> = known
            .iter()
            .map(|h| h.unwrap_or(ESTIMATED_ROW_HEIGHT))
            .collect();
        let visible = visible_rows(&heights, viewport.min.y, viewport.max.y);
        ui.add_space(visible.before);
        let mut measured = Vec::with_capacity(visible.rows.len());
        for &row in &rows[visible.rows.clone()] {
            let top = ui.cursor().top();
            draw_row(ui, row, runs, markdown, &mut view.show_all);
            let height = ui.cursor().top() - top;
            view.heights.insert(row, height);
            measured.push(height);
        }
        ui.add_space(visible.after);
        if jump {
            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
        }
        anchor_shift(
            visible.before,
            &known[visible.rows],
            &measured,
            viewport.min.y,
        )
    });
    if output.inner != 0.0 && !jump {
        view.scroll_to = Some(output.state.clamped_offset().y + output.inner);
        ui.ctx().request_repaint();
    }

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

const ESTIMATED_ROW_HEIGHT: f32 = 40.0;

#[derive(Debug, PartialEq)]
struct Visible {
    rows: Range<usize>,
    before: f32,
    after: f32,
}

fn visible_rows(heights: &[f32], top: f32, bottom: f32) -> Visible {
    let total: f32 = heights.iter().sum();
    let mut start = 0;
    let mut y = 0.0;
    while start + 1 < heights.len() && y + heights[start] <= top {
        y += heights[start];
        start += 1;
    }
    let before = y;
    let mut end = start;
    while end < heights.len() && (end == start || y < bottom) {
        y += heights[end];
        end += 1;
    }
    Visible {
        rows: start..end,
        before,
        after: total - y,
    }
}

fn anchor_shift(before: f32, known: &[Option<f32>], measured: &[f32], viewport_top: f32) -> f32 {
    let mut top = before;
    let mut shift = 0.0;
    for (old, new) in known.iter().zip(measured) {
        if top >= viewport_top {
            break;
        }
        if old.is_none() {
            shift += new - ESTIMATED_ROW_HEIGHT;
        }
        top += new;
    }
    shift
}

fn draw_row(
    ui: &mut egui::Ui,
    row: Row,
    runs: &[Run],
    markdown: &mut CommonMarkCache,
    show_all: &mut HashSet<ItemKey>,
) {
    match row {
        Row::Separator(_) => {
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);
        }
        Row::Prompt(run) => prompt_bubble(ui, &runs[run].prompt),
        Row::Item((run, id)) => {
            let items = Items {
                run,
                trajectory: &runs[run].trajectory,
            };
            items.show(ui, id, markdown, show_all);
        }
        Row::Error(run) => {
            if let Some(error) = &runs[run].error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
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

#[derive(Clone, Copy)]
struct Items<'a> {
    run: usize,
    trajectory: &'a Trajectory,
}

impl Items<'_> {
    fn show(
        self,
        ui: &mut egui::Ui,
        id: NodeId,
        markdown: &mut CommonMarkCache,
        show_all: &mut HashSet<ItemKey>,
    ) {
        let key = (self.run, id);
        ui.push_id(key, |ui| {
            if self.trajectory.is_orphan(id) {
                ui.label(
                    egui::RichText::new("⚠ orphan: its parent call is unknown")
                        .small()
                        .weak(),
                );
            }
            self.item(ui, key, markdown, show_all);
        });
    }

    fn item(
        self,
        ui: &mut egui::Ui,
        key: ItemKey,
        markdown: &mut CommonMarkCache,
        show_all: &mut HashSet<ItemKey>,
    ) {
        match &self.trajectory.nodes[key.1] {
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
            Node::Tool(tool) if tool.is_subagent() || !tool.children.is_empty() => {
                self.subagent_card(ui, key, tool, markdown, show_all)
            }
            Node::Tool(tool) => tool_card(ui, key, tool, show_all),
            Node::System(line) => {
                ui.label(egui::RichText::new(line).small().weak());
            }
        }
        ui.add_space(4.0);
    }

    fn subagent_card(
        self,
        ui: &mut egui::Ui,
        key: ItemKey,
        tool: &Tool,
        markdown: &mut CommonMarkCache,
        show_all: &mut HashSet<ItemKey>,
    ) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::CollapsingHeader::new(subagent_header(ui, tool))
                .id_salt("subagent")
                .default_open(false)
                .show(ui, |ui| {
                    for &child in &tool.children {
                        self.show(ui, child, markdown, show_all);
                    }
                });
            if let Some(result) = &tool.result {
                tool_result(ui, key, result, show_all);
            }
        });
    }
}

fn subagent_header(ui: &egui::Ui, tool: &Tool) -> egui::text::LayoutJob {
    let visuals = ui.visuals();
    let (glyph, color) = match status_glyph(visuals, tool.status) {
        Some(glyph) => glyph,
        None => ("⏺", visuals.warn_fg_color),
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let format = |color| egui::TextFormat::simple(font.clone(), color);
    let mut job = egui::text::LayoutJob::default();
    job.append(glyph, 0.0, format(color));
    job.append(
        &format!(" {}", subagent_label(tool)),
        0.0,
        format(visuals.strong_text_color()),
    );
    job
}

fn subagent_label(tool: &Tool) -> String {
    let steps = match tool.steps {
        1 => "1 step".to_string(),
        n => format!("{n} steps"),
    };
    if tool.summary.is_empty() {
        format!("{} · {steps}", tool.name)
    } else {
        format!("{} · {} · {steps}", tool.name, tool.summary)
    }
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
    match status_glyph(ui.visuals(), status) {
        Some((glyph, color)) => {
            ui.label(egui::RichText::new(glyph).color(color));
        }
        None => quiet_spinner(ui),
    }
}

fn status_glyph(
    visuals: &egui::Visuals,
    status: ToolStatus,
) -> Option<(&'static str, egui::Color32)> {
    match status {
        ToolStatus::Running => None,
        ToolStatus::Ok => Some(("✔", OK_GREEN)),
        ToolStatus::Error => Some(("✖", visuals.error_fg_color)),
        ToolStatus::Interrupted => Some(("⏸", visuals.weak_text_color())),
    }
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

    fn agent(summary: &str, steps: usize) -> Tool {
        Tool {
            id: "a".into(),
            name: "Agent".into(),
            input: serde_json::Value::Null,
            summary: summary.into(),
            result: None,
            status: ToolStatus::Running,
            children: Vec::new(),
            steps,
        }
    }

    #[test]
    fn subagent_label_has_name_description_and_steps() {
        assert_eq!(
            subagent_label(&agent("Find TODOs", 3)),
            "Agent · Find TODOs · 3 steps"
        );
        assert_eq!(
            subagent_label(&agent("Find TODOs", 1)),
            "Agent · Find TODOs · 1 step"
        );
        assert_eq!(subagent_label(&agent("", 0)), "Agent · 0 steps");
    }

    fn visible(heights: &[f32], top: f32, bottom: f32) -> (Range<usize>, f32, f32) {
        let v = visible_rows(heights, top, bottom);
        (v.rows, v.before, v.after)
    }

    #[test]
    fn rows_above_and_below_the_viewport_are_replaced_by_space() {
        let heights = [10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(visible(&heights, 30.0, 70.0), (2..4, 30.0, 50.0));
    }

    #[test]
    fn a_row_cut_by_the_viewport_edge_is_drawn() {
        let heights = [10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(visible(&heights, 29.0, 61.0), (1..4, 10.0, 50.0));
    }

    #[test]
    fn the_top_of_the_list_starts_with_no_space() {
        assert_eq!(visible(&[10.0, 10.0, 10.0], 0.0, 15.0), (0..2, 0.0, 10.0));
    }

    #[test]
    fn an_offset_past_the_end_still_draws_the_last_row() {
        assert_eq!(visible(&[10.0, 10.0], 500.0, 600.0), (1..2, 10.0, 0.0));
    }

    #[test]
    fn no_rows_draw_nothing() {
        assert_eq!(visible(&[], 0.0, 100.0), (0..0, 0.0, 0.0));
    }

    #[test]
    fn a_row_cut_by_the_top_that_grows_shifts_the_scroll_by_its_growth() {
        assert_eq!(
            anchor_shift(100.0, &[None, None], &[200.0, 60.0], 120.0),
            160.0
        );
    }

    #[test]
    fn a_measured_row_that_grows_at_the_top_does_not_shift_the_scroll() {
        assert_eq!(
            anchor_shift(100.0, &[Some(80.0), None], &[200.0, 60.0], 120.0),
            0.0
        );
    }

    #[test]
    fn rows_starting_inside_the_viewport_do_not_shift_the_scroll() {
        assert_eq!(
            anchor_shift(100.0, &[None, None], &[200.0, 60.0], 100.0),
            0.0
        );
        assert_eq!(anchor_shift(0.0, &[], &[], 50.0), 0.0);
    }

    #[test]
    fn rows_put_each_run_between_a_separator_and_its_error() {
        let mut first = Run::failed_to_start("one", "no claude".into());
        first.trajectory.push(&crate::event::parse_line(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        ));
        let second = Run::new("two");
        assert_eq!(
            rows(&[first, second]),
            [
                Row::Prompt(0),
                Row::Item((0, 0)),
                Row::Error(0),
                Row::Separator(1),
                Row::Prompt(1)
            ]
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
