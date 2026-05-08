//! In-memory unit tests for `AgentProviderSecrets`.
//!
//! Persistence-layer behavior (V1→V2 fallback, keychain round-trip) is not
//! covered here because the codebase has no in-memory `SecureStorage` mock —
//! `secure_storage::register_noop` is a true no-op. Phase 1b-2 Task 4's
//! migration helper tests exercise the full chain end-to-end via `App::test`,
//! providing the integration coverage these unit tests intentionally skip.

use super::*;
use warpui::{App, SingletonEntity};

#[test]
fn set_inserts_and_get_returns_value() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            warpui_extras::secure_storage::register_noop("warp_test", ctx);
        });
        app.add_singleton_model(AgentProviderSecrets::new);
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("provider-uuid-1", "sk-abc123".to_owned(), ctx);
            assert_eq!(secrets.get("provider-uuid-1"), Some("sk-abc123"));
            assert_eq!(secrets.get("provider-uuid-2"), None);
        });
    });
}

#[test]
fn set_with_empty_string_removes_entry() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            warpui_extras::secure_storage::register_noop("warp_test", ctx);
        });
        app.add_singleton_model(AgentProviderSecrets::new);
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("p1", "key1".to_owned(), ctx);
            assert_eq!(secrets.get("p1"), Some("key1"));
            secrets.set("p1", String::new(), ctx);
            assert_eq!(secrets.get("p1"), None);
        });
    });
}

#[test]
fn remove_clears_existing_entry() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            warpui_extras::secure_storage::register_noop("warp_test", ctx);
        });
        app.add_singleton_model(AgentProviderSecrets::new);
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("p1", "key1".to_owned(), ctx);
            secrets.set("p2", "key2".to_owned(), ctx);
            secrets.remove("p1", ctx);
            assert_eq!(secrets.get("p1"), None);
            assert_eq!(secrets.get("p2"), Some("key2"));
        });
    });
}

#[test]
fn provider_ids_iterates_all_keys() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            warpui_extras::secure_storage::register_noop("warp_test", ctx);
        });
        app.add_singleton_model(AgentProviderSecrets::new);
        AgentProviderSecrets::handle(&app).update(&mut app, |secrets, ctx| {
            secrets.set("alpha", "a".to_owned(), ctx);
            secrets.set("beta", "b".to_owned(), ctx);
            secrets.set("gamma", "c".to_owned(), ctx);
            let mut ids: Vec<String> = secrets.provider_ids().map(str::to_owned).collect();
            ids.sort();
            assert_eq!(ids, vec!["alpha", "beta", "gamma"]);
        });
    });
}
