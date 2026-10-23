//! GenerateSessionTurn: one turn of one thread's session (A29, backlog §1.3,
//! §1.4). The Messages API tool loop runs here; every tool_use goes down the
//! caller's own callback stream as an MCP `tools/call` frame and its answer
//! comes back as the tool_result.

use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;
use tonic::Status;

use llmproxy_model::types::services::llmproxy::llmproxy::{
    content_block::Block, ContentBlock, Message as ProtoMessage, Role as ProtoRole,
    SessionTurnRequest, SessionTurnResponse, StopReason, ToolResult as ProtoToolResult,
    ToolUse as ProtoToolUse, Usage as ProtoUsage,
};
use llmproxy_model::types::shared::crew::crew::BudgetBucket;

use crate::anthropic::{
    session_user_id, to_wire_messages, ReplyBlock, TurnReply, WireMessage, WireToolResult,
};
use crate::budget::{Ledger, UsageRow};
use crate::service::{to_status, LlmProxyService};

struct ToolCall<'a> {
    id: &'a str,
    name: &'a str,
    input: &'a serde_json::Value,
}

/// What a turn has produced so far.
struct Turn {
    appended: Vec<ProtoMessage>,
    usage: ProtoUsage,
}

impl Turn {
    fn finish(self, stop_reason: StopReason) -> SessionTurnResponse {
        SessionTurnResponse {
            appended: self.appended,
            stop_reason: stop_reason as i32,
            usage: Some(self.usage),
            ..Default::default()
        }
    }
}

impl LlmProxyService {
    pub(crate) async fn run_turn(
        &self,
        caller: String,
        req: SessionTurnRequest,
    ) -> Result<SessionTurnResponse, Status> {
        let cfg = self.config();
        let bucket = BudgetBucket::try_from(req.bucket)
            .ok()
            .filter(|b| *b != BudgetBucket::Unspecified)
            .ok_or_else(|| Status::invalid_argument("bucket is required"))?;
        // No callback stream, no spend: refused before any provider call.
        if !req.tools.is_empty() && self.callbacks.get(&caller).is_none() {
            return Err(Status::failed_precondition(
                "open BrokerToolCallback before requesting a turn with tools",
            ));
        }

        let script = self.script_for(&caller, &req.messages)?;
        let session_id = format!("{caller}#{}", req.thread_id);
        let user_id = session_user_id(&session_id);
        let max_rounds = match req.max_tool_rounds {
            0 => cfg.max_tool_rounds_ceiling,
            n => n.min(cfg.max_tool_rounds_ceiling),
        };
        let client = self.anthropic();
        tracing::info!(
            session_id = %session_id,
            parent_thread_id = %req.parent_thread_id,
            turn_id = %req.turn_id,
            "session turn"
        );

        let mut wire = to_wire_messages(&req.messages);
        let mut turn = Turn {
            appended: Vec::new(),
            usage: ProtoUsage {
                bucket: bucket as i32,
                ..Default::default()
            },
        };
        let mut rounds = 0;
        loop {
            let spent = self.ledger.lock().unwrap().spent(&caller, bucket);
            if spent >= Ledger::limit(&cfg, bucket) {
                return Ok(turn.finish(StopReason::BudgetExhausted));
            }

            let reply = match &script {
                Some(script) => script.turn_reply(&req.thread_id, &req.messages, &turn.appended)?,
                None => match client
                    .create_turn_message(
                        &req.model_alias,
                        &req.system,
                        req.max_tokens,
                        &wire,
                        &req.tools,
                        &user_id,
                    )
                    .await
                {
                    Ok(reply) => reply,
                    // Nothing has happened yet: the caller gets the real error.
                    Err(e) if turn.appended.is_empty() => return Err(to_status(e)),
                    // Tools have already run: hand back what the turn produced.
                    Err(e) => {
                        tracing::warn!(session_id = %session_id, error = %e, "provider call failed mid-turn");
                        return Ok(turn.finish(StopReason::ProviderError));
                    }
                },
            };
            self.charge(&caller, &session_id, bucket, &reply, &mut turn.usage);

            wire.push(WireMessage::assistant(&reply.blocks));
            turn.appended.push(assistant_message(&reply.blocks));

            let calls: Vec<ToolCall> = reply
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ReplyBlock::ToolUse { id, name, input } => Some(ToolCall { id, name, input }),
                    ReplyBlock::Text(_) => None,
                })
                .collect();
            if calls.is_empty() {
                return Ok(turn.finish(StopReason::EndTurn));
            }

            let (results, lost) = self.run_tools(&caller, &req.turn_id, &calls, &cfg).await;
            wire.push(WireMessage::tool_results(&results));
            turn.appended.push(tool_result_message(&results));
            if lost {
                return Ok(turn.finish(StopReason::CallbackLost));
            }
            rounds += 1;
            if rounds >= max_rounds {
                return Ok(turn.finish(StopReason::MaxToolRounds));
            }
        }
    }

    fn charge(
        &self,
        caller: &str,
        session_id: &str,
        bucket: BudgetBucket,
        reply: &TurnReply,
        usage: &mut ProtoUsage,
    ) {
        usage.input_tokens += reply.input_tokens;
        usage.output_tokens += reply.output_tokens;
        usage.cache_read_input_tokens += reply.cache_read_input_tokens;
        usage.cost_micro_usd += reply.cost_micro_usd;
        self.ledger.lock().unwrap().charge(
            caller,
            UsageRow {
                session_id: session_id.to_string(),
                bucket,
                input_tokens: reply.input_tokens,
                output_tokens: reply.output_tokens,
                cost_micro_usd: reply.cost_micro_usd,
            },
        );
    }

    /// Sends one `tools/call` frame per tool_use down the caller's stream and
    /// collects the answers. Returns the tool_results in tool_use order and
    /// whether the stream was lost.
    async fn run_tools(
        &self,
        caller: &str,
        turn_id: &str,
        calls: &[ToolCall<'_>],
        cfg: &llmproxy_model::types::services::llmproxy::llmproxy::Config,
    ) -> (Vec<WireToolResult>, bool) {
        let timeout = Duration::from_millis(cfg.tool_call_timeout_ms as u64);
        let grace = Duration::from_millis(cfg.callback_reopen_grace_ms as u64);
        let lost_result = |call: &ToolCall| WireToolResult {
            tool_use_id: call.id.to_string(),
            content: format!(
                "tool {} did not answer: the callback stream was lost; the call may or may not have taken effect; check before retrying",
                call.name
            ),
            is_error: true,
        };

        let Some(stream) = self.callbacks.wait_for(caller, grace).await else {
            return (calls.iter().map(lost_result).collect(), true);
        };

        let mut sent = Vec::with_capacity(calls.len());
        for call in calls {
            let frame_id = format!("llmproxy-{}", self.next_frame.fetch_add(1, Ordering::Relaxed));
            let frame = json!({
                "jsonrpc": "2.0",
                "id": frame_id,
                "method": "tools/call",
                "params": {
                    "name": call.name,
                    "arguments": call.input,
                    "_meta": {"turn": turn_id},
                },
            });
            let answer = stream.call(&frame_id, frame.to_string()).await.ok();
            sent.push((frame_id, answer, tokio::time::Instant::now() + timeout));
        }

        let mut lost = false;
        let mut results = Vec::with_capacity(calls.len());
        for (call, (frame_id, answer, deadline)) in calls.iter().zip(sent) {
            let result = match answer {
                None => None,
                Some(answer) => match tokio::time::timeout_at(deadline, answer).await {
                    Ok(Ok(payload)) => Some(tool_result(call.id, &payload)),
                    Ok(Err(_)) => None,
                    Err(_) => {
                        stream.forget(&frame_id);
                        Some(WireToolResult {
                            tool_use_id: call.id.to_string(),
                            content: format!(
                                "tool {} timed out after {}; the call may or may not have taken effect; check before retrying",
                                call.name,
                                human_duration(timeout)
                            ),
                            is_error: true,
                        })
                    }
                },
            };
            results.push(result.unwrap_or_else(|| {
                lost = true;
                lost_result(call)
            }));
        }
        (results, lost)
    }
}

/// The MCP `tools/call` answer as a tool_result: the result's text content,
/// `isError` carried over; a JSON-RPC error is an is_error result with its
/// message.
fn tool_result(tool_use_id: &str, payload: &str) -> WireToolResult {
    let (content, is_error) = match serde_json::from_str::<serde_json::Value>(payload) {
        Err(e) => (format!("the tool answered with an unparsable frame: {e}"), true),
        Ok(frame) => match (frame.get("error"), frame.get("result")) {
            (Some(error), _) => (
                error
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| error.to_string()),
                true,
            ),
            (None, Some(result)) => (
                result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .map(|b| match b.get("text").and_then(|t| t.as_str()) {
                                Some(text) => text.to_string(),
                                None => b.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                result.get("isError").and_then(|e| e.as_bool()).unwrap_or(false),
            ),
            (None, None) => ("the tool answered with neither result nor error".to_string(), true),
        },
    };
    WireToolResult {
        tool_use_id: tool_use_id.to_string(),
        content,
        is_error,
    }
}

fn human_duration(d: Duration) -> String {
    if d.subsec_millis() == 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{} ms", d.as_millis())
    }
}

fn assistant_message(blocks: &[ReplyBlock]) -> ProtoMessage {
    ProtoMessage {
        role: ProtoRole::Assistant as i32,
        content: blocks
            .iter()
            .map(|b| ContentBlock {
                block: Some(match b {
                    ReplyBlock::Text(text) => Block::Text(text.clone()),
                    ReplyBlock::ToolUse { id, name, input } => Block::ToolUse(ProtoToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input_json: input.to_string(),
                        ..Default::default()
                    }),
                }),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// The proto ToolResult has no is_error field: the flag reaches the model
/// within this turn (wire history) and the descriptive text carries it after.
fn tool_result_message(results: &[WireToolResult]) -> ProtoMessage {
    ProtoMessage {
        role: ProtoRole::User as i32,
        content: results
            .iter()
            .map(|r| ContentBlock {
                block: Some(Block::ToolResult(ProtoToolResult {
                    tool_use_id: r.tool_use_id.clone(),
                    content: r.content.clone(),
                    ..Default::default()
                })),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}
