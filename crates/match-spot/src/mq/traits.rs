use std::fmt;

pub trait OrderSink: Send + Sync {
    fn send(&self, topic: &str, body: &[u8]) -> Result<(), SinkError>;
}

pub trait MessageSource: Send + Sync {
    fn start(
        &self,
        subscriptions: &[Subscription],
        handler: InboundHandler,
    ) -> Result<(), SourceError>;

    fn stop(&self) {}
}

#[derive(Debug, Clone)]
pub struct Subscription {
    pub topic: String,
    pub consumer_group: String,
}

pub type InboundHandler = std::sync::Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

#[derive(Debug)]
pub struct SinkError {
    pub message: String,
}

impl SinkError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sink error: {}", self.message)
    }
}

impl std::error::Error for SinkError {}

#[derive(Debug)]
pub struct SourceError {
    pub message: String,
}

impl SourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source error: {}", self.message)
    }
}

impl std::error::Error for SourceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_and_source_error_display() {
        assert!(SinkError::new("x").to_string().contains("x"));
        assert!(SourceError::new("y").to_string().contains("y"));
    }

    struct NoopSource;
    impl MessageSource for NoopSource {
        fn start(
            &self,
            _subscriptions: &[Subscription],
            _handler: InboundHandler,
        ) -> Result<(), SourceError> {
            Ok(())
        }
    }

    #[test]
    fn message_source_default_stop() {
        NoopSource.stop();
    }
}
