//! The A29 callback registry: one `BrokerToolCallback` stream per caller SPIFFE
//! identity, shared by every thread of that caller (backlog §1.1, §1.4).
//!
//! A re-open replaces the old stream (latest wins): the old outbound half is
//! closed and its pending frames fail. When a caller's inbound half ends, its
//! entry is removed (only if it is still the current one) and its pending
//! frames fail, which the turn loop reports as `callback_lost`.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, Notify};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::Status;

use llmproxy_model::types::sys::mcp::{McpRequest, McpResponse};

pub type InboundStream = Pin<Box<dyn Stream<Item = Result<McpResponse, Status>> + Send>>;
pub type OutboundStream = Pin<Box<dyn Stream<Item = Result<McpRequest, Status>> + Send>>;

/// Frames a caller has not read yet before llmproxy stops sending.
const OUTBOUND_BUFFER: usize = 64;

/// The stream is gone: closed, replaced, or the caller stopped reading.
#[derive(Debug)]
pub struct CallbackLost;

pub struct CallbackStream {
    generation: u64,
    out: Mutex<Option<mpsc::Sender<Result<McpRequest, Status>>>>,
    /// JSON-RPC id → the turn waiting for that frame's answer.
    pending: Mutex<HashMap<String, oneshot::Sender<String>>>,
}

impl CallbackStream {
    /// Sends one JSON-RPC frame and returns where its answer will arrive.
    pub async fn call(
        &self,
        frame_id: &str,
        payload: String,
    ) -> Result<oneshot::Receiver<String>, CallbackLost> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(frame_id.to_string(), tx);
        let out = self.out.lock().unwrap().clone();
        let sent = match out {
            Some(out) => out.send(Ok(McpRequest { payload })).await.is_ok(),
            None => false,
        };
        if !sent {
            self.pending.lock().unwrap().remove(frame_id);
            return Err(CallbackLost);
        }
        Ok(rx)
    }

    /// Forgets a frame (its timeout fired); a late answer is then dropped and
    /// logged by `route`.
    pub fn forget(&self, frame_id: &str) {
        self.pending.lock().unwrap().remove(frame_id);
    }

    fn route(&self, caller: &str, payload: String) {
        let id = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.get("id").and_then(|id| id.as_str().map(str::to_string)));
        let waiter = id.as_ref().and_then(|id| self.pending.lock().unwrap().remove(id));
        match waiter {
            Some(waiter) => {
                let _ = waiter.send(payload);
            }
            None => tracing::warn!(caller, frame_id = ?id, "dropped a callback answer with no waiting frame (late or unknown)"),
        }
    }

    /// Closes the outbound half and fails every pending frame.
    fn close(&self) {
        self.out.lock().unwrap().take();
        self.pending.lock().unwrap().clear();
    }
}

#[derive(Default)]
pub struct CallbackRegistry {
    streams: Mutex<HashMap<String, Arc<CallbackStream>>>,
    opened: Notify,
    generations: AtomicU64,
}

impl CallbackRegistry {
    /// Registers `caller`'s new stream (latest wins) and returns the outbound
    /// frames for the generated handler to hand back to the caller.
    pub fn open(self: &Arc<Self>, caller: String, mut inbound: InboundStream) -> OutboundStream {
        let (tx, rx) = mpsc::channel(OUTBOUND_BUFFER);
        let stream = Arc::new(CallbackStream {
            generation: self.generations.fetch_add(1, Ordering::Relaxed),
            out: Mutex::new(Some(tx)),
            pending: Mutex::new(HashMap::new()),
        });
        if let Some(old) = self.streams.lock().unwrap().insert(caller.clone(), stream.clone()) {
            tracing::info!(caller = %caller, "callback stream re-opened; closing the previous one");
            old.close();
        }
        self.opened.notify_waiters();

        let registry = self.clone();
        tokio::spawn(async move {
            while let Some(item) = inbound.next().await {
                match item {
                    Ok(resp) => stream.route(&caller, resp.payload),
                    Err(status) => {
                        tracing::warn!(caller = %caller, error = %status, "callback stream failed");
                        break;
                    }
                }
            }
            registry.remove_if_current(&caller, stream.generation);
            stream.close();
        });

        Box::pin(ReceiverStream::new(rx))
    }

    pub fn get(&self, caller: &str) -> Option<Arc<CallbackStream>> {
        self.streams.lock().unwrap().get(caller).cloned()
    }

    /// The caller's stream, waiting up to `grace` for one to be (re-)opened.
    pub async fn wait_for(&self, caller: &str, grace: Duration) -> Option<Arc<CallbackStream>> {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let opened = self.opened.notified();
            tokio::pin!(opened);
            opened.as_mut().enable();
            if let Some(stream) = self.get(caller) {
                return Some(stream);
            }
            if tokio::time::timeout_at(deadline, opened).await.is_err() {
                return None;
            }
        }
    }

    /// Kill switch (BQ7): closes every stream whose caller starts with one of
    /// `prefixes`.
    pub fn close_matching(&self, prefixes: &[String]) {
        let mut streams = self.streams.lock().unwrap();
        streams.retain(|caller, stream| {
            let blocked = prefixes.iter().any(|p| caller.starts_with(p.as_str()));
            if blocked {
                tracing::info!(caller = %caller, "closing a blocked caller's callback stream");
                stream.close();
            }
            !blocked
        });
    }

    fn remove_if_current(&self, caller: &str, generation: u64) {
        let mut streams = self.streams.lock().unwrap();
        if streams.get(caller).is_some_and(|s| s.generation == generation) {
            streams.remove(caller);
        }
    }
}
