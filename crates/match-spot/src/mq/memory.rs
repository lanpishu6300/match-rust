//! In-memory / file-channel MQ adapter for local testing (no live RocketMQ).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::traits::{
    InboundHandler, MessageSource, OrderSink, SinkError, SourceError, Subscription,
};

/// Captures outbound sends for assertions and optional file persistence.
#[derive(Clone, Default)]
pub struct MemoryOrderSink {
    sent: Arc<Mutex<Vec<(String, Vec<u8>)>>>,
    out_dir: Option<PathBuf>,
    fail_topics: Arc<Mutex<Vec<String>>>,
}

impl MemoryOrderSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// When set, each send also writes `{topic}/{seq}.json` under `out_dir`.
    pub fn with_out_dir(path: impl Into<PathBuf>) -> Self {
        Self {
            sent: Arc::new(Mutex::new(Vec::new())),
            out_dir: Some(path.into()),
            fail_topics: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn sent(&self) -> Vec<(String, Vec<u8>)> {
        self.sent.lock().expect("memory sink lock").clone()
    }

    /// Force `send` to fail for topics containing this substring (tests error_queue path).
    pub fn fail_topic_containing(&self, needle: impl Into<String>) {
        self.fail_topics
            .lock()
            .expect("fail topics lock")
            .push(needle.into());
    }

    pub fn clear_fail_topics(&self) {
        self.fail_topics.lock().expect("fail topics lock").clear();
    }
}

impl OrderSink for MemoryOrderSink {
    fn send(&self, topic: &str, body: &[u8]) -> Result<(), SinkError> {
        {
            let fails = self.fail_topics.lock().expect("fail topics lock");
            if fails.iter().any(|n| topic.contains(n.as_str())) {
                return Err(SinkError::new(format!("forced failure for topic {topic}")));
            }
        }

        self.sent
            .lock()
            .expect("memory sink lock")
            .push((topic.to_string(), body.to_vec()));

        if let Some(dir) = &self.out_dir {
            let topic_dir = dir.join(sanitize_topic(topic));
            fs::create_dir_all(&topic_dir).map_err(|e| SinkError::new(e.to_string()))?;
            let seq = self.sent.lock().expect("memory sink lock").len();
            let path = topic_dir.join(format!("{seq:06}.json"));
            let mut f = fs::File::create(path).map_err(|e| SinkError::new(e.to_string()))?;
            f.write_all(body)
                .map_err(|e| SinkError::new(e.to_string()))?;
        }
        Ok(())
    }
}

fn sanitize_topic(topic: &str) -> String {
    topic.replace('/', "_")
}

/// Push-based in-memory inbound source. Inject messages via [`MemoryMessageSource::publish`].
#[derive(Clone)]
pub struct MemoryMessageSource {
    inner: Arc<MemoryMessageSourceInner>,
}

struct MemoryMessageSourceInner {
    inbox: Mutex<Vec<(String, Vec<u8>)>>,
    running: AtomicBool,
    handler: Mutex<Option<InboundHandler>>,
    subscriptions: Mutex<Vec<Subscription>>,
}

impl MemoryMessageSource {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(MemoryMessageSourceInner {
                inbox: Mutex::new(Vec::new()),
                running: AtomicBool::new(false),
                handler: Mutex::new(None),
                subscriptions: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Publish a raw body to `topic` (delivered after `start`).
    pub fn publish(&self, topic: impl Into<String>, body: Vec<u8>) {
        let topic = topic.into();
        let mut delivered = false;
        if let Some(handler) = self.inner.handler.lock().expect("handler lock").as_ref() {
            if self.inner.running.load(Ordering::SeqCst) {
                handler(&topic, &body);
                delivered = true;
            }
        }
        if !delivered {
            self.inner
                .inbox
                .lock()
                .expect("inbox lock")
                .push((topic, body));
        }
    }

    /// Bypass immediate delivery (exercises background poller in tests).
    pub fn enqueue_inbox(&self, topic: impl Into<String>, body: Vec<u8>) {
        self.inner
            .inbox
            .lock()
            .expect("inbox lock")
            .push((topic.into(), body));
    }

    /// Load JSON files from `dir/{topic}/*.json` once (file-channel helper).
    pub fn load_dir(&self, dir: &Path) -> Result<usize, SourceError> {
        let mut count = 0;
        if !dir.exists() {
            return Ok(0);
        }
        for entry in fs::read_dir(dir).map_err(|e| SourceError::new(e.to_string()))? {
            let entry = entry.map_err(|e| SourceError::new(e.to_string()))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let topic = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let body = fs::read(&path).map_err(|e| SourceError::new(e.to_string()))?;
            self.publish(topic, body);
            count += 1;
        }
        Ok(count)
    }
}

impl Default for MemoryMessageSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageSource for MemoryMessageSource {
    fn start(
        &self,
        subscriptions: &[Subscription],
        handler: InboundHandler,
    ) -> Result<(), SourceError> {
        *self.inner.subscriptions.lock().expect("subs lock") = subscriptions.to_vec();
        *self.inner.handler.lock().expect("handler lock") = Some(handler.clone());
        self.inner.running.store(true, Ordering::SeqCst);

        // Drain buffered messages.
        let pending: Vec<_> = self
            .inner
            .inbox
            .lock()
            .expect("inbox lock")
            .drain(..)
            .collect();
        for (topic, body) in pending {
            handler(&topic, &body);
        }

        // Background poller for late publishes that raced with start (no-op mostly).
        spawn_memory_poller(Arc::clone(&self.inner))
            .map_err(|e| SourceError::new(e.to_string()))?;

        Ok(())
    }

    fn stop(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
    }
}

fn spawn_memory_poller(
    inner: Arc<MemoryMessageSourceInner>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    thread::Builder::new()
        .name("memory-mq-poller".into())
        .spawn(move || memory_poller_loop(inner))
        .map(|_| ())
        .map_err(|e| e.into())
}

#[cfg_attr(coverage, coverage(off))]
fn memory_poller_loop(inner: Arc<MemoryMessageSourceInner>) {
    while inner.running.load(Ordering::SeqCst) {
        let batch: Vec<_> = inner
            .inbox
            .lock()
            .expect("inbox lock")
            .drain(..)
            .collect();
        if let Some(h) = inner.handler.lock().expect("handler lock").as_ref() {
            for (topic, body) in batch {
                h(&topic, &body);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;
    use crate::inbound::InboundRouter;
    use crate::mq::consumer::start_shard_consumers;
    use std::sync::Arc;

    #[test]
    fn sink_forced_failure() {
        let sink = MemoryOrderSink::new();
        sink.fail_topic_containing("bad");
        assert!(sink.send("topic/bad/x", b"x").is_err());
        assert!(sink.send("topic/ok/x", b"x").is_ok());
    }

    #[test]
    fn publish_after_stop_buffers_inbox() {
        let source = MemoryMessageSource::new();
        let router = Arc::new(InboundRouter::new());
        start_shard_consumers(&source, "g", false, router).unwrap();
        source.stop();
        source.publish("contract_match_order", b"[]".to_vec());
    }
}

/// Monotonic id helper for file naming in tests.
#[allow(dead_code)]
pub fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    SEQ.fetch_add(1, Ordering::Relaxed)
}
