use std::fs;

use serde_json::Value;
use tempfile::TempDir;

use super::*;

#[test]
fn prepare_gemini_settings_creates_file_with_api_key_auth() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");

    prepare_gemini_settings(&settings_path, false, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert_eq!(
        settings["security"]["auth"]["selectedType"],
        Value::String("gemini-api-key".to_owned()),
    );
}

#[test]
fn prepare_gemini_settings_preserves_unrelated_keys() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    fs::write(
        &settings_path,
        r#"{
            "ui": {"theme": "dark"},
            "security": {
                "folderTrust": {"enabled": true},
                "auth": {"enforcedType": "vertex-ai"}
            }
        }"#,
    )
    .unwrap();

    prepare_gemini_settings(&settings_path, false, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert_eq!(settings["ui"]["theme"], "dark");
    assert_eq!(settings["security"]["folderTrust"]["enabled"], true);
    // Sibling auth fields survive, and selectedType is set alongside them.
    assert_eq!(settings["security"]["auth"]["enforcedType"], "vertex-ai");
    assert_eq!(
        settings["security"]["auth"]["selectedType"],
        "gemini-api-key",
    );
}

#[test]
fn prepare_gemini_settings_surfaces_malformed_json_as_error() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    // `security` typed as a string instead of an object is a parse error;
    // we prefer surfacing that to silently rewriting user-owned state.
    fs::write(
        &settings_path,
        r#"{"ui":{"theme":"dark"},"security":"broken"}"#,
    )
    .unwrap();

    assert!(prepare_gemini_settings(&settings_path, false, None).is_err());
}

#[test]
fn prepare_gemini_trusted_folders_creates_file_with_working_dir() {
    let tmp = TempDir::new().unwrap();
    let trusted_path = tmp.path().join("trustedFolders.json");
    let working_dir = tmp.path().join("workspace/project");

    prepare_gemini_trusted_folders(&trusted_path, &working_dir).unwrap();

    let trusted: Value = serde_json::from_slice(&fs::read(trusted_path).unwrap()).unwrap();
    let working_dir_key = working_dir.to_string_lossy().to_string();
    assert_eq!(trusted[working_dir_key], "TRUST_FOLDER");
}

#[test]
fn prepare_gemini_trusted_folders_preserves_existing_entries() {
    let tmp = TempDir::new().unwrap();
    let trusted_path = tmp.path().join("trustedFolders.json");
    fs::write(
        &trusted_path,
        r#"{"/other/project":"TRUST_PARENT","/do/not/trust":"DO_NOT_TRUST"}"#,
    )
    .unwrap();
    let working_dir = tmp.path().join("workspace/project");

    prepare_gemini_trusted_folders(&trusted_path, &working_dir).unwrap();

    let trusted: Value = serde_json::from_slice(&fs::read(trusted_path).unwrap()).unwrap();
    assert_eq!(trusted["/other/project"], "TRUST_PARENT");
    assert_eq!(trusted["/do/not/trust"], "DO_NOT_TRUST");
    let working_dir_key = working_dir.to_string_lossy().to_string();
    assert_eq!(trusted[working_dir_key], "TRUST_FOLDER");
}

#[test]
fn prepare_gemini_settings_adds_context_file_name_when_system_prompt_present() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");

    prepare_gemini_settings(&settings_path, true, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert_eq!(
        settings["context"]["fileName"],
        Value::Array(vec![Value::String("OZ_SYSTEM_PROMPT.md".to_owned())]),
    );
}

#[test]
fn prepare_gemini_settings_appends_to_existing_context_file_name() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    fs::write(
        &settings_path,
        r#"{"context":{"fileName":["existing.md"]}}"#,
    )
    .unwrap();

    prepare_gemini_settings(&settings_path, true, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    let file_names: Vec<String> = settings["context"]["fileName"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(file_names, vec!["existing.md", "OZ_SYSTEM_PROMPT.md"]);
}

#[test]
fn prepare_gemini_settings_does_not_duplicate_system_prompt_file_name() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    fs::write(
        &settings_path,
        r#"{"context":{"fileName":["OZ_SYSTEM_PROMPT.md"]}}"#,
    )
    .unwrap();

    prepare_gemini_settings(&settings_path, true, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    let file_names: Vec<String> = settings["context"]["fileName"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(file_names, vec!["OZ_SYSTEM_PROMPT.md"]);
}

#[test]
fn prepare_gemini_settings_omits_context_when_no_system_prompt() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");

    prepare_gemini_settings(&settings_path, false, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert!(settings.get("context").is_none());
}

#[test]
fn prepare_gemini_settings_writes_byop_api_key_and_endpoint() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "AIza-byop-test".to_owned(),
        base_url: "https://my-gemini-proxy.example.com/v1beta".to_owned(),
    };

    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert_eq!(
        settings["security"]["auth"]["selectedType"],
        Value::String("gemini-api-key".to_owned()),
    );
    assert_eq!(
        settings["security"]["auth"]["apiKey"],
        Value::String("AIza-byop-test".to_owned()),
    );
    assert_eq!(
        settings["security"]["auth"]["endpoint"],
        Value::String("https://my-gemini-proxy.example.com/v1beta".to_owned()),
    );
}

#[test]
fn prepare_gemini_settings_clears_byop_fields_when_none() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "leak".to_owned(),
        base_url: "https://leak.example".to_owned(),
    };

    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();
    prepare_gemini_settings(&settings_path, false, None).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert!(
        settings["security"]["auth"].get("apiKey").is_none(),
        "apiKey must be cleared on non-BYOP run",
    );
    assert!(
        settings["security"]["auth"].get("endpoint").is_none(),
        "endpoint must be cleared on non-BYOP run",
    );
}

#[test]
fn prepare_gemini_settings_treats_empty_trim_as_none() {
    let tmp = TempDir::new().unwrap();
    let settings_path = tmp.path().join("settings.json");
    let byop = GeminiByopConfig {
        api_key: "   ".to_owned(),
        base_url: "\t\n".to_owned(),
    };

    prepare_gemini_settings(&settings_path, false, Some(&byop)).unwrap();

    let settings: Value = serde_json::from_slice(&fs::read(settings_path).unwrap()).unwrap();
    assert!(settings["security"]["auth"].get("apiKey").is_none());
    assert!(settings["security"]["auth"].get("endpoint").is_none());
}
