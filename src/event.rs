use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    System(SystemEvent),
    Assistant(MessageEvent),
    User(MessageEvent),
    RateLimit,
    Result(ResultEvent),
    Unknown(Value),
    Unparsed(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemEvent {
    pub subtype: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub cwd: Option<String>,
    pub hook_name: Option<String>,
    pub outcome: Option<String>,
    pub task_id: Option<String>,
    pub tool_use_id: Option<String>,
    pub usage: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MessageEvent {
    pub message: Message,
    pub parent_tool_use_id: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Message {
    #[serde(deserialize_with = "string_or_blocks")]
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultEvent {
    pub subtype: String,
    #[serde(default)]
    pub is_error: bool,
    pub total_cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
    pub num_turns: Option<u32>,
    pub session_id: Option<String>,
    pub result: Option<String>,
}

fn string_or_blocks<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<ContentBlock>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Content {
        Text(String),
        Blocks(Vec<ContentBlock>),
    }
    Ok(match Content::deserialize(d)? {
        Content::Text(text) => vec![ContentBlock::Text { text }],
        Content::Blocks(blocks) => blocks,
    })
}

pub fn parse_line(line: &str) -> Event {
    let Ok(raw) = serde_json::from_str::<Value>(line) else {
        return Event::Unparsed(line.to_string());
    };
    let kind = raw["type"].as_str().unwrap_or("");

    match kind {
        "system" => typed(raw, Event::System),
        "assistant" => typed(raw, Event::Assistant),
        "user" => typed(raw, Event::User),
        "rate_limit_event" => Event::RateLimit,
        "result" => typed(raw, Event::Result),
        _ => Event::Unknown(raw),
    }
}

fn typed<T: DeserializeOwned>(raw: Value, wrap: fn(T) -> Event) -> Event {
    match T::deserialize(&raw) {
        Ok(event) => wrap(event),
        Err(_) => Event::Unknown(raw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = include_str!("../tests/fixtures/hello.jsonl");

    #[test]
    fn hello_fixture_parses_to_the_expected_variants() {
        let events: Vec<Event> = HELLO.lines().map(parse_line).collect();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::System(s) => s.subtype.as_str(),
                Event::Assistant(_) => "assistant",
                Event::User(_) => "user",
                Event::RateLimit => "rate_limit",
                Event::Result(_) => "result",
                Event::Unknown(_) => "unknown",
                Event::Unparsed(_) => "unparsed",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "hook_started",
                "hook_started",
                "hook_started",
                "hook_started",
                "hook_response",
                "hook_response",
                "hook_response",
                "hook_response",
                "init",
                "assistant",
                "rate_limit",
                "result",
            ]
        );
    }

    #[test]
    fn init_carries_the_session_id() {
        let init = HELLO.lines().map(parse_line).nth(8).unwrap();
        let Event::System(init) = init else {
            panic!("{init:?}")
        };
        assert_eq!(
            init.session_id.as_deref(),
            Some("9e26b157-1db2-4ff3-99fc-8737a92f55ba")
        );
    }

    #[test]
    fn result_carries_the_run_stats() {
        let Event::Result(r) = parse_line(HELLO.lines().last().unwrap()) else {
            panic!()
        };
        assert!(!r.is_error);
        assert_eq!(r.duration_ms, Some(2771));
        assert_eq!(r.num_turns, Some(1));
        assert!((r.total_cost_usd.unwrap() - 0.1938238).abs() < 1e-9);
    }

    #[test]
    fn assistant_text_block() {
        let Event::Assistant(m) = parse_line(HELLO.lines().nth(9).unwrap()) else {
            panic!()
        };
        assert_eq!(m.parent_tool_use_id, None);
        assert!(matches!(
            &m.message.content[..],
            [ContentBlock::Text { .. }]
        ));
    }

    #[test]
    fn tool_use_and_tool_result_blocks() {
        let use_line = r#"{"type":"assistant","parent_tool_use_id":"toolu_parent","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let Event::Assistant(m) = parse_line(use_line) else {
            panic!()
        };
        assert_eq!(m.parent_tool_use_id.as_deref(), Some("toolu_parent"));
        assert!(
            matches!(&m.message.content[0], ContentBlock::ToolUse { name, .. } if name == "Bash")
        );

        let result_line = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"boom","is_error":true}]}}"#;
        let Event::User(m) = parse_line(result_line) else {
            panic!()
        };
        assert!(matches!(
            &m.message.content[0],
            ContentBlock::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn user_content_may_be_a_plain_string() {
        let Event::User(m) = parse_line(r#"{"type":"user","message":{"content":"hi"}}"#) else {
            panic!()
        };
        assert_eq!(
            m.message.content,
            [ContentBlock::Text { text: "hi".into() }]
        );
    }

    #[test]
    fn unknown_block_type_is_kept_as_other() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"server_tool_use","id":"x"}]}}"#;
        let Event::Assistant(m) = parse_line(line) else {
            panic!()
        };
        assert_eq!(m.message.content, [ContentBlock::Other]);
    }

    #[test]
    fn unknown_type_is_kept_with_its_json() {
        let event = parse_line(r#"{"type":"stream_event","x":1}"#);
        let Event::Unknown(v) = event else {
            panic!("{event:?}")
        };
        assert_eq!(v["type"], "stream_event");
    }

    #[test]
    fn known_type_with_an_unexpected_shape_is_kept_as_unknown() {
        let event = parse_line(r#"{"type":"result","is_error":false}"#);
        let Event::Unknown(v) = event else {
            panic!("{event:?}")
        };
        assert_eq!(v["type"], "result");
    }

    #[test]
    fn non_json_line_is_unparsed() {
        assert_eq!(parse_line("not json"), Event::Unparsed("not json".into()));
    }
}
