fn main() {
    if let Ok(path) = std::env::var("MATCH_CONTRACT_CONFIG") {
        match match_contract::config::load_from_path(&path) {
            Ok(cfg) => {
                println!(
                    "match-contract loaded config (shard={}, market={}, order={})",
                    cfg.shard, cfg.rpc.market_base_url, cfg.rpc.order_base_url
                );
            }
            Err(e) => eprintln!("match-contract config load failed: {e}"),
        }
    }
    println!("match-contract stub — not production ready");
}
