//! HTTP fetch of `https://models.dev/api.json`. No auth, 10s timeout,
//! 5 MB response cap. Returns the parsed `Vec<CatalogModel>` on success.

use std::time::Duration;

use super::parse::{parse_catalog, CatalogError, CatalogModel};

const CATALOG_URL: &str = "https://models.dev/api.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

pub async fn fetch_catalog(
    http: &reqwest::Client,
) -> Result<Vec<CatalogModel>, CatalogError> {
    let body = tokio::time::timeout(FETCH_TIMEOUT, fetch_body(http, CATALOG_URL))
        .await
        .map_err(|_| {
            CatalogError::Fetch(format!(
                "request timed out after {}s",
                FETCH_TIMEOUT.as_secs()
            ))
        })??;
    parse_catalog(&body)
}

/// Lower-level helper used by tests to point at a mock server.
pub async fn fetch_body(http: &reqwest::Client, url: &str) -> Result<String, CatalogError> {
    let resp = http
        .get(url)
        .send()
        .await
        .map_err(|e| CatalogError::Fetch(format!("{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CatalogError::HttpStatus(status.as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CatalogError::Fetch(format!("{e}")))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(CatalogError::BodyTooLarge);
    }
    String::from_utf8(bytes.to_vec()).map_err(|e| CatalogError::Parse(format!("{e}")))
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
