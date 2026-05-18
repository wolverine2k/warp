//! Phase 4c-1 tests for the capability resolver. Cover each precedence
//! level (explicit, catalog, heuristic, conservative-fallback) for each
//! modality (image / pdf / audio).

use crate::catalog::CatalogModel;
use crate::local_provider::AgentProviderApiType;

use super::{resolve_audio, resolve_image, resolve_pdf};

fn catalog_entry(
    provider: &str,
    id: &str,
    image: bool,
    pdf: bool,
    audio: bool,
) -> CatalogModel {
    CatalogModel {
        catalog_provider: provider.to_string(),
        id: id.to_string(),
        name: id.to_string(),
        context_window: Some(8000),
        max_output_tokens: Some(4000),
        tool_call: true,
        reasoning: false,
        image,
        pdf,
        audio,
        open_weights: false,
    }
}

// ── Level 1: explicit user setting short-circuits ──────────────────────────

#[test]
fn explicit_some_true_wins_over_catalog_and_heuristic() {
    let catalog = vec![catalog_entry("openai", "gpt-4o", false, false, false)];
    assert!(resolve_image(
        AgentProviderApiType::OpenAi,
        "gpt-4o",
        Some(true),
        &catalog,
    ));
}

#[test]
fn explicit_some_false_wins_over_catalog_and_heuristic() {
    let catalog = vec![catalog_entry("openai", "gpt-4o", true, true, true)];
    assert!(!resolve_image(
        AgentProviderApiType::OpenAi,
        "gpt-4o",
        Some(false),
        &catalog,
    ));
    assert!(!resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-5-sonnet-20241022",
        Some(false),
        &catalog,
    ));
}

// ── Level 2: catalog lookup ────────────────────────────────────────────────

#[test]
fn catalog_lookup_resolves_image() {
    let catalog = vec![catalog_entry("anthropic", "claude-opus-4-7", true, true, false)];
    assert!(resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-opus-4-7",
        None,
        &catalog,
    ));
}

#[test]
fn catalog_lookup_resolves_pdf_and_audio_independently() {
    let catalog = vec![catalog_entry("google", "gemini-2-pro", true, true, true)];
    assert!(resolve_image(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
    assert!(resolve_pdf(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
    assert!(resolve_audio(AgentProviderApiType::Gemini, "gemini-2-pro", None, &catalog));
}

#[test]
fn catalog_lookup_can_return_false_explicitly() {
    // A catalog entry that says image:false should override the heuristic.
    let catalog = vec![catalog_entry("openai", "gpt-4o", false, false, false)];
    assert!(!resolve_image(AgentProviderApiType::OpenAi, "gpt-4o", None, &catalog));
}

#[test]
fn ollama_catalog_lookup_uses_open_weights_union() {
    // Ollama models live under various catalog_providers (meta, alibaba, etc.)
    // but the resolver matches by id within the open_weights subset.
    let mut llama = catalog_entry("meta", "llama-3.2-vision-11b", true, false, false);
    llama.open_weights = true;
    let mut qwen = catalog_entry("alibaba", "qwen2-vl-72b", true, false, false);
    qwen.open_weights = true;
    let catalog = vec![llama, qwen];
    assert!(resolve_image(
        AgentProviderApiType::Ollama,
        "llama-3.2-vision-11b",
        None,
        &catalog,
    ));
    assert!(resolve_image(
        AgentProviderApiType::Ollama,
        "qwen2-vl-72b",
        None,
        &catalog,
    ));
}

// ── Level 3: heuristic table (no catalog match) ────────────────────────────

#[test]
fn heuristic_resolves_openai_gpt4o_image_true() {
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4o", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4o-mini", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "gpt-4-turbo", None, &[]));
    assert!(resolve_image(AgentProviderApiType::OpenAi, "o1", None, &[]));
}

#[test]
fn heuristic_resolves_anthropic_claude_3_image_true_pdf_false() {
    // Claude 3 (no -5/-7 suffix) gets image but not pdf in the heuristic.
    assert!(resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-3-opus-20240229",
        None,
        &[],
    ));
    assert!(!resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-opus-20240229",
        None,
        &[],
    ));
}

#[test]
fn heuristic_resolves_claude_3_5_pdf_true() {
    assert!(resolve_pdf(
        AgentProviderApiType::Anthropic,
        "claude-3-5-sonnet-20241022",
        None,
        &[],
    ));
}

#[test]
fn heuristic_resolves_gemini_all_modalities() {
    assert!(resolve_image(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
    assert!(resolve_pdf(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
    assert!(resolve_audio(AgentProviderApiType::Gemini, "gemini-1.5-pro", None, &[]));
}

#[test]
fn heuristic_resolves_ollama_llava_image_only() {
    assert!(resolve_image(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
    assert!(!resolve_pdf(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
    assert!(!resolve_audio(AgentProviderApiType::Ollama, "llava:latest", None, &[]));
}

#[test]
fn heuristic_deepseek_all_false() {
    assert!(!resolve_image(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
    assert!(!resolve_pdf(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
    assert!(!resolve_audio(AgentProviderApiType::DeepSeek, "deepseek-chat", None, &[]));
}

// ── Level 4: conservative fallback ─────────────────────────────────────────

#[test]
fn unknown_model_returns_false() {
    // No catalog entry, no heuristic match — defaults to false.
    assert!(!resolve_image(
        AgentProviderApiType::OpenAi,
        "completely-made-up-model-id",
        None,
        &[],
    ));
    assert!(!resolve_image(
        AgentProviderApiType::Anthropic,
        "claude-2-old-model",
        None,
        &[],
    ));
    assert!(!resolve_image(
        AgentProviderApiType::Ollama,
        "mistral-text-only",
        None,
        &[],
    ));
}

// ── Case-insensitivity ──────────────────────────────────────────────────────

#[test]
fn heuristic_match_is_case_insensitive() {
    assert!(resolve_image(AgentProviderApiType::OpenAi, "GPT-4O", None, &[]));
    assert!(resolve_image(AgentProviderApiType::Anthropic, "Claude-3-Opus", None, &[]));
}
