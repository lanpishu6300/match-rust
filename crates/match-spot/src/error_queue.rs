//! MQ send-failure queue (`match:poc_redis_send_mq_error_data_queue`).

use crate::redis_store::{mq_error_queue_key, RedisStore, RedisStoreError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ErrorQueueError {
    #[error("redis store: {0}")]
    Store(#[from] RedisStoreError),
}

pub struct ErrorQueue<'a> {
    store: &'a mut RedisStore,
    key: String,
}

impl<'a> ErrorQueue<'a> {
    pub fn new(store: &'a mut RedisStore) -> Self {
        Self {
            store,
            key: mq_error_queue_key(),
        }
    }

    pub fn push_raw(&mut self, payload: &[u8]) -> Result<i64, ErrorQueueError> {
        Ok(self.store.lpush_bytes(&self.key, payload)?)
    }
}
