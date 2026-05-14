use std::sync::Once;

use mockito::Server;

use super::{fetch_body, CatalogError};

fn ensure_rustls_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

fn http_client() -> reqwest::Client {
    ensure_rustls_provider();
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn returns_body_on_200() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/api.json")
        .with_status(200)
        .with_body(r#"{"openai":{"id":"openai","name":"OpenAI","models":{}}}"#)
        .create_async()
        .await;
    let body = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap();
    assert!(body.contains("openai"));
}

#[tokio::test]
async fn http_500_returns_http_status_error() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/api.json")
        .with_status(503)
        .with_body("upstream busy")
        .create_async()
        .await;
    let err = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::HttpStatus(503)));
}

#[tokio::test]
async fn rejects_body_over_5mb_cap() {
    let mut server = Server::new_async().await;
    let big = "x".repeat(5 * 1024 * 1024 + 1);
    let _m = server
        .mock("GET", "/api.json")
        .with_status(200)
        .with_body(big)
        .create_async()
        .await;
    let err = fetch_body(&http_client(), &format!("{}/api.json", server.url()))
        .await
        .unwrap_err();
    assert!(matches!(err, CatalogError::BodyTooLarge));
}
