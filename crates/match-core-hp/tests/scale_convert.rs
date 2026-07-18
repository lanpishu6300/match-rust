use match_core_hp::{from_lot, from_tick, to_lot, to_tick, SymbolScale};

#[test]
fn price_to_tick_scale_2() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert_eq!(to_tick(&s, "100.05").unwrap(), 10005);
    assert_eq!(from_tick(&s, 10005), "100.05");
}

#[test]
fn rejects_overflow_digits() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert!(to_tick(&s, "100.999").is_err()); // more fractional digits than scale
}

#[test]
fn qty_to_lot_scale_6() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert_eq!(to_lot(&s, "1.5").unwrap(), 1_500_000);
    assert_eq!(from_lot(&s, 1_500_000), "1.500000");
}

#[test]
fn integer_price_pads_fraction() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert_eq!(to_tick(&s, "100").unwrap(), 10000);
    assert_eq!(from_tick(&s, 10000), "100.00");
}

#[test]
fn rejects_empty_and_garbage() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert!(to_tick(&s, "").is_err());
    assert!(to_tick(&s, "abc").is_err());
    assert!(to_lot(&s, "1.0000001").is_err());
}

#[test]
fn accepts_plus_prefix_and_rejects_sign_only() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert_eq!(to_tick(&s, "+100.05").unwrap(), 10005);
    assert!(to_tick(&s, "+").is_err());
    assert!(to_tick(&s, "-").is_err());
}

#[test]
fn rejects_non_digit_fraction() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert!(to_tick(&s, "1.2x").is_err());
}

#[test]
fn scale_zero_formats_without_fraction() {
    let s = SymbolScale {
        price_scale: 0,
        qty_scale: 0,
    };
    assert_eq!(to_tick(&s, "42").unwrap(), 42);
    assert_eq!(from_tick(&s, 42), "42");
    assert_eq!(from_lot(&s, 7), "7");
}

#[test]
fn rejects_leading_dot_and_negative_fraction() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert!(to_tick(&s, ".25").is_err());
    assert_eq!(to_tick(&s, "-0.01").unwrap(), -1);
}

#[test]
fn rejects_i64_min_negation_overflow() {
    let s = SymbolScale {
        price_scale: 0,
        qty_scale: 0,
    };
    assert!(to_tick(&s, "-9223372036854775808").is_err());
}

#[test]
fn zero_and_negative_formatting() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 2,
    };
    assert_eq!(to_tick(&s, "0.00").unwrap(), 0);
    assert_eq!(from_tick(&s, -125), "-1.25");
    assert_eq!(from_lot(&s, 0), "0.00");
}

#[test]
fn rejects_non_digit_integer_part() {
    let s = SymbolScale {
        price_scale: 2,
        qty_scale: 6,
    };
    assert!(to_tick(&s, "1a.0").is_err());
    assert!(to_tick(&s, "  ").is_err());
}
