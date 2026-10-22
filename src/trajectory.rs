use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde_json::Value;

use crate::event::{ContentBlock, Event};

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(String),
    UserText(String),
    Thinking(String),
    Tool(Box<Tool>),
    System(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub summary: String,
    pub result: Option<ToolResult>,
    pub status: ToolStatus,
    pub children: Vec<NodeId>,
    pub steps: usize,
}

impl Tool {
    pub fn is_subagent(&self) -> bool {
        matches!(self.name.as_str(), "Agent" | "Task")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
    pub line_count: usize,
    pub preview: String,
    pub saved: Option<SavedOutput>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedOutput {
    pub size: String,
    pub path: PathBuf,
}

impl ToolResult {
    pub const PREVIEW_LINES: usize = 3;
    pub const EXPANDED_LINES: usize = 500;

    fn new(text: String, is_error: bool) -> Self {
        let (text, saved) = match read_saved_output(&text) {
            Some((saved, preview)) => (preview, Some(saved)),
            None => (text, None),
        };
        ToolResult {
            saved,
            line_count: text.lines().count(),
            preview: first_lines(&text, Self::PREVIEW_LINES).to_string(),
            text,
            is_error,
        }
    }

    pub fn expanded(&self, show_all: bool) -> (&str, Option<usize>) {
        if show_all || self.line_count <= Self::EXPANDED_LINES {
            (&self.text, None)
        } else {
            (
                first_lines(&self.text, Self::EXPANDED_LINES),
                Some(self.line_count),
            )
        }
    }
}

fn read_saved_output(text: &str) -> Option<(SavedOutput, String)> {
    let body = text
        .strip_prefix("<persisted-output>")?
        .trim_end()
        .strip_suffix("</persisted-output>")?
        .trim();
    let (header, rest) = body.split_once('\n').unwrap_or((body, ""));
    let size = header
        .strip_prefix("Output too large (")?
        .split_once(')')?
        .0;
    let path = header.split_once("saved to: ")?.1.trim();
    if size.is_empty() || path.is_empty() {
        return None;
    }
    let rest = rest.trim_start_matches('\n');
    let preview = match rest.split_once('\n') {
        Some((first, after)) if first.starts_with("Preview (") => after,
        _ => rest,
    };
    let preview = preview.trim_end();
    let preview = match preview.rsplit_once('\n') {
        Some((before, "...")) => before,
        None if preview == "..." => "",
        _ => preview,
    };
    Some((
        SavedOutput {
            size: size.to_string(),
            path: PathBuf::from(path),
        },
        preview.trim_end().to_string(),
    ))
}

pub fn first_lines(text: &str, n: usize) -> &str {
    if n == 0 {
        return "";
    }
    let end = text
        .match_indices('\n')
        .nth(n - 1)
        .map_or(text.len(), |(at, _)| at);
    let prefix = &text[..end];
    prefix.strip_suffix('\r').unwrap_or(prefix)
}

#[derive(Debug, Default)]
pub struct Trajectory {
    pub nodes: Vec<Node>,
    pub roots: Vec<NodeId>,
    parents: Vec<Option<NodeId>>,
    orphans: HashSet<NodeId>,
    tools: HashMap<String, NodeId>,
}

impl Trajectory {
    pub fn interrupt_open_tools(&mut self) {
        for node in &mut self.nodes {
            if let Node::Tool(tool) = node
                && tool.status == ToolStatus::Running
            {
                tool.status = ToolStatus::Interrupted;
            }
        }
    }

    pub fn is_orphan(&self, id: NodeId) -> bool {
        self.orphans.contains(&id)
    }

    pub fn push(&mut self, event: &Event) {
        match event {
            Event::System(s) if s.subtype == "init" => {
                let model = s.model.as_deref().unwrap_or("unknown model");
                self.add(Node::System(format!("session started · {model}")), None);
            }
            Event::System(s) if s.subtype == "hook_started" => {}
            Event::System(s) => {
                let label = match s.hook_name.as_deref() {
                    Some(name) => format!("{} {name}", s.subtype),
                    None => s.subtype.clone(),
                };
                self.add(Node::System(label), None);
            }
            Event::Assistant(m) | Event::User(m) => {
                let from_user = matches!(event, Event::User(_));
                let parent = m.parent_tool_use_id.as_deref();
                for block in &m.message.content {
                    self.push_block(block, from_user, parent);
                }
            }
            Event::RateLimit => {
                self.add(Node::System("rate limit update".into()), None);
            }
            Event::Result(_) => {}
            Event::Unknown(v) => {
                let kind = v["type"].as_str().unwrap_or("without a type");
                self.add(Node::System(format!("unknown event {kind}")), None);
            }
            Event::Unparsed(line) => {
                self.add(Node::System(format!("unparsed line: {line}")), None);
            }
        }
    }

    fn add(&mut self, node: Node, parent_tool_use_id: Option<&str>) -> NodeId {
        let id = self.nodes.len();
        let is_tool = matches!(node, Node::Tool(_));
        self.nodes.push(node);
        let parent = parent_tool_use_id.and_then(|p| self.tools.get(p).copied());
        self.parents.push(parent);
        let Some(parent) = parent else {
            if parent_tool_use_id.is_some() {
                self.orphans.insert(id);
            }
            self.roots.push(id);
            return id;
        };
        if let Node::Tool(tool) = &mut self.nodes[parent] {
            tool.children.push(id);
        }
        if is_tool {
            let mut ancestor = Some(parent);
            while let Some(at) = ancestor {
                if let Node::Tool(tool) = &mut self.nodes[at] {
                    tool.steps += 1;
                }
                ancestor = self.parents[at];
            }
        }
        id
    }

    fn push_block(&mut self, block: &ContentBlock, from_user: bool, parent: Option<&str>) {
        match block {
            ContentBlock::Text { text } if from_user => {
                self.add(Node::UserText(text.clone()), parent);
            }
            ContentBlock::Text { text } => {
                self.add(Node::Text(text.clone()), parent);
            }
            ContentBlock::Thinking { thinking } => {
                self.add(Node::Thinking(thinking.clone()), parent);
            }
            ContentBlock::ToolUse { id, name, input } => {
                let tool = Node::Tool(Box::new(Tool {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    summary: input_summary(name, input),
                    result: None,
                    status: ToolStatus::Running,
                    children: Vec::new(),
                    steps: 0,
                }));
                let node = self.add(tool, parent);
                self.tools.insert(id.clone(), node);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let result = ToolResult::new(result_text(content), *is_error);
                match self.tools.get(tool_use_id).map(|&id| &mut self.nodes[id]) {
                    Some(Node::Tool(tool)) => {
                        tool.status = if result.is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Ok
                        };
                        tool.result = Some(result);
                    }
                    _ => {
                        let line = format!("result for unknown call {tool_use_id}");
                        self.add(Node::System(line), parent);
                    }
                }
            }
            ContentBlock::Other => {}
        }
    }
}

fn input_summary(tool_name: &str, input: &Value) -> String {
    let field = match tool_name {
        "Bash" => Some("command"),
        "Read" | "Edit" | "Write" => Some("file_path"),
        "Grep" | "Glob" => Some("pattern"),
        "Agent" | "Task" => Some("description"),
        _ => None,
    };
    let known = field.and_then(|f| input[f].as_str());
    let text = known.or_else(|| input.as_object()?.values().find_map(Value::as_str));
    one_line(text.unwrap_or_default())
}

fn one_line(text: &str) -> String {
    const MAX: usize = 120;
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let cut = match first.char_indices().nth(MAX) {
        Some((at, _)) => &first[..at],
        None => first,
    };
    if cut.len() < text.trim().len() {
        format!("{cut}…")
    } else {
        cut.to_string()
    }
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::parse_line;

    fn trajectory(jsonl: &str) -> Trajectory {
        let mut t = Trajectory::default();
        for line in jsonl.lines() {
            t.push(&parse_line(line));
        }
        t
    }

    fn tools(t: &Trajectory) -> Vec<&Tool> {
        t.nodes
            .iter()
            .filter_map(|n| match n {
                Node::Tool(tool) => Some(&**tool),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn items_keep_the_order_of_arrival() {
        let t = trajectory(include_str!("../tests/fixtures/tools.jsonl"));
        let kinds: Vec<String> = t
            .nodes
            .iter()
            .map(|n| match n {
                Node::Tool(tool) => tool.name.clone(),
                Node::Text(_) => "text".into(),
                Node::System(s) => s.split(' ').next().unwrap().into(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "hook_response",
                "hook_response",
                "hook_response",
                "hook_response",
                "session",
                "Read",
                "rate",
                "Bash",
                "Read",
                "text"
            ]
        );
    }

    #[test]
    fn each_result_goes_to_the_call_with_the_same_id() {
        let t = trajectory(include_str!("../tests/fixtures/tools.jsonl"));
        let [read, bash, missing] = tools(&t)[..] else {
            panic!()
        };
        assert!(read.result.as_ref().unwrap().text.contains("alpha"));
        assert!(!bash.result.as_ref().unwrap().is_error);
        assert!(bash.result.as_ref().unwrap().text.contains("notes.txt"));
        assert!(missing.result.as_ref().unwrap().is_error);
    }

    #[test]
    fn results_out_of_order_still_go_to_their_own_call() {
        let t = trajectory(concat!(
            r#"{"type":"assistant","message":{"content":["#,
            r#"{"type":"tool_use","id":"a","name":"Read","input":{}},"#,
            r#"{"type":"tool_use","id":"b","name":"Grep","input":{}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":["#,
            r#"{"type":"tool_result","tool_use_id":"b","content":"from b"},"#,
            r#"{"type":"tool_result","tool_use_id":"a","content":"from a"}]}}"#,
        ));
        let [a, b] = tools(&t)[..] else { panic!() };
        assert_eq!(a.result.as_ref().unwrap().text, "from a");
        assert_eq!(b.result.as_ref().unwrap().text, "from b");
    }

    #[test]
    fn system_labels_have_no_stray_quotes_or_spaces() {
        let t = trajectory(concat!(
            r#"{"type":"system","subtype":"compact_boundary"}"#,
            "\n",
            r#"{"type":"stream_event"}"#,
        ));
        assert_eq!(
            t.nodes,
            [
                Node::System("compact_boundary".into()),
                Node::System("unknown event stream_event".into())
            ]
        );
    }

    #[test]
    fn tool_status_follows_its_result() {
        let t = trajectory(include_str!("../tests/fixtures/tools.jsonl"));
        let statuses: Vec<ToolStatus> = tools(&t).iter().map(|t| t.status).collect();
        assert_eq!(
            statuses,
            [ToolStatus::Ok, ToolStatus::Ok, ToolStatus::Error]
        );
    }

    #[test]
    fn a_call_without_a_result_is_running_until_interrupted() {
        let mut t = trajectory(concat!(
            r#"{"type":"assistant","message":{"content":["#,
            r#"{"type":"tool_use","id":"a","name":"Bash","input":{}},"#,
            r#"{"type":"tool_use","id":"b","name":"Read","input":{}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"ok"}]}}"#,
        ));
        assert_eq!(tools(&t)[0].status, ToolStatus::Running);
        t.interrupt_open_tools();
        let statuses: Vec<ToolStatus> = tools(&t).iter().map(|t| t.status).collect();
        assert_eq!(statuses, [ToolStatus::Interrupted, ToolStatus::Ok]);
    }

    #[test]
    fn a_call_without_a_result_has_none() {
        let t = trajectory(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}"#,
        );
        assert_eq!(tools(&t)[0].result, None);
    }

    #[test]
    fn a_result_with_an_unknown_id_becomes_an_orphan_system_line() {
        let t = trajectory(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"nope","content":"x"}]}}"#,
        );
        assert_eq!(
            t.nodes,
            [Node::System("result for unknown call nope".into())]
        );
    }

    fn summary(name: &str, input: Value) -> String {
        input_summary(name, &input)
    }

    #[test]
    fn bash_summary_is_the_command() {
        let input = serde_json::json!({"command": "ls -la", "description": "List files"});
        assert_eq!(summary("Bash", input), "ls -la");
    }

    #[test]
    fn file_tools_summary_is_the_path() {
        for name in ["Read", "Edit", "Write"] {
            let input =
                serde_json::json!({"file_path": "/home/user/demo/notes.txt", "content": "x"});
            assert_eq!(summary(name, input), "/home/user/demo/notes.txt", "{name}");
        }
    }

    #[test]
    fn search_tools_summary_is_the_pattern() {
        for name in ["Grep", "Glob"] {
            let input = serde_json::json!({"path": "src", "pattern": "fn main"});
            assert_eq!(summary(name, input), "fn main", "{name}");
        }
    }

    #[test]
    fn subagent_summary_is_the_description() {
        for name in ["Agent", "Task"] {
            let input = serde_json::json!({"context": "long brief", "description": "Find TODOs"});
            assert_eq!(summary(name, input), "Find TODOs", "{name}");
        }
    }

    #[test]
    fn a_known_tool_without_its_field_falls_back_to_the_first_string() {
        let input = serde_json::json!({"description": "List files"});
        assert_eq!(summary("Bash", input), "List files");
    }

    #[test]
    fn unknown_tool_summary_is_the_first_string_value_on_one_line() {
        let input = serde_json::json!({"count": 3, "query": "first line\nsecond line"});
        assert_eq!(summary("mcp__search", input), "first line…");
    }

    #[test]
    fn summary_is_empty_when_no_string_is_found() {
        assert_eq!(summary("Mystery", serde_json::json!({"n": 1})), "");
    }

    #[test]
    fn one_line_cuts_long_text_on_a_char_boundary() {
        let cut = one_line(&"é".repeat(200));
        assert_eq!(cut.chars().count(), 121);
        assert!(cut.ends_with('…'));
        assert_eq!(one_line("short"), "short");
        assert_eq!(one_line("\n  cargo test\n"), "cargo test");
    }

    #[test]
    fn first_lines_of_short_text_is_the_whole_text() {
        assert_eq!(first_lines("", 3), "");
        assert_eq!(first_lines("one", 3), "one");
        assert_eq!(first_lines("one\ntwo\nthree", 3), "one\ntwo\nthree");
    }

    #[test]
    fn first_lines_stops_before_the_break_after_line_n() {
        assert_eq!(first_lines("one\ntwo\nthree\nfour", 3), "one\ntwo\nthree");
        assert_eq!(first_lines("one\ntwo\n", 1), "one");
    }

    #[test]
    fn first_lines_drops_the_carriage_return_of_crlf() {
        assert_eq!(first_lines("one\r\ntwo\r\nthree", 2), "one\r\ntwo");
    }

    #[test]
    fn first_lines_with_zero_is_empty() {
        assert_eq!(first_lines("one\ntwo", 0), "");
    }

    fn numbered(lines: usize) -> String {
        (1..=lines).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn apply_counts_lines_and_keeps_a_preview() {
        for (content, count, preview) in [
            ("", 0, ""),
            ("one\ntwo", 2, "one\ntwo"),
            (numbered(10).as_str(), 10, "line 1\nline 2\nline 3"),
        ] {
            let mut t = trajectory(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}"#,
            );
            t.push(&Event::User(crate::event::MessageEvent {
                message: crate::event::Message {
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "t".into(),
                        content: Value::String(content.into()),
                        is_error: false,
                    }],
                },
                parent_tool_use_id: None,
                session_id: None,
            }));
            let result = tools(&t)[0].result.clone().unwrap();
            assert_eq!(
                (result.line_count, result.preview.as_str()),
                (count, preview),
                "{content:?}"
            );
        }
    }

    const SAVED: &str = "<persisted-output>\nOutput too large (55.9KB). Full output saved to: /home/user/.claude/tool-results/b1.txt\n\nPreview (first 2KB):\ntotal 172184\ndrwxr-xr-x  926 root  wheel  29632 .\n...\n</persisted-output>";

    #[test]
    fn a_saved_output_stub_gives_size_path_and_preview() {
        let result = ToolResult::new(SAVED.into(), false);
        assert_eq!(
            result.saved,
            Some(SavedOutput {
                size: "55.9KB".into(),
                path: PathBuf::from("/home/user/.claude/tool-results/b1.txt"),
            })
        );
        assert_eq!(
            result.text,
            "total 172184\ndrwxr-xr-x  926 root  wheel  29632 ."
        );
        assert_eq!(result.line_count, 2);
    }

    #[test]
    fn only_a_standalone_ellipsis_line_is_dropped_from_the_preview() {
        let stub = "<persisted-output>\nOutput too large (9KB). Full output saved to: /x.txt\n\nPreview (first 2KB):\nnpm WARN deprecated\nDownloading...\n</persisted-output>";
        let result = ToolResult::new(stub.into(), false);
        assert_eq!(result.text, "npm WARN deprecated\nDownloading...");
    }

    #[test]
    fn a_stub_without_a_size_or_a_path_stays_plain_text() {
        for text in [
            "<persisted-output>\nOutput too large. Full output saved to: /x.txt\n</persisted-output>",
            "<persisted-output>\nOutput too large (5KB). Somewhere else\n</persisted-output>",
            "<persisted-output>\nOutput too large (5KB). Full output saved to: /x.txt\n",
        ] {
            let result = ToolResult::new(text.into(), false);
            assert_eq!((result.saved, result.text.as_str()), (None, text));
        }
    }

    #[test]
    fn expanded_shows_everything_up_to_the_cap() {
        let result = ToolResult::new(numbered(500), false);
        assert_eq!(result.expanded(false), (result.text.as_str(), None));
    }

    #[test]
    fn expanded_cuts_at_the_cap_and_offers_show_all() {
        let result = ToolResult::new(numbered(501), false);
        let (text, show_all) = result.expanded(false);
        assert_eq!(text.lines().count(), 500);
        assert!(text.ends_with("line 500"));
        assert_eq!(show_all, Some(501));
    }

    #[test]
    fn expanded_with_show_all_has_everything_and_no_button() {
        let result = ToolResult::new(numbered(501), false);
        assert_eq!(result.expanded(true), (result.text.as_str(), None));
    }

    #[test]
    fn result_text_joins_text_blocks() {
        let content = serde_json::json!([{"type":"text","text":"a"},{"type":"image"},{"type":"text","text":"b"}]);
        assert_eq!(result_text(&content), "a\nb");
    }

    #[test]
    fn thinking_and_user_text_are_their_own_items() {
        let t = trajectory(concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":"sub-prompt"}}"#,
        ));
        assert_eq!(
            t.nodes,
            [
                Node::Thinking("hmm".into()),
                Node::UserText("sub-prompt".into())
            ]
        );
    }

    fn tool_named<'a>(t: &'a Trajectory, name: &str) -> (NodeId, &'a Tool) {
        t.nodes
            .iter()
            .enumerate()
            .find_map(|(id, n)| match n {
                Node::Tool(tool) if tool.name == name => Some((id, &**tool)),
                _ => None,
            })
            .unwrap()
    }

    fn names(t: &Trajectory, ids: &[NodeId]) -> Vec<String> {
        ids.iter()
            .map(|&id| match &t.nodes[id] {
                Node::Tool(tool) => tool.name.clone(),
                Node::UserText(_) => "prompt".into(),
                Node::Text(_) => "text".into(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    fn call(parent: Option<&str>, id: &str, name: &str) -> String {
        let parent = parent.map_or("null".to_string(), |p| format!("\"{p}\""));
        format!(
            r#"{{"type":"assistant","parent_tool_use_id":{parent},"message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}]}}}}"#
        )
    }

    #[test]
    fn subagent_steps_go_under_their_agent_call() {
        let t = trajectory(include_str!("../tests/fixtures/subagents.jsonl"));
        let (outer, agent) = tool_named(&t, "Agent");
        assert_eq!(
            names(&t, &agent.children),
            ["prompt", "Agent", "Read", "SubagentHandback"]
        );
        assert!(t.roots.contains(&outer));
        let (read, nested_read) = tool_named(&t, "Read");
        assert!(!t.roots.contains(&read));
        assert_eq!(nested_read.status, ToolStatus::Ok);
        assert!(
            nested_read
                .result
                .as_ref()
                .unwrap()
                .text
                .contains("fn main")
        );
    }

    #[test]
    fn a_subagent_inside_a_subagent_nests_at_depth_two() {
        let t = trajectory(
            &[
                call(None, "outer", "Agent"),
                call(Some("outer"), "inner", "Agent"),
                call(Some("inner"), "grep", "Grep"),
            ]
            .join("\n"),
        );
        assert_eq!(names(&t, &t.roots), ["Agent"]);
        let Node::Tool(outer) = &t.nodes[t.roots[0]] else {
            panic!()
        };
        let Node::Tool(inner) = &t.nodes[outer.children[0]] else {
            panic!()
        };
        assert_eq!(inner.id, "inner");
        assert_eq!(names(&t, &inner.children), ["Grep"]);
    }

    #[test]
    fn an_unknown_parent_gives_a_top_level_orphan() {
        let t = trajectory(&[call(None, "a", "Bash"), call(Some("gone"), "b", "Read")].join("\n"));
        assert_eq!(names(&t, &t.roots), ["Bash", "Read"]);
        assert!(!t.is_orphan(t.roots[0]));
        assert!(t.is_orphan(t.roots[1]));
    }

    #[test]
    fn steps_count_every_nested_tool_call_at_any_depth() {
        let t = trajectory(
            &[
                call(None, "outer", "Agent"),
                call(Some("outer"), "inner", "Agent"),
                call(Some("inner"), "grep", "Grep"),
                call(Some("inner"), "read", "Read"),
                call(Some("outer"), "bash", "Bash"),
            ]
            .join("\n"),
        );
        let steps = |id: &str| match &t.nodes[t.tools[id]] {
            Node::Tool(tool) => tool.steps,
            _ => panic!(),
        };
        assert_eq!((steps("outer"), steps("inner"), steps("grep")), (4, 2, 0));
        let captured = trajectory(include_str!("../tests/fixtures/subagents.jsonl"));
        assert_eq!(tool_named(&captured, "Agent").1.steps, 3);
    }
}
