//! Token overflow detection — ported from openwarp's
//! `app/src/ai/byop_compaction/overflow.rs`, itself a port of opencode
//! `packages/opencode/src/session/overflow.ts`.
//!
//! ```ts
//! const COMPACTION_BUFFER = 20_000
//!
//! export function usable(input: { cfg, model }) {
//!   const context = input.model.limit.context
//!   if (context === 0) return 0
//!   const reserved = input.cfg.compaction?.reserved
//!     ?? Math.min(COMPACTION_BUFFER, ProviderTransform.maxOutputTokens(input.model))
//!   return input.model.limit.input
//!     ? Math.max(0, input.model.limit.input - reserved)
//!     : Math.max(0, context - ProviderTransform.maxOutputTokens(input.model))
//! }
//!
//! export function isOverflow(input: { cfg, tokens, model }) {
//!   if (input.cfg.compaction?.auto === false) return false
//!   if (input.model.limit.context === 0) return false
//!   const count = input.tokens.total
//!     || input.tokens.input + input.tokens.output + input.tokens.cache.read + input.tokens.cache.write
//!   return count >= usable(input)
//! }
//! ```

use super::consts::COMPACTION_BUFFER;
use super::CompactionConfig;

/// Per-model token limits, sourced from `LocalProviderConfig.context_window`
/// (when populated) plus reasonable fallbacks for `input` / `max_output`.
#[derive(Debug, Clone, Copy)]
pub struct ModelLimit {
    /// Total context window the model accepts.
    pub context: usize,
    /// Optional input-token cap (some providers split input/output). 0 means
    /// "unknown" — `usable` falls back to `context - max_output`.
    pub input: usize,
    /// Cap on a single response's output tokens.
    pub max_output: usize,
}

impl ModelLimit {
    /// Conservative defaults when the model card / settings lack metadata.
    /// Tracks today's mainstream Anthropic / OpenAI flagship models.
    pub const FALLBACK: ModelLimit = ModelLimit {
        context: 200_000,
        input: 180_000,
        max_output: 8_000,
    };

    /// Build a ModelLimit from a `LocalProviderConfig.context_window` override
    /// when populated; otherwise return [`FALLBACK`].
    ///
    /// `context_window` is the only field the user can configure, so we
    /// derive `max_output` and `input` from it conservatively:
    /// - `max_output = min(8_000, context / 4)` — leave at least 75% of the
    ///   window for input.
    /// - `input = max(0, context - max_output)` — assume providers don't
    ///   distinguish input vs. context unless they say otherwise.
    pub fn from_context_window(window: Option<usize>) -> Self {
        match window {
            None => Self::FALLBACK,
            Some(0) => Self::FALLBACK,
            Some(ctx) => {
                let max_output = (ctx / 4).clamp(1, 8_000);
                let input = ctx.saturating_sub(max_output);
                ModelLimit {
                    context: ctx,
                    input,
                    max_output,
                }
            }
        }
    }
}

/// Cumulative token usage observed for the conversation. Mirrors the shape
/// opencode reads off `MessageV2.Assistant.tokens` so a future StreamFinished
/// usage plumbing change can fan in directly.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokenCounts {
    /// LLM-reported total. When non-zero takes precedence over the sum of
    /// the parts (matches opencode's `tokens.total || ...` short-circuit).
    pub total: usize,
    pub input: usize,
    pub output: usize,
    pub cache_read: usize,
    pub cache_write: usize,
}

impl TokenCounts {
    /// `tokens.total || input + output + cache.read + cache.write`.
    pub fn count(&self) -> usize {
        if self.total > 0 {
            self.total
        } else {
            self.input + self.output + self.cache_read + self.cache_write
        }
    }
}

/// Usable input budget: `cfg.reserved ?? min(COMPACTION_BUFFER, max_output)`
/// is held back as headroom so a single overflowing response doesn't blow
/// past the model's context window.
pub fn usable(cfg: &CompactionConfig, model: ModelLimit) -> usize {
    if model.context == 0 {
        return 0;
    }
    let reserved = cfg
        .reserved
        .unwrap_or_else(|| COMPACTION_BUFFER.min(model.max_output));
    if model.input > 0 {
        model.input.saturating_sub(reserved)
    } else {
        model.context.saturating_sub(model.max_output)
    }
}

/// Returns true if the conversation has crossed the model's usable budget.
/// `cfg.auto == false` always returns false (the user has opted out of
/// auto-compaction). `model.context == 0` (unknown model) also returns false
/// to avoid spurious triggers on misconfigured profiles.
pub fn is_overflow(cfg: &CompactionConfig, tokens: TokenCounts, model: ModelLimit) -> bool {
    if !cfg.auto {
        return false;
    }
    if model.context == 0 {
        return false;
    }
    tokens.count() >= usable(cfg, model)
}

#[cfg(test)]
mod tests {
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
        let m = ModelLimit { context: 0, input: 0, max_output: 0 };
        assert_eq!(usable(&cfg_default(), m), 0);
    }

    #[test]
    fn usable_subtracts_reserved_from_input_when_present() {
        let m = ModelLimit { context: 200_000, input: 180_000, max_output: 8_000 };
        // default reserved = min(COMPACTION_BUFFER=20k, max_output=8k) = 8_000
        assert_eq!(usable(&cfg_default(), m), 180_000 - 8_000);
    }

    #[test]
    fn usable_falls_back_to_context_minus_max_output_when_input_zero() {
        let m = ModelLimit { context: 100_000, input: 0, max_output: 4_000 };
        assert_eq!(usable(&cfg_default(), m), 100_000 - 4_000);
    }

    #[test]
    fn usable_honours_reserved_override() {
        let mut cfg = cfg_default();
        cfg.reserved = Some(50_000);
        let m = ModelLimit { context: 200_000, input: 180_000, max_output: 8_000 };
        assert_eq!(usable(&cfg, m), 180_000 - 50_000);
    }

    #[test]
    fn is_overflow_false_when_auto_disabled() {
        let mut cfg = cfg_default();
        cfg.auto = false;
        let m = ModelLimit::FALLBACK;
        let t = TokenCounts { total: 1_000_000, ..Default::default() };
        assert!(!is_overflow(&cfg, t, m));
    }

    #[test]
    fn is_overflow_false_when_context_zero() {
        let cfg = cfg_default();
        let m = ModelLimit { context: 0, input: 0, max_output: 0 };
        let t = TokenCounts { total: 1_000_000, ..Default::default() };
        assert!(!is_overflow(&cfg, t, m));
    }

    #[test]
    fn is_overflow_triggers_at_or_above_usable_budget() {
        let cfg = cfg_default();
        let m = ModelLimit { context: 200_000, input: 180_000, max_output: 8_000 };
        // usable = 180_000 - 8_000 = 172_000
        let just_under = TokenCounts { total: 171_999, ..Default::default() };
        let exactly_at = TokenCounts { total: 172_000, ..Default::default() };
        let just_over = TokenCounts { total: 172_001, ..Default::default() };
        assert!(!is_overflow(&cfg, just_under, m));
        assert!(is_overflow(&cfg, exactly_at, m));
        assert!(is_overflow(&cfg, just_over, m));
    }
}
