//! Inbound consumer wiring over [`MessageSource`].

use std::sync::Arc;

use match_protocol::encode_symbol_key;

use super::topics::pull_order_topic;
use super::traits::{InboundHandler, MessageSource, SourceError, Subscription};
use crate::inbound::InboundRouter;

/// Build per-symbol pull subscriptions (topic + group).
///
/// Group = `{consumer_group}{encoded_symbol}` using the configured base group
/// (Java parity / cutover runbook: `usdt_contract_match_channel_one_group`).
pub fn subscriptions_for_symbols(symbols: &[String], consumer_group: &str) -> Vec<Subscription> {
    symbols
        .iter()
        .map(|symbol| Subscription {
            topic: pull_order_topic(symbol),
            consumer_group: format!("{consumer_group}{}", encode_symbol_key(symbol)),
        })
        .collect()
}

/// Start the message source with an inbound router handler (always-ACK semantics).
pub fn start_consumers(
    source: &dyn MessageSource,
    symbols: &[String],
    consumer_group: &str,
    router: Arc<InboundRouter>,
) -> Result<(), SourceError> {
    let subs = subscriptions_for_symbols(symbols, consumer_group);
    let handler: InboundHandler = Arc::new(move |_topic, body| {
        // ACK always — parity with Java `BaseConsumer.process` finally-return-true.
        let _ = router.handle_body(body);
    });
    source.start(&subs, handler)
}
