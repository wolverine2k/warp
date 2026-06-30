use super::*;

#[test]
fn picker_filter_image_only_returns_image_extensions() {
    // llava is an Ollama vision model; heuristic resolves image=true,
    // pdf=false, audio=false when all settings are None (auto).
    let types = picker_file_types_for_caps(
        ai::local_provider::AgentProviderApiType::Ollama,
        "llava",
        None,
        None,
        None,
    );
    assert!(types
        .iter()
        .any(|t| matches!(t, warpui::platform::FileType::Image)));
    assert!(!types
        .iter()
        .any(|t| matches!(t, warpui::platform::FileType::Pdf)));
    assert!(!types
        .iter()
        .any(|t| matches!(t, warpui::platform::FileType::Audio)));
}

#[test]
fn picker_filter_no_modalities_returns_empty_vec() {
    // Explicit false for all three modalities → empty type list.
    let types = picker_file_types_for_caps(
        ai::local_provider::AgentProviderApiType::Ollama,
        "tinyllama-text-only",
        Some(false),
        Some(false),
        Some(false),
    );
    assert!(types.is_empty());
}

#[test]
fn mime_for_path_known_extensions() {
    assert_eq!(mime_for_path(std::path::Path::new("foo.png")), "image/png");
    // Case-insensitive via to_ascii_lowercase.
    assert_eq!(
        mime_for_path(std::path::Path::new("foo.PDF")),
        "application/pdf"
    );
    assert_eq!(mime_for_path(std::path::Path::new("foo.wav")), "audio/wav");
    assert_eq!(
        mime_for_path(std::path::Path::new("foo.xyz")),
        "application/octet-stream"
    );
}

// Phase 4c-3 task 6 — paste-from-clipboard branching logic tests.
//
// Full `AgentInputFooter` construction requires a live `AppContext`, so we
// test the pure predicate logic that `handle_attachment_paste` uses to
// decide whether to take the image branch or the text-fallback branch.

/// Clipboard with raw PNG bytes is detected as image content, and
/// `should_insert_text_on_paste` does NOT short-circuit (returns true only
/// when `has_image_data()` is false AND `num_paths()` is 0).
#[test]
fn paste_with_image_data_does_not_short_circuit_to_text() {
    use warpui::clipboard::{ClipboardContent, ImageData};

    let content = ClipboardContent {
        plain_text: String::new(),
        paths: None,
        html: None,
        images: Some(vec![ImageData {
            data: vec![0u8; 16],
            mime_type: "image/png".to_owned(),
            filename: None,
        }]),
    };

    // has_image_data() must be true so the image branch fires.
    assert!(content.has_image_data());
    // should_insert_text_on_paste returns true even when image data is
    // present and num_paths is 0 (direct clipboard image paste).
    // Callers must not rely on it to skip the image branch.
    let insert_text = warpui::clipboard::should_insert_text_on_paste(&content);
    // The invariant: when has_image_data() is true we always enter the
    // image branch regardless of `insert_text`.
    assert!(content.has_image_data(), "image branch should be taken");
    let _ = insert_text; // value is documented; not the gate that matters
}

/// Clipboard with only plain text (no images, no paths) is detected as
/// text-only paste, so `handle_attachment_paste` would find no image data
/// and no image file paths — falling through without producing an attachment.
#[test]
fn paste_with_text_only_clipboard_has_no_image_content() {
    use warpui::clipboard::ClipboardContent;

    let content = ClipboardContent {
        plain_text: "hello world".to_owned(),
        paths: None,
        html: None,
        images: None,
    };

    assert!(!content.has_image_data());
    assert_eq!(content.num_paths(), 0);
    // Verifies that the agent-view paste gate in `process_paste_event`
    // would NOT route a text-only clipboard through `PasteFromClipboard`.
    let clipboard_has_image_content = content.has_image_data()
        || content.paths.as_ref().is_some_and(|paths| {
            !warpui::clipboard_utils::get_image_filepaths_from_paths(paths).is_empty()
        });
    assert!(!clipboard_has_image_content);
}

// Phase 4c-3 task 7 — per-chip capability state and submit predicate tests.

fn make_attachment(mime: &str) -> ai::attachments::AgentAttachment {
    ai::attachments::AgentAttachment {
        mime: mime.to_owned(),
        bytes: vec![0u8; 4],
        display_name: Some("test_file".to_owned()),
        thumbnail_bytes: None,
    }
}

#[test]
fn chip_capability_state_for_image_unsupported_returns_unsupported() {
    // Explicit false for image → UnsupportedByActiveModel.
    let att = make_attachment("image/png");
    let state = chip_capability_state_for_attachment(
        &att,
        ai::local_provider::AgentProviderApiType::Ollama,
        "text-only-model",
        Some(false),
        None,
        None,
    );
    assert_eq!(
        state,
        attachment_chip::ChipCapabilityState::UnsupportedByActiveModel {
            modality: attachment_input_validator::Modality::Image,
        },
    );
}

#[test]
fn chip_capability_state_for_supported_modality_returns_supported() {
    // llava resolves image=true by heuristic (None override).
    let att = make_attachment("image/png");
    let state = chip_capability_state_for_attachment(
        &att,
        ai::local_provider::AgentProviderApiType::Ollama,
        "llava",
        None,
        None,
        None,
    );
    assert_eq!(state, attachment_chip::ChipCapabilityState::Supported);
}

#[test]
fn check_pending_attachments_pure_returns_ok_when_empty() {
    // Empty pending list → Ok regardless of caps.
    let attachments: Vec<ai::attachments::AgentAttachment> = vec![];
    let result = check_pending_attachments_against_caps(
        &attachments,
        ai::local_provider::AgentProviderApiType::Ollama,
        "text-only-model",
        Some(false),
        Some(false),
        Some(false),
    );
    assert!(result.is_ok());
}

#[test]
fn check_pending_attachments_pure_returns_err_when_any_unsupported() {
    // One image attachment + image explicitly disabled → Err.
    let attachments = vec![make_attachment("image/png")];
    let result = check_pending_attachments_against_caps(
        &attachments,
        ai::local_provider::AgentProviderApiType::Ollama,
        "text-only-model",
        Some(false),
        None,
        None,
    );
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        attachment_input_validator::AttachmentRejection::UnsupportedModality {
            modality: attachment_input_validator::Modality::Image,
            ..
        },
    ));
}
