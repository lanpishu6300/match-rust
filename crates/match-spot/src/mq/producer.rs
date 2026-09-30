use std::sync::Arc;

use super::topics::{
    push_deeps_topic, push_market_topic, push_no_deal_topic, push_order_topic, push_robot_topic,
};
use super::traits::{OrderSink, SinkError};

pub struct Producer {
    sink: Arc<dyn OrderSink>,
}

impl Producer {
    pub fn new(sink: Arc<dyn OrderSink>) -> Self {
        Self { sink }
    }

    pub fn send_push_order(
        &self,
        symbol_key: &str,
        mm_suffix: bool,
        body: &[u8],
    ) -> Result<(), SinkError> {
        self.sink
            .send(&push_order_topic(symbol_key, mm_suffix), body)
    }

    pub fn send_push_market(
        &self,
        symbol_key: &str,
        mm_suffix: bool,
        body: &[u8],
    ) -> Result<(), SinkError> {
        self.sink
            .send(&push_market_topic(symbol_key, mm_suffix), body)
    }

    pub fn send_no_deal(&self, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(push_no_deal_topic(), body)
    }

    pub fn send_deeps(&self, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(push_deeps_topic(), body)
    }

    pub fn send_robot(&self, body: &[u8]) -> Result<(), SinkError> {
        self.sink.send(push_robot_topic(), body)
    }
}
