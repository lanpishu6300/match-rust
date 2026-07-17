use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

fn is_ascii_token(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn encode_part(value: &str) -> String {
    if is_ascii_token(value) {
        value.to_string()
    } else {
        URL_SAFE_NO_PAD.encode(value.as_bytes())
    }
}

/// Encodes a symbol key for RocketMQ topic suffixes, aligned with Java `CoinMarketEncode.encodeSymbolKey`.
pub fn encode_symbol_key(symbol_key: &str) -> String {
    if symbol_key.is_empty() {
        return String::new();
    }

    if symbol_key.contains('/') {
        let mut parts = symbol_key.splitn(2, '/');
        let base = parts.next().unwrap_or("");
        let quote = parts.next().unwrap_or("");

        let base_needs_encode = !is_ascii_token(base);
        let quote_needs_encode = !is_ascii_token(quote);

        if base_needs_encode || quote_needs_encode {
            return format!("{}/{}", encode_part(base), encode_part(quote));
        }

        return symbol_key.to_string();
    }

    if is_ascii_token(symbol_key) {
        symbol_key.to_string()
    } else {
        URL_SAFE_NO_PAD.encode(symbol_key.as_bytes())
    }
}
