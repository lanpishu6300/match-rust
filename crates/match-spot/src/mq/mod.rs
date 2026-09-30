//! Messaging layer.

pub mod consumer;
pub mod memory;
pub mod producer;
pub mod topics;
pub mod traits;

pub use consumer::start_shard_consumers;
pub use memory::{MemoryMessageSource, MemoryOrderSink};
pub use producer::Producer;
pub use traits::{MessageSource, OrderSink, SinkError, SourceError, Subscription};
