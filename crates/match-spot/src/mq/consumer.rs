use std::sync::Arc;

use super::topics::{pull_group, pull_mm_group, pull_order_mm_topic, pull_order_topic};
use super::traits::{InboundHandler, MessageSource, SourceError, Subscription};
use crate::inbound::InboundRouter;

pub fn shard_subscriptions(consumer_group: &str, enable_mm: bool) -> Vec<Subscription> {
    let mut subs = vec![Subscription {
        topic: pull_order_topic().to_string(),
        consumer_group: pull_group(consumer_group),
    }];
    if enable_mm {
        subs.push(Subscription {
            topic: pull_order_mm_topic(),
            consumer_group: pull_mm_group(consumer_group),
        });
    }
    subs
}

pub fn start_shard_consumers(
    source: &dyn MessageSource,
    consumer_group: &str,
    enable_mm: bool,
    router: Arc<InboundRouter>,
) -> Result<(), SourceError> {
    let subs = shard_subscriptions(consumer_group, enable_mm);
    let handler: InboundHandler = Arc::new(move |_topic, body| {
        let _ = router.handle_body(body);
    });
    source.start(&subs, handler)
}
