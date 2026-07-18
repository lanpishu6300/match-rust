//! Inbound consumer wiring over [`MessageSource`].

use std::sync::Arc;

use super::topics::{pull_order_group, pull_order_topic};
use super::traits::{InboundHandler, MessageSource, SourceError, Subscription};
use crate::inbound::InboundRouter;

/// Build per-symbol pull subscriptions (topic + group), matching Java `InitLoadData`.
pub fn subscriptions_for_symbols(symbols: &[String]) -> Vec<Subscription> {
    symbols
        .iter()
        .map(|symbol| Subscription {
            topic: pull_order_topic(symbol),
            consumer_group: pull_order_group(symbol),
        })
        .collect()
}

/// Start the message source with an inbound router handler (always-ACK semantics).
pub fn start_consumers(
    source: &dyn MessageSource,
    symbols: &[String],
    router: Arc<InboundRouter>,
) -> Result<(), SourceError> {
    let subs = subscriptions_for_symbols(symbols);
    let handler: InboundHandler = Arc::new(move |_topic, body| {
        // ACK always — parity with Java `BaseConsumer.process` finally-return-true.
        let _ = router.handle_body(body);
    });
    source.start(&subs, handler)
}
