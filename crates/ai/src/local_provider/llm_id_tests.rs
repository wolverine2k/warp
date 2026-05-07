use super::*;
use crate::LLMId;

#[test]
fn round_trip() {
    let id = encode("6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34", "deepseek-chat");
    assert_eq!(
        id.as_str(),
        "byop:6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34:deepseek-chat"
    );
    assert_eq!(
        decode(&id),
        Some((
            "6f3b1c54-8a02-4d8a-9fe6-1e2b09a06b34".to_owned(),
            "deepseek-chat".to_owned(),
        ))
    );
}

#[test]
fn model_id_with_colon_is_preserved() {
    // Some gateways (notably OpenRouter-style "vendor/model" or
    // "vendor:model:variant") use multiple colons in the model id. The
    // codec must split only on the first colon after the prefix.
    let id = encode("uuid-1", "vendor:model:v2");
    assert_eq!(
        decode(&id),
        Some(("uuid-1".to_owned(), "vendor:model:v2".to_owned()))
    );
}

#[test]
fn legacy_local_prefix_is_not_byop() {
    let legacy = LLMId::from("local:llama3.1");
    assert_eq!(decode(&legacy), None);
    assert!(!is_byop(&legacy));
}

#[test]
fn empty_provider_or_model_decodes_to_none() {
    assert_eq!(decode(&LLMId::from("byop::deepseek-chat")), None);
    assert_eq!(decode(&LLMId::from("byop:uuid-1:")), None);
    assert_eq!(decode(&LLMId::from("byop::")), None);
}

#[test]
fn missing_separator_decodes_to_none() {
    // `byop:<provider_id>` without the second colon is malformed.
    assert_eq!(decode(&LLMId::from("byop:uuid-only-no-model")), None);
}

#[test]
fn is_byop_recognizes_prefix_only() {
    assert!(is_byop(&LLMId::from("byop:x:y")));
    assert!(is_byop(&LLMId::from("byop:"))); // even malformed
    assert!(!is_byop(&LLMId::from("local:foo")));
    assert!(!is_byop(&LLMId::from("claude-3")));
    assert!(!is_byop(&LLMId::from("")));
}
