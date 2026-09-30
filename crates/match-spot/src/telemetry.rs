use std::sync::atomic::{AtomicU64, Ordering};

static ORDER_EVENTS_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORDERS_INBOUND_INVALID_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORDERS_PLACED_TOTAL: AtomicU64 = AtomicU64::new(0);
static ORDERS_CANCELLED_TOTAL: AtomicU64 = AtomicU64::new(0);
static TRADES_DEALS_TOTAL: AtomicU64 = AtomicU64::new(0);

pub fn record_order_event() {
    ORDER_EVENTS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_inbound_invalid() {
    ORDERS_INBOUND_INVALID_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_order_placed() {
    ORDERS_PLACED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_order_cancelled() {
    ORDERS_CANCELLED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_fill() {
    TRADES_DEALS_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn render_prometheus() -> String {
    format!(
        concat!(
            "# HELP match.order.events.total Spot match order events processed\n",
            "# TYPE match.order.events.total counter\n",
            "match.order.events.total {}\n",
            "# HELP match.orders.inbound.invalid.total Spot inbound orders rejected\n",
            "# TYPE match.orders.inbound.invalid.total counter\n",
            "match.orders.inbound.invalid.total {}\n",
            "# HELP match.orders.placed.total Spot orders accepted into match queue\n",
            "# TYPE match.orders.placed.total counter\n",
            "match.orders.placed.total {}\n",
            "# HELP match.orders.cancelled.total Spot orders revoked\n",
            "# TYPE match.orders.cancelled.total counter\n",
            "match.orders.cancelled.total {}\n",
            "# HELP match.trades.deals.total Spot fill events emitted\n",
            "# TYPE match.trades.deals.total counter\n",
            "match.trades.deals.total {}\n",
        ),
        ORDER_EVENTS_TOTAL.load(Ordering::Relaxed),
        ORDERS_INBOUND_INVALID_TOTAL.load(Ordering::Relaxed),
        ORDERS_PLACED_TOTAL.load(Ordering::Relaxed),
        ORDERS_CANCELLED_TOTAL.load(Ordering::Relaxed),
        TRADES_DEALS_TOTAL.load(Ordering::Relaxed),
    )
}
