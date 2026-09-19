//! Spot match engine process shell (config, restore RPC, MQ/Redis, bootstrap).

#![cfg_attr(any(coverage, coverage_nightly), feature(coverage_attribute))]

pub mod spot_depth;
pub mod bootstrap;
pub mod config;
pub mod error_queue;
pub mod health;
pub mod inbound;
pub mod mq;
pub mod outbound;
pub mod redis_store;
pub mod rpc;
pub mod symbol_worker;
pub mod telemetry;
