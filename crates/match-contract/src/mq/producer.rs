//! Outbound producer wrapper over [`OrderSink`].

use std::sync::Arc;

use super::topics::{
    push_deeps_topic, push_market_topic, push_no_deal_topic, push_order_topic, PUSH_ROBOT,
};
use super::traits::{OrderSink, SinkError};

/// Thin facade that maps logical destinations to topic names.
pub struct Producer {
    sink: Arc<dyn OrderSink>,
}

impl Producer {
    pub fn new(sink: Arc<dyn OrderSink>) -> Self {
        Self { sink }
    }

    pub fn sink(&self) -> &Arc<dyn OrderSink> {
        &self.sink
    }

    pub fn send_push_order(&self, symbol_key: &str, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(&push_order_topic(symbol_key), body)
    }

    pub fn send_push_market(&self, symbol_key: &str, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(&push_market_topic(symbol_key), body)
    }

    pub fn send_no_deal(&self, symbol_key: &str, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(&push_no_deal_topic(symbol_key), body)
    }

    pub fn send_deeps(&self, symbol_key: &str, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(&push_deeps_topic(symbol_key), body)
    }

    pub fn send_robot(&self, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(PUSH_ROBOT, body)
    }
}
