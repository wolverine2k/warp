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
#[path = "overflow_tests.rs"]
mod tests;
