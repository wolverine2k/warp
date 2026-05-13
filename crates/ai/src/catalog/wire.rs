//! Wire-level deserialize types matching `https://models.dev/api.json`.
//! Every field except `id` and `name` is `#[serde(default)]` so upstream
//! schema drift produces a partial parse rather than a hard failure.

use std::collections::HashMap;

use serde::Deserialize;

/// Top-level: keyed by catalog provider id (e.g. "anthropic", "openai").
pub type WireRoot = HashMap<String, WireProvider>;

#[derive(Debug, Deserialize)]
pub struct WireProvider {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub models: HashMap<String, WireModel>,
}

#[derive(Debug, Deserialize)]
pub struct WireModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub tool_call: Option<bool>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub open_weights: Option<bool>,
    #[serde(default)]
    pub modalities: Option<WireModalities>,
    #[serde(default)]
    pub limit: Option<WireLimit>,
}

#[derive(Debug, Deserialize)]
pub struct WireModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct WireLimit {
    #[serde(default)]
    pub context: Option<u32>,
    #[serde(default)]
    pub output: Option<u32>,
}
