//! Branch coverage for match-spot process shell.

use bigdecimal::{BigDecimal, Zero};
use match_core::{Engine, MatchEvent, Side};
use match_protocol::{
    check_mq_order_spot, type_convert_spot, MqOrder, ORDER_FORM_MARKET_PRICE, ORDER_ROBOT,
    ORDER_STATUS_REVOKE, SPOT_ORDER_USER,
};
use match_spot::bootstrap::{self, BootstrapError};
use match_spot::config::{
    load_from_path, Config, HealthConfig, MatchConfig, MqTransport, RedisConfig, RocketMqConfig,
    RpcConfig, ShardConfig, TopicSplitConfig,
};
use match_spot::error_queue::ErrorQueue;
use match_spot::health::{spawn_server_ephemeral, BootstrapReady};
use match_spot::inbound::{InboundError, InboundRouter};
use match_spot::mq::consumer::{shard_subscriptions, start_shard_consumers};
use match_spot::mq::memory::{next_seq, MemoryMessageSource, MemoryOrderSink};
use match_spot::mq::producer::Producer;
use match_spot::mq::traits::{MessageSource, OrderSink, SourceError, Subscription};
use match_spot::outbound::Outbound;
use match_spot::redis_store::{depth_key, mq_error_queue_key, DepthSuffix, RedisStore};
use match_spot::rpc::market::SpotCoinMarket;
use match_spot::rpc::order::{build_mq_order_spot, EntrustListRow};
use match_spot::rpc::ResponseData;
use match_spot::spot_depth::build_depth_levels;
use match_spot::symbol_worker::test_set_engine_panic;
use match_spot::telemetry;
use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct FailMessageSource;

impl MessageSource for FailMessageSource {
    fn start(
        &self,
        _subscriptions: &[Subscription],
        _handler: match_spot::mq::traits::InboundHandler,
    ) -> Result<(), SourceError> {
        Err(SourceError::new("forced mq start failure"))
    }
}

fn sample_config(market_url: &str, order_url: &str) -> Config {
    Config {
        shard: ShardConfig { main_stream: 1 },
        startup_delay_ms: 0,
        depth_push_interval_ms: 1000,
        symbol_workers: 1,
        health: HealthConfig::default(),
        rocketmq: RocketMqConfig {
            name_server: "unused".into(),
            consumer_group: "contract_match_group".into(),
            transport: MqTransport::Memory,
            memory_dir: None,
            enable_mm_consumer: true,
        },
        redis: RedisConfig {
            cluster_nodes: vec!["127.0.0.1:6379".into()],
            password: String::new(),
        },
        rpc: RpcConfig {
            market_base_url: market_url.into(),
            order_base_url: order_url.into(),
        },
        r#match: MatchConfig {
            symbols_whitelist: vec![],
            queue_capacity: 8,
        },
        topic_split: TopicSplitConfig::default(),
    }
}

fn sample_mq(order_no: &str) -> MqOrder {
    MqOrder {
        user_id: Some(1),
        uid: Some(100),
        c_type: 1,
        deal_type: None,
        r#type: Some(SPOT_ORDER_USER),
        order_type: Some(1),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        trust_order_no: Some(order_no.into()),
        close_position: None,
        start_deposit: None,
        position_type: None,
        taker_rate: None,
        order_status: Some(0),
        order_form: Some(1),
        gear: None,
        lever_times: None,
        trust_number: Some("1".into()),
        trust_price: Some("50000".into()),
        create_time: Some(1_700_000_000),
        face_value: None,
        handicap_type: None,
    }
}

fn sample_bb(order_no: &str, user_type: i8) -> match_protocol::BbOrder {
    type_convert_spot(&{
        let mut mq = sample_mq(order_no);
        mq.r#type = Some(user_type);
        mq
    })
    .expect("convert")
}

#[test]
fn config_load_and_filter_branches() {
    let yaml = r#"
shard:
  main_stream: 1
startup_delay_ms: 0
depth_push_interval_ms: 1000
symbol_workers: 1
rocketmq:
  name_server: "ns"
  consumer_group: "g"
redis:
  cluster_nodes: ["127.0.0.1:6379"]
rpc:
  market_base_url: "http://m"
  order_base_url: "http://o"
match:
  symbols_whitelist: ["BTC/USDT"]
  queue_capacity: 0
"#;
    let dir = std::env::temp_dir().join(format!("match-spot-cfg-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cfg.yaml");
    fs::write(&path, yaml).unwrap();
    let cfg = load_from_path(&path).unwrap();
    assert_eq!(cfg.r#match.queue_capacity, 0);

    let markets = vec![
        SpotCoinMarket {
            coin_market: Some("BTC/USDT".into()),
            symbol_key: Some("btcusdt".into()),
            main_stream: Some(1),
        },
        SpotCoinMarket {
            coin_market: Some("ETH/USDT".into()),
            symbol_key: Some("ethusdt".into()),
            main_stream: Some(2),
        },
        SpotCoinMarket {
            coin_market: None,
            symbol_key: Some("x".into()),
            main_stream: Some(1),
        },
    ];
    let filtered = cfg.filter_markets(&markets);
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].symbol_key.as_deref(), Some("btcusdt"));

    let mut split = TopicSplitConfig::default();
    assert!(!split.uses_mm_suffix("BTC/USDT", SPOT_ORDER_USER, Some(ORDER_ROBOT)));
    assert!(!split.uses_mm_suffix("BTC/USDT", ORDER_ROBOT, Some(SPOT_ORDER_USER)));
    split.topic_enable = true;
    assert!(split.uses_mm_suffix("BTC/USDT", ORDER_ROBOT, Some(ORDER_ROBOT)));
    split.topic_enable = false;
    split.topic_coin_markets = vec!["BTC/USDT".into()];
    assert!(split.uses_mm_suffix("BTC/USDT", ORDER_ROBOT, None));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn config_load_io_error() {
    assert!(load_from_path("/nonexistent/match-spot-config.yaml").is_err());
}

#[tokio::test]
async fn inbound_router_branches() {
    let router = InboundRouter::new();
    let mut bad = sample_mq("x");
    bad.trust_number = None;
    router.handle_mq_order(&bad).unwrap();

    router.handle_body(b"not-json").unwrap_err();

    let (tx, mut rx) = mpsc::channel(1);
    router.register_queue("btcusdt", tx);
    router.handle_mq_order(&sample_mq("a")).unwrap();
    router.handle_mq_order(&sample_mq("b")).unwrap_err();

    let body = serde_json::to_vec(&[sample_mq("c")]).unwrap();
    router.handle_body(&body).unwrap();
    assert_eq!(rx.try_recv().unwrap().trust_order_no, "a");

    drop(rx);
    assert!(matches!(
        router.handle_mq_order(&sample_mq("closed")),
        Err(InboundError::Enqueue(_))
    ));

    let mut unknown = sample_mq("missing");
    unknown.symbol_key = Some("unknown".into());
    assert!(matches!(
        router.handle_mq_order(&unknown),
        Err(InboundError::MissingQueue(_))
    ));
}

#[test]
fn outbound_branches() {
    let sink = Arc::new(MemoryOrderSink::new());
    let mut split = TopicSplitConfig::default();
    split.topic_enable = true;
    let outbound_no_redis = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        60_000,
        TopicSplitConfig::default(),
    );
    let outbound = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        Some(RedisStore::mock()),
        60_000,
        split,
    );

    let taker = sample_bb("t1", SPOT_ORDER_USER);
    let fill = MatchEvent::Fill {
        symbol: "btcusdt".into(),
        taker_order_no: "t1".into(),
        maker_order_no: "m1".into(),
        taker_user_type: SPOT_ORDER_USER,
        maker_user_type: SPOT_ORDER_USER,
        price: "1".into(),
        qty: "1".into(),
        taker_remaining: "0".into(),
        maker_remaining: "0".into(),
        taker_status: 1,
        maker_status: 1,
    };
    let revoke = MatchEvent::Revoke {
        order_no: "t1".into(),
        symbol: "btcusdt".into(),
        remaining: "1".into(),
        reason: "user".into(),
    };

    let engine = Engine::new();
    outbound.handle_order_result("btcusdt", &taker, &[fill.clone()], &engine);

    let robot_fill = MatchEvent::Fill {
        symbol: "btcusdt".into(),
        taker_order_no: "t1".into(),
        maker_order_no: "m1".into(),
        taker_user_type: ORDER_ROBOT,
        maker_user_type: ORDER_ROBOT,
        price: "1".into(),
        qty: "1".into(),
        taker_remaining: "0".into(),
        maker_remaining: "0".into(),
        taker_status: 1,
        maker_status: 1,
    };
    let robot_taker = sample_bb("rb", ORDER_ROBOT);
    let order_push_before = sink
        .sent()
        .iter()
        .filter(|(t, _)| t.contains("order_push"))
        .count();
    outbound.handle_order_result("btcusdt", &robot_taker, &[robot_fill], &engine);
    let order_push_after = sink
        .sent()
        .iter()
        .filter(|(t, _)| t.contains("order_push"))
        .count();
    assert_eq!(
        order_push_before, order_push_after,
        "robot×robot fill skips order module push"
    );

    outbound.handle_order_result("btcusdt", &taker, &[fill], &engine);

    outbound.handle_order_result("btcusdt", &taker, &[revoke], &engine);
    assert!(sink
        .sent()
        .iter()
        .any(|(t, _)| t.contains("order_push")));

    let mut market_taker = sample_bb("mkt", SPOT_ORDER_USER);
    market_taker.order_form = ORDER_FORM_MARKET_PRICE;
    outbound.handle_order_result("btcusdt", &market_taker, &[], &engine);

    let mut revoked = sample_bb("rvk", SPOT_ORDER_USER);
    revoked.order_status = ORDER_STATUS_REVOKE;
    outbound.handle_order_result("btcusdt", &revoked, &[], &engine);

    sink.fail_topic_containing("no_deal");
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);
    sink.fail_topic_containing("deeps");
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);

    sink.clear_fail_topics();
    sink.fail_topic_containing("contract_match_order_push_order");
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);

    sink.clear_fail_topics();
    sink.fail_topic_containing("market_push_order");
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);

    sink.clear_fail_topics();
    sink.fail_topic_containing("market_push_order");
    outbound.handle_order_result(
        "btcusdt",
        &taker,
        &[MatchEvent::Fill {
            symbol: "btcusdt".into(),
            taker_order_no: "t1".into(),
            maker_order_no: "m1".into(),
            taker_user_type: SPOT_ORDER_USER,
            maker_user_type: SPOT_ORDER_USER,
            price: "1".into(),
            qty: "1".into(),
            taker_remaining: "0".into(),
            maker_remaining: "0".into(),
            taker_status: 1,
            maker_status: 1,
        }],
        &engine,
    );

    sink.clear_fail_topics();
    sink.fail_topic_containing("market_push_order");
    outbound_no_redis.handle_order_result("btcusdt", &taker, &[], &engine);

    let mut store = RedisStore::mock();
    {
        let mut q = ErrorQueue::new(&mut store);
        assert!(q.push_raw(b"failed").unwrap() >= 1);
    }
    store.test_set_fail_lpush(true);
    {
        let mut q = ErrorQueue::new(&mut store);
        assert!(q.push_raw(b"failed2").is_err());
    }
    assert!(!mq_error_queue_key().is_empty());
}

#[test]
fn outbound_depth_throttle_and_interval_pass() {
    let sink = Arc::new(MemoryOrderSink::new());
    let outbound = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        60_000,
        TopicSplitConfig::default(),
    );
    let outbound_fast = Outbound::new(
        Producer::new(Arc::clone(&sink) as Arc<dyn OrderSink>),
        None,
        50,
        TopicSplitConfig::default(),
    );
    let mut taker = sample_bb("th", SPOT_ORDER_USER);
    taker.order_status = ORDER_STATUS_REVOKE;
    let engine = Engine::new();

    let depth_msgs = || {
        sink.sent()
            .iter()
            .filter(|(t, _)| t.contains("no_deal"))
            .count()
    };
    let before = depth_msgs();
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);
    outbound.handle_order_result("btcusdt", &taker, &[], &engine);
    assert_eq!(
        depth_msgs(),
        before + 1,
        "second depth push within interval should be throttled"
    );

    outbound_fast.handle_order_result("ethusdt", &taker, &[], &engine);
    std::thread::sleep(Duration::from_millis(65));
    outbound_fast.handle_order_result("ethusdt", &taker, &[], &Engine::new());
    assert_eq!(
        depth_msgs(),
        before + 3,
        "depth push should fire again after interval"
    );
}

#[test]
fn inbound_convert_failure() {
    let router = InboundRouter::default();
    let (tx, _rx) = mpsc::channel(4);
    router.register_queue("btcusdt", tx);
    let mut mq = sample_mq("bad-decimal");
    mq.trust_number = Some("1.2.3".into());
    router.handle_mq_order(&mq).unwrap();
}

#[test]
fn spot_depth_skips_zero_remaining() {
    let mut o = match_core::BbOrder::test_limit(Side::Buy, BigDecimal::from(10), "1", 1, "1").0;
    o.remaining_number = BigDecimal::zero();
    let (levels, sum) = build_depth_levels(vec![o], 25);
    assert!(levels.is_empty());
    assert_eq!(sum, BigDecimal::zero());
}

#[test]
fn memory_mq_branches() {
    let dir = std::env::temp_dir().join(format!("match-spot-mq-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out");
    let sink = MemoryOrderSink::with_out_dir(&out);
    sink.fail_topic_containing("fail");
    assert!(sink
        .send("topic/fail/here", b"{}")
        .is_err());
    sink.send("topic/ok/here", b"{}").unwrap();
    assert_eq!(sink.sent().len(), 1);

    let source = MemoryMessageSource::new();
    source.publish("contract_match_order", b"[]".to_vec());
    let router = Arc::new(InboundRouter::new());
    start_shard_consumers(
        &source,
        "contract_match_group",
        false,
        Arc::clone(&router),
    )
    .unwrap();
    source.publish("contract_match_order", b"[]".to_vec());
    source.stop();

    let subs = shard_subscriptions("g", true);
    assert_eq!(subs.len(), 2);
    assert_eq!(shard_subscriptions("g", false).len(), 1);

    let in_dir = dir.join("in");
    fs::create_dir_all(&in_dir).unwrap();
    fs::write(in_dir.join("contract_match_order.json"), b"[]").unwrap();
    fs::write(in_dir.join("readme.txt"), b"x").unwrap();
    let source2 = MemoryMessageSource::new();
    assert_eq!(source2.load_dir(&in_dir).unwrap(), 1);
    assert_eq!(
        source2
            .load_dir(std::path::Path::new("/no/such/dir"))
            .unwrap(),
        0
    );
    assert!(next_seq() > 0);

    let _default_sink = MemoryOrderSink::default();
    let _default_source = MemoryMessageSource::default();

    let producer = Producer::new(Arc::new(sink) as Arc<dyn OrderSink>);
    producer.send_robot(b"{}").unwrap();

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn memory_mq_publish_delivery_modes() {
    let source = MemoryMessageSource::new();
    source.publish("contract_match_order", b"[]".to_vec());

    let router = Arc::new(InboundRouter::new());
    start_shard_consumers(&source, "contract_match_group", false, router).unwrap();
    source.publish("contract_match_order", b"[1]".to_vec());

    source.stop();
    source.publish("contract_match_order", b"[2]".to_vec());

    let sink = MemoryOrderSink::new();
    sink.send("topic/ok", b"{}").unwrap();
}

#[test]
fn rpc_build_entrust_row() {
    let row = EntrustListRow {
        user_id: Some(1),
        uid: Some(2),
        user_type: Some(1),
        r#type: Some(1),
        market_id: Some(1),
        coin_id: Some(2),
        symbol_key: Some("btcusdt".into()),
        coin_market: Some("BTC/USDT".into()),
        entrust_no: Some("E1".into()),
        price_type: Some(1),
        gear: Some(0),
        status: Some(0),
        remain_amount: Some("1".into()),
        price: Some("1".into()),
        create_time: Some(1),
    };
    let mq = build_mq_order_spot(&row);
    assert!(check_mq_order_spot(&mq));
}

#[test]
fn telemetry_counters_render() {
    telemetry::record_order_event();
    telemetry::record_inbound_invalid();
    telemetry::record_order_placed();
    telemetry::record_order_cancelled();
    telemetry::record_fill();
    let body = telemetry::render_prometheus();
    assert!(body.contains("match.order.events.total"));
    assert!(body.contains("match.trades.deals.total"));
}

#[tokio::test]
async fn health_server_endpoints() {
    let ready = BootstrapReady::new();
    let (port, _handle) = spawn_server_ephemeral(ready.shared()).await.unwrap();
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        client
            .get(format!("{base}/healthz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{base}/readyz"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    ready.mark_ready();
    assert_eq!(
        client
            .get(format!("{base}/readyz"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let metrics = client
        .get(format!("{base}/metrics"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(metrics.contains("match.order.events.total"));
}

async fn shutdown_running(running: bootstrap::Running, source: &dyn MessageSource) {
    source.stop();
    running.router.shutdown_queues();
    for handle in running.workers {
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("worker shutdown timeout")
            .expect("worker join");
    }
}

async fn mount_rpc_stubs(server: &MockServer, entrust_rows: serde_json::Value) {
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [
                {"symbolKey":"btcusdt","coinMarket":"BTC/USDT","mainStream":1},
                {"coinMarket":"ETH/USDT","mainStream":1}
            ]
        })))
        .mount(server)
        .await;

    Mock::given(method("POST"))
        .and(path("/order/entrust-list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": { "rows": [] }
        })))
        .mount(server)
        .await;

    if entrust_rows.as_array().is_some_and(|rows| !rows.is_empty()) {
        Mock::given(method("POST"))
            .and(path("/order/entrust-list"))
            .and(body_string_contains("\"type\":1"))
            .and(body_string_contains("\"page\":1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 1,
                "data": { "rows": entrust_rows }
            })))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn bootstrap_skips_markets_without_symbol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"mainStream": 1}]
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let err = bootstrap::run_with_redis(cfg, sink, source, RedisStore::mock())
        .await
        .err()
        .expect("expected error");
    assert!(matches!(err, BootstrapError::NoMarkets));
}

#[tokio::test]
async fn bootstrap_run_with_redis_and_rpc() {
    let server = MockServer::start().await;
    mount_rpc_stubs(&server, serde_json::json!([])).await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());

    let running = bootstrap::run_with_redis(
        cfg,
        Arc::clone(&sink),
        Arc::clone(&source),
        RedisStore::mock(),
    )
    .await
    .expect("bootstrap");

    assert_eq!(running.symbols, vec!["btcusdt", "ethusdt"]);
    assert!(depth_key("btcusdt", DepthSuffix::Trade).contains("_trade"));
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_no_markets_after_filter() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"symbolKey":"btcusdt","mainStream":99}]
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let err = bootstrap::run_with_redis(cfg, sink, source, RedisStore::mock())
        .await
        .err()
        .expect("expected error");
    assert!(matches!(err, BootstrapError::NoMarkets));
}

#[tokio::test]
async fn bootstrap_restore_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"symbolKey":"btcusdt","coinMarket":"BTC/USDT","mainStream":1}]
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/order/entrust-list"))
        .and(body_string_contains("\"type\":1"))
        .and(body_string_contains("\"page\":1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": { "rows": [{
                "userId": 1,
                "uid": 1,
                "userType": 1,
                "type": 1,
                "marketId": 1,
                "coinId": 2,
                "symbolKey": "unknown",
                "coinMarket": "UNK/USDT",
                "entrustNo": "E1",
                "priceType": 1,
                "gear": 0,
                "status": 0,
                "remainAmount": "1",
                "price": "1",
                "createTime": 1700000000
            }] }
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/order/entrust-list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": { "rows": [] }
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let err = bootstrap::run_with_redis(cfg, sink, source, RedisStore::mock())
        .await
        .err()
        .expect("expected error");
    assert!(matches!(err, BootstrapError::Restore(_)));
}

#[tokio::test]
async fn bootstrap_run_local_and_worker() {
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.startup_delay_ms = 200;
    cfg.rocketmq.enable_mm_consumer = false;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .unwrap();

    let body = serde_json::to_vec(&[sample_mq("live-1")]).unwrap();
    source.publish("contract_match_order", body);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(sink.sent().iter().any(|(t, _)| t.contains("market")));
    assert_eq!(running.symbols, vec!["btcusdt"]);
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_startup_delay_and_restore_success() {
    let server = MockServer::start().await;
    mount_rpc_stubs(
        &server,
        serde_json::json!([{
            "userId": 1,
            "uid": 1,
            "userType": 1,
            "type": 1,
            "marketId": 1,
            "coinId": 2,
            "symbolKey": "btcusdt",
            "coinMarket": "BTC/USDT",
            "entrustNo": "E-OK",
            "priceType": 1,
            "gear": 0,
            "status": 0,
            "remainAmount": "1",
            "price": "50000",
            "createTime": 1700000000
        }]),
    )
    .await;

    let mut cfg = sample_config(&server.uri(), &server.uri());
    cfg.startup_delay_ms = 1;
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_with_redis(
        cfg,
        Arc::clone(&sink),
        Arc::clone(&source),
        RedisStore::mock(),
    )
    .await
    .expect("bootstrap with restore");
    assert!(running.symbols.iter().any(|s| s == "btcusdt"));
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_coin_market_symbol_fallback() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"coinMarket":"SOL/USDT","mainStream":1}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/order/entrust-list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": { "rows": [] }
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_with_redis(
        cfg,
        Arc::clone(&sink),
        Arc::clone(&source),
        RedisStore::mock(),
    )
    .await
    .expect("bootstrap");
    assert!(running.symbols.iter().any(|s| s == "solusdt"));
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_empty_symbol_key_uses_coin_market() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"symbolKey":"","coinMarket":"ADA/USDT","mainStream":1}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/order/entrust-list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": { "rows": [] }
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_with_redis(
        cfg,
        Arc::clone(&sink),
        Arc::clone(&source),
        RedisStore::mock(),
    )
    .await
    .expect("bootstrap");
    assert!(running.symbols.iter().any(|s| s == "adausdt"));
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_no_symbols_after_market_skip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/market/coinMarkets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "code": 1,
            "data": [{"mainStream":1,"symbolKey":"","coinMarket":""}]
        })))
        .mount(&server)
        .await;

    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(MemoryMessageSource::new());
    let err = bootstrap::run_with_redis(cfg, sink, source, RedisStore::mock())
        .await
        .err()
        .expect("expected error");
    assert!(matches!(err, BootstrapError::NoMarkets));
}

#[tokio::test]
async fn bootstrap_run_local_without_startup_delay() {
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.startup_delay_ms = 0;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .unwrap();
    assert_eq!(running.symbols, vec!["btcusdt"]);
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_run_local_short_startup_delay() {
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.startup_delay_ms = 50;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .unwrap();
    assert_eq!(running.symbols, vec!["btcusdt"]);
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_run_local_mq_start_failure() {
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.startup_delay_ms = 0;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(FailMessageSource);
    let err = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .err()
    .expect("expected mq start failure");
    assert!(matches!(err, BootstrapError::Source(_)));
}

#[tokio::test]
async fn bootstrap_run_local_clamps_zero_queue_capacity() {
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.r#match.queue_capacity = 0;
    cfg.startup_delay_ms = 0;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .expect("bootstrap with clamped queue capacity");
    assert_eq!(running.symbols, vec!["btcusdt"]);
    shutdown_running(running, source.as_ref()).await;
}

#[tokio::test]
async fn bootstrap_run_with_redis_mq_start_failure() {
    let server = MockServer::start().await;
    mount_rpc_stubs(&server, serde_json::json!([])).await;
    let cfg = sample_config(&server.uri(), &server.uri());
    let sink: Arc<dyn OrderSink> = Arc::new(MemoryOrderSink::new());
    let source: Arc<dyn MessageSource> = Arc::new(FailMessageSource);
    let err = bootstrap::run_with_redis(cfg, sink, source, RedisStore::mock())
        .await
        .err()
        .expect("expected mq start failure");
    assert!(matches!(err, BootstrapError::Source(_)));
}

#[test]
fn spot_depth_stops_at_limit() {
    use match_core::Side;
    use std::str::FromStr;
    let orders: Vec<match_protocol::BbOrder> = (0..30)
        .map(|i| {
            let mut o = match_core::BbOrder::test_limit(
                Side::Buy,
                BigDecimal::from_str(&format!("{}", 100 + i)).unwrap(),
                &format!("o{i}"),
                1,
                "1",
            )
            .0;
            o.remaining_number = BigDecimal::from_str("1").unwrap();
            o
        })
        .collect();
    let (levels, _) = build_depth_levels(orders, 25);
    assert_eq!(levels.len(), 25);
}

#[test]
fn redis_mock_empty_rpop() {
    let mut store = RedisStore::mock();
    assert!(store.rpop_bytes("missing-list").unwrap().is_none());
}

#[test]
fn rpc_response_code_variants() {
    let ok: ResponseData<()> = serde_json::from_str(r#"{"code":1}"#).unwrap();
    assert!(ok.is_success());
    let n: ResponseData<()> = serde_json::from_str(r#"{"code":2}"#).unwrap();
    assert_eq!(n.code, 2);
    assert!(!n.is_success());
    let s: ResponseData<()> = serde_json::from_str(r#"{"code":"-1"}"#).unwrap();
    assert_eq!(s.code, -1);
    let u: ResponseData<()> = serde_json::from_str(r#"{"code":42}"#).unwrap();
    assert_eq!(u.code, 42);
    assert!(serde_json::from_str::<ResponseData<()>>(r#"{"code":"nope"}"#).is_err());
}

#[test]
fn mq_traits_default_stop() {
    struct NoopSource;
    impl MessageSource for NoopSource {
        fn start(
            &self,
            _subscriptions: &[Subscription],
            _handler: match_spot::mq::traits::InboundHandler,
        ) -> Result<(), match_spot::mq::traits::SourceError> {
            Ok(())
        }
    }
    NoopSource.stop();
}

#[test]
fn spot_depth_merges_same_price_levels() {
    use match_core::Side;
    use std::str::FromStr;
    let mut o1 = match_core::BbOrder::test_limit(
        Side::Buy,
        BigDecimal::from_str("10").unwrap(),
        "1",
        1,
        "5",
    )
    .0;
    o1.consumer_all_number = BigDecimal::from_str("1").unwrap();
    o1.remaining_number = BigDecimal::from_str("4").unwrap();
    let mut o2 = o1.clone();
    o2.trust_order_no = "2".into();
    let (levels, sum) = build_depth_levels(vec![o1, o2], 25);
    assert_eq!(levels.len(), 1);
    assert_eq!(levels[0].cumulative_commission_quantity, "8");
    assert_eq!(sum, BigDecimal::from_str("10").unwrap());
}

#[test]
fn memory_mq_poller_delivers_inbox() {
    let source = MemoryMessageSource::new();
    let router = Arc::new(InboundRouter::new());
    let (tx, _rx) = mpsc::channel(4);
    router.register_queue("btcusdt", tx);
    start_shard_consumers(&source, "contract_match_group", false, router).unwrap();
    source.enqueue_inbox(
        "contract_match_order",
        serde_json::to_vec(&[sample_mq("poller-1")]).unwrap(),
    );
    std::thread::sleep(Duration::from_millis(30));
    source.stop();
}

#[tokio::test]
async fn symbol_worker_panic_and_shutdown() {
    test_set_engine_panic(true);
    let mut cfg = sample_config("http://unused", "http://unused");
    cfg.startup_delay_ms = 0;
    let sink = Arc::new(MemoryOrderSink::new());
    let source = Arc::new(MemoryMessageSource::new());
    let running = bootstrap::run_local(
        &cfg,
        vec!["btcusdt".into()],
        Arc::clone(&sink) as Arc<dyn OrderSink>,
        Arc::clone(&source) as Arc<dyn MessageSource>,
    )
    .await
    .unwrap();

    let body = serde_json::to_vec(&[sample_mq("panic-order")]).unwrap();
    source.publish("contract_match_order", body);
    tokio::time::sleep(Duration::from_millis(50)).await;
    test_set_engine_panic(false);
    shutdown_running(running, source.as_ref()).await;
}
