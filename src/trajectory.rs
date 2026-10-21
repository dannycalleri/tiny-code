use std::collections::HashMap;

use serde_json::Value;

use crate::event::{ContentBlock, Event};

pub type NodeId = usize;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(String),
    UserText(String),
    Thinking(String),
    Tool(Tool),
    System(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub input: Value,
    pub summary: String,
    pub result: Option<ToolResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Default)]
pub struct Trajectory {
    pub nodes: Vec<Node>,
    tools: HashMap<String, NodeId>,
}

impl Trajectory {
    pub fn push(&mut self, event: &Event) {
        match event {
            Event::System(s) if s.subtype == "init" => {
                let model = s.model.as_deref().unwrap_or("unknown model");
                self.nodes
                    .push(Node::System(format!("session started · {model}")));
            }
            Event::System(s) if s.subtype == "hook_started" => {}
            Event::System(s) => {
                let label = match s.hook_name.as_deref() {
                    Some(name) => format!("{} {name}", s.subtype),
                    None => s.subtype.clone(),
                };
                self.nodes.push(Node::System(label));
            }
            Event::Assistant(m) | Event::User(m) => {
                let from_user = matches!(event, Event::User(_));
                for block in &m.message.content {
                    self.push_block(block, from_user);
                }
            }
            Event::RateLimit => self.nodes.push(Node::System("rate limit update".into())),
            Event::Result(_) => {}
            Event::Unknown(v) => self.nodes.push(Node::System(format!(
                "unknown event {}",
                v["type"].as_str().unwrap_or("without a type")
            ))),
            Event::Unparsed(line) => self
                .nodes
                .push(Node::System(format!("unparsed line: {line}"))),
        }
    }

    fn push_block(&mut self, block: &ContentBlock, from_user: bool) {
        match block {
            ContentBlock::Text { text } if from_user => {
                self.nodes.push(Node::UserText(text.clone()))
            }
            ContentBlock::Text { text } => self.nodes.push(Node::Text(text.clone())),
            ContentBlock::Thinking { thinking } => {
                self.nodes.push(Node::Thinking(thinking.clone()))
            }
            ContentBlock::ToolUse { id, name, input } => {
                self.tools.insert(id.clone(), self.nodes.len());
                self.nodes.push(Node::Tool(Tool {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    summary: input_summary(name, input),
                    result: None,
                }));
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let result = ToolResult {
                    text: result_text(content),
                    is_error: *is_error,
                };
                match self.tools.get(tool_use_id).map(|&id| &mut self.nodes[id]) {
                    Some(Node::Tool(tool)) => tool.result = Some(result),
                    _ => self.nodes.push(Node::System(format!(
                        "result for unknown call {tool_use_id}"
                    ))),
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
                Node::Tool(tool) => Some(tool),
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
}
