use super::*;

fn cfg_default() -> CompactionConfig {
    CompactionConfig::default()
}

#[test]
fn fallback_constants_are_consistent() {
    const _: () = assert!(ModelLimit::FALLBACK.context > ModelLimit::FALLBACK.input);
    const _: () = assert!(ModelLimit::FALLBACK.input > ModelLimit::FALLBACK.max_output);
}

#[test]
fn from_context_window_none_returns_fallback() {
    let m = ModelLimit::from_context_window(None);
    assert_eq!(m.context, ModelLimit::FALLBACK.context);
}

#[test]
fn from_context_window_zero_returns_fallback() {
    let m = ModelLimit::from_context_window(Some(0));
    assert_eq!(m.context, ModelLimit::FALLBACK.context);
}

#[test]
fn from_context_window_caps_max_output_at_quarter() {
    let m = ModelLimit::from_context_window(Some(16_000));
    assert!(m.max_output <= 8_000);
    assert!(m.max_output <= m.context / 4 + 1);
    assert_eq!(m.input + m.max_output, m.context);
}

#[test]
fn token_counts_uses_total_when_set() {
    let t = TokenCounts {
        total: 5_000,
        input: 100,
        output: 100,
        ..Default::default()
    };
    assert_eq!(t.count(), 5_000);
}

#[test]
fn token_counts_falls_back_to_sum_when_total_zero() {
    let t = TokenCounts {
        total: 0,
        input: 1_000,
        output: 500,
        cache_read: 200,
        cache_write: 100,
    };
    assert_eq!(t.count(), 1_800);
}

#[test]
fn usable_returns_zero_for_zero_context() {
    let m = ModelLimit {
        context: 0,
        input: 0,
        max_output: 0,
    };
    assert_eq!(usable(&cfg_default(), m), 0);
}

#[test]
fn usable_subtracts_reserved_from_input_when_present() {
    let m = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    // default reserved = min(COMPACTION_BUFFER=20k, max_output=8k) = 8_000
    assert_eq!(usable(&cfg_default(), m), 180_000 - 8_000);
}

#[test]
fn usable_falls_back_to_context_minus_max_output_when_input_zero() {
    let m = ModelLimit {
        context: 100_000,
        input: 0,
        max_output: 4_000,
    };
    assert_eq!(usable(&cfg_default(), m), 100_000 - 4_000);
}

#[test]
fn usable_honours_reserved_override() {
    let mut cfg = cfg_default();
    cfg.reserved = Some(50_000);
    let m = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    assert_eq!(usable(&cfg, m), 180_000 - 50_000);
}

#[test]
fn is_overflow_false_when_auto_disabled() {
    let mut cfg = cfg_default();
    cfg.auto = false;
    let m = ModelLimit::FALLBACK;
    let t = TokenCounts {
        total: 1_000_000,
        ..Default::default()
    };
    assert!(!is_overflow(&cfg, t, m));
}

#[test]
fn is_overflow_false_when_context_zero() {
    let cfg = cfg_default();
    let m = ModelLimit {
        context: 0,
        input: 0,
        max_output: 0,
    };
    let t = TokenCounts {
        total: 1_000_000,
        ..Default::default()
    };
    assert!(!is_overflow(&cfg, t, m));
}

#[test]
fn is_overflow_triggers_at_or_above_usable_budget() {
    let cfg = cfg_default();
    let m = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };
    // usable = 180_000 - 8_000 = 172_000
    let just_under = TokenCounts {
        total: 171_999,
        ..Default::default()
    };
    let exactly_at = TokenCounts {
        total: 172_000,
        ..Default::default()
    };
    let just_over = TokenCounts {
        total: 172_001,
        ..Default::default()
    };
    assert!(!is_overflow(&cfg, just_under, m));
    assert!(is_overflow(&cfg, exactly_at, m));
    assert!(is_overflow(&cfg, just_over, m));
}
