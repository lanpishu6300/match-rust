use match_contract::rpc::{market, order};

#[test]
fn market_list_path() {
    assert_eq!(
        market::list_url("http://m"),
        "http://m/contract-market/contractcoinMarketList"
    );
}

#[test]
fn market_list_path_trailing_slash() {
    assert_eq!(
        market::list_url("http://m/"),
        "http://m/contract-market/contractcoinMarketList"
    );
}

#[test]
fn order_entrust_list_path() {
    assert_eq!(
        order::entrust_list_url("http://o"),
        "http://o/contract/entrust-list"
    );
}

#[test]
fn order_entrust_list_path_trailing_slash() {
    assert_eq!(
        order::entrust_list_url("http://o/"),
        "http://o/contract/entrust-list"
    );
}
