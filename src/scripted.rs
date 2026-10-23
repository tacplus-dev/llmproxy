//! The `harness-scripted` model stand-in (decisions/harness-scotty-scenarios-
//! 2026-10-02.md §1.3, HS1/HS8). For a caller in one of
//! `Config.scripted_model_tenants` whose newest user turn carries
//! `[[script:<name>]]`, the provider reply comes from `scripts/<name>.yaml`
//! instead of Anthropic. Everything around it stays real: tool_use still goes
//! down the caller's BrokerToolCallback stream.
//!
//! A script remembers nothing. Which turn comes next is the number of
//! assistant messages after the marker, and its templates read only the
//! turn's own input: `{{marker}}`, `{{thread_id}}` and
//! `{{last_tool_result.<path>}}`.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;
use tonic::Status;

use llmproxy_model::types::services::llmproxy::llmproxy::{
    content_block::Block, generate_completion_response::Result as CompletionResult,
    GenerateCompletionResponse, Message as ProtoMessage, Role as ProtoRole,
    ToolUse as ProtoToolUse, Usage as ProtoUsage,
};

use crate::anthropic::{ReplyBlock, TurnReply};

/// Where the Dockerfile puts `services/llmproxy/scripts/`.
pub const SCRIPTS_DIR: &str = "/usr/share/llmproxy/scripts";

const MARKER_OPEN: &str = "[[script:";
const MARKER_CLOSE: &str = "]]";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptFile {
    // `- text: ...` / `- tool_use: {...}` rather than YAML `!tags`.
    #[serde(with = "serde_yaml::with::singleton_map_recursive")]
    turns: Vec<ScriptTurn>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum ScriptTurn {
    ToolUse { name: String, input: Value },
    Text(String),
}

/// A script selected for one request.
pub struct Script {
    name: String,
    turns: Vec<ScriptTurn>,
    /// Index of the marker-bearing user message in the request's messages.
    at: usize,
    /// The marker message's text after `[[script:<name>]]`, trimmed.
    marker: String,
}

/// The newest user message that carries text, if its text holds a marker:
/// (its index, the script name, the text after the marker).
fn find_marker(messages: &[ProtoMessage]) -> Option<(usize, String, String)> {
    let (at, text) = messages.iter().enumerate().rev().find_map(|(i, m)| {
        if m.role != ProtoRole::User as i32 {
            return None;
        }
        let text: Vec<&str> = m
            .content
            .iter()
            .filter_map(|b| match &b.block {
                Some(Block::Text(t)) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        (!text.is_empty()).then(|| (i, text.join("\n")))
    })?;
    let start = text.find(MARKER_OPEN)? + MARKER_OPEN.len();
    let len = text[start..].find(MARKER_CLOSE)?;
    let name = text[start..start + len].to_string();
    let marker = text[start + len + MARKER_CLOSE.len()..].trim().to_string();
    Some((at, name, marker))
}

impl Script {
    /// The script the request's newest user turn names, loaded from `dir`.
    /// `None` when there is no marker; an unknown name is INVALID_ARGUMENT.
    pub fn select(dir: &Path, messages: &[ProtoMessage]) -> Result<Option<Script>, Status> {
        let Some((at, name, marker)) = find_marker(messages) else {
            return Ok(None);
        };
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Status::invalid_argument(format!(
                "unknown script: {name:?}"
            )));
        }
        let source = match std::fs::read_to_string(dir.join(format!("{name}.yaml"))) {
            Ok(source) => source,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Status::invalid_argument(format!(
                    "unknown script: {name:?}"
                )))
            }
            Err(e) => return Err(Status::internal(format!("script {name}: {e}"))),
        };
        let file: ScriptFile = serde_yaml::from_str(&source)
            .map_err(|e| Status::internal(format!("script {name} does not parse: {e}")))?;
        Ok(Some(Script {
            name,
            turns: file.turns,
            at,
            marker,
        }))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The next reply, given the request's messages and what this turn has
    /// appended since.
    pub fn reply(
        &self,
        thread_id: &str,
        request: &[ProtoMessage],
        appended: &[ProtoMessage],
    ) -> Result<ReplyBlock, Status> {
        let history = || request[self.at + 1..].iter().chain(appended);
        let k = history()
            .filter(|m| m.role == ProtoRole::Assistant as i32)
            .count();
        let turn = self.turns.get(k).ok_or_else(|| {
            Status::failed_precondition(format!(
                "script {} has {} turns and was asked for turn {}",
                self.name,
                self.turns.len(),
                k + 1
            ))
        })?;
        let last_tool_result = history()
            .flat_map(|m| &m.content)
            .filter_map(|b| match &b.block {
                Some(Block::ToolResult(r)) => Some(r.content.as_str()),
                _ => None,
            })
            .last();
        let vars = Vars {
            marker: &self.marker,
            thread_id,
            last_tool_result,
        };
        let err = |e: String| {
            Status::failed_precondition(format!("script {} turn {}: {e}", self.name, k + 1))
        };
        Ok(match turn {
            ScriptTurn::ToolUse { name, input } => ReplyBlock::ToolUse {
                id: format!("toolu_script_{k}"),
                name: name.clone(),
                input: vars.render(input).map_err(err)?,
            },
            ScriptTurn::Text(text) => ReplyBlock::Text(format!(
                "{} seen=[{}]",
                vars.render_str(text).map_err(err)?,
                seen(request).join(",")
            )),
        })
    }

    /// The reply as one session-turn provider reply. A script costs nothing.
    pub fn turn_reply(
        &self,
        thread_id: &str,
        request: &[ProtoMessage],
        appended: &[ProtoMessage],
    ) -> Result<TurnReply, Status> {
        Ok(TurnReply {
            blocks: vec![self.reply(thread_id, request, appended)?],
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cost_micro_usd: 0,
        })
    }

    /// The reply as a GenerateCompletionAsService response (no thread).
    pub fn completion(
        &self,
        request: &[ProtoMessage],
    ) -> Result<GenerateCompletionResponse, Status> {
        let result = match self.reply("", request, &[])? {
            ReplyBlock::Text(text) => CompletionResult::Text(text),
            ReplyBlock::ToolUse { id, name, input } => CompletionResult::ToolUse(ProtoToolUse {
                id,
                name,
                input_json: input.to_string(),
                ..Default::default()
            }),
        };
        Ok(GenerateCompletionResponse {
            result: Some(result),
            usage: Some(ProtoUsage::default()),
            ..Default::default()
        })
    }
}

/// The `m<k>` marker tokens anywhere in the request's messages, first
/// occurrence order, each once.
fn seen(messages: &[ProtoMessage]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for block in messages.iter().flat_map(|m| &m.content) {
        let text = match &block.block {
            Some(Block::Text(t)) => t.as_str(),
            Some(Block::ToolUse(u)) => u.input_json.as_str(),
            Some(Block::ToolResult(r)) => r.content.as_str(),
            None => continue,
        };
        for token in text.split(|c: char| !c.is_ascii_alphanumeric()) {
            let is_marker = token.len() > 1
                && token.starts_with('m')
                && token[1..].chars().all(|c| c.is_ascii_digit());
            if is_marker && !seen.iter().any(|s| s == token) {
                seen.push(token.to_string());
            }
        }
    }
    seen
}

struct Vars<'a> {
    marker: &'a str,
    thread_id: &'a str,
    last_tool_result: Option<&'a str>,
}

impl Vars<'_> {
    /// Renders every string in a tool_use input template.
    fn render(&self, template: &Value) -> Result<Value, String> {
        Ok(match template {
            Value::String(s) => Value::String(self.render_str(s)?),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.render(v))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), self.render(v)?)))
                    .collect::<Result<_, String>>()?,
            ),
            other => other.clone(),
        })
    }

    fn render_str(&self, template: &str) -> Result<String, String> {
        let mut out = String::new();
        let mut rest = template;
        while let Some(start) = rest.find("{{") {
            out.push_str(&rest[..start]);
            let len = rest[start..]
                .find("}}")
                .ok_or_else(|| format!("unclosed {{{{ in {template:?}"))?;
            out.push_str(&self.lookup(rest[start + 2..start + len].trim())?);
            rest = &rest[start + len + 2..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn lookup(&self, name: &str) -> Result<String, String> {
        match name {
            "marker" => Ok(self.marker.to_string()),
            "thread_id" => Ok(self.thread_id.to_string()),
            _ => {
                let path = name
                    .strip_prefix("last_tool_result.")
                    .ok_or_else(|| format!("unknown template value {{{{{name}}}}}"))?;
                let result = self.last_tool_result.ok_or("no tool_result yet")?;
                let mut value: Value = serde_json::from_str(result)
                    .map_err(|e| format!("the last tool_result is not JSON: {e}"))?;
                for segment in path.split('.') {
                    value = match value {
                        Value::Array(mut items) => segment
                            .parse::<usize>()
                            .ok()
                            .filter(|i| *i < items.len())
                            .map(|i| items.swap_remove(i)),
                        Value::Object(mut fields) => fields.remove(segment),
                        _ => None,
                    }
                    .ok_or_else(|| format!("the last tool_result has no {path}"))?;
                }
                Ok(match value {
                    Value::String(s) => s,
                    other => other.to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmproxy_model::types::services::llmproxy::llmproxy::{ContentBlock, ToolResult};

    fn message(role: ProtoRole, block: Block) -> ProtoMessage {
        ProtoMessage {
            role: role as i32,
            content: vec![ContentBlock {
                block: Some(block),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn user(text: &str) -> ProtoMessage {
        message(ProtoRole::User, Block::Text(text.to_string()))
    }

    fn script(yaml: &str, messages: &[ProtoMessage]) -> Script {
        let (at, name, marker) = find_marker(messages).expect("a marker");
        let file: ScriptFile = serde_yaml::from_str(yaml).unwrap();
        Script {
            name,
            turns: file.turns,
            at,
            marker,
        }
    }

    #[test]
    fn the_marker_is_read_from_the_newest_user_turn_only() {
        let older = [user("[[script:echo]] marker m1"), user("again, no marker")];
        assert!(find_marker(&older).is_none());
        let newest = [user("hi"), user("@tally [[script:post-into]] target=t-2")];
        assert_eq!(
            find_marker(&newest),
            Some((1, "post-into".to_string(), "target=t-2".to_string()))
        );
    }

    #[test]
    fn last_tool_result_paths_are_substituted_into_the_next_turn() {
        let request = [user("[[script:fetch]] for m3")];
        let s = script(
            r#"
turns:
  - tool_use: {name: request_pigeon, input: {kind: "bidportal.fetch_packet", note: "{{marker}} in {{thread_id}}"}}
  - tool_use: {name: post_room, input: {text: "run {{last_tool_result.run.id}} step {{last_tool_result.steps.1}}"}}
  - text: "done {{last_tool_result.ok}}"
"#,
            &request,
        );
        let first = s.reply("t-1", &request, &[]).unwrap();
        let ReplyBlock::ToolUse { name, input, .. } = first else {
            panic!("tool_use first")
        };
        assert_eq!(name, "request_pigeon");
        assert_eq!(
            input,
            serde_json::json!({"kind": "bidportal.fetch_packet", "note": "for m3 in t-1"})
        );

        let appended = [
            message(ProtoRole::Assistant, Block::Text("calling".to_string())),
            message(
                ProtoRole::User,
                Block::ToolResult(ToolResult {
                    tool_use_id: "toolu_script_0".to_string(),
                    content: r#"{"run": {"id": "pr-7"}, "steps": ["a", "b"], "ok": true}"#
                        .to_string(),
                    ..Default::default()
                }),
            ),
        ];
        let ReplyBlock::ToolUse { input, .. } = s.reply("t-1", &request, &appended).unwrap() else {
            panic!("tool_use second")
        };
        assert_eq!(input, serde_json::json!({"text": "run pr-7 step b"}));

        let missing = script(
            "turns:\n  - text: \"{{last_tool_result.nope}}\"\n",
            &request,
        );
        let err = missing
            .reply("t-1", &request, &appended[..0])
            .err()
            .expect("an error");
        assert!(
            err.message().contains("no tool_result yet"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn a_text_reply_ends_with_the_markers_seen_in_the_request() {
        let request = [
            user("[[script:echo]] marker m1"),
            message(
                ProtoRole::Assistant,
                Block::Text("echo t-1 seen=[m1]".to_string()),
            ),
            user("[[script:echo]] marker m2, not m 3 or am4 or m"),
        ];
        let s = script("turns:\n  - text: \"echo {{thread_id}}\"\n", &request);
        let ReplyBlock::Text(text) = s.reply("t-1", &request, &[]).unwrap() else {
            panic!("text")
        };
        assert_eq!(text, "echo t-1 seen=[m1,m2]");
    }

    #[test]
    fn a_script_past_its_last_turn_and_an_unknown_template_value_are_errors() {
        let request = [user("[[script:x]]")];
        let s = script("turns:\n  - text: \"{{who}}\"\n", &request);
        assert!(s
            .reply("t", &request, &[])
            .err()
            .expect("an error")
            .message()
            .contains("unknown template value {{who}}"));
        let done = [message(ProtoRole::Assistant, Block::Text("x".to_string()))];
        assert!(s
            .reply("t", &request, &done)
            .err()
            .expect("an error")
            .message()
            .contains("has 1 turns"));
    }

    #[test]
    fn every_shipped_script_parses() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let name = name
                .strip_suffix(".yaml")
                .expect("only .yaml files in scripts/");
            let request = [user(&format!("[[script:{name}]]"))];
            assert!(Script::select(&dir, &request).unwrap().is_some(), "{name}");
        }
    }
}
