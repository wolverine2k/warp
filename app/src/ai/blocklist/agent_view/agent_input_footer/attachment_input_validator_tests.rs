use super::{
    validate, ActiveModelCaps, AttachmentRejection, Modality, MAX_ATTACHMENTS_PER_TURN,
    MAX_ATTACHMENT_BYTES,
};
use ai::attachments::AgentAttachment;
use ai::local_provider::AgentProviderApiType;

/// Build a minimal `AgentAttachment` with the given mime and byte length.
fn make_attachment(mime: &str, byte_len: usize) -> AgentAttachment {
    AgentAttachment {
        mime: mime.into(),
        bytes: vec![0u8; byte_len],
        display_name: None,
        thumbnail_bytes: None,
    }
}

/// Default caps fixture: all modalities enabled via explicit `Some(true)`.
/// Uses Anthropic + a Claude model so heuristics would also pass, but
/// the explicit setting wins either way.
fn test_caps(model_id: &str) -> ActiveModelCaps<'_> {
    ActiveModelCaps {
        api_type: AgentProviderApiType::Anthropic,
        model_id,
        image_setting: Some(true),
        pdf_setting: Some(true),
        audio_setting: Some(true),
        catalog: &[],
    }
}

/// Caps with all modalities disabled via explicit `Some(false)`.
fn test_caps_none(model_id: &str) -> ActiveModelCaps<'_> {
    ActiveModelCaps {
        api_type: AgentProviderApiType::Anthropic,
        model_id,
        image_setting: Some(false),
        pdf_setting: Some(false),
        audio_setting: Some(false),
        catalog: &[],
    }
}

#[test]
fn valid_image_returns_ok() {
    let att = make_attachment("image/png", 100);
    let caps = test_caps("claude-3-5-sonnet-20241022");
    assert_eq!(validate(&att, 0, &caps), Ok(()));
}

#[test]
fn valid_pdf_returns_ok() {
    let att = make_attachment("application/pdf", 100);
    let caps = test_caps("claude-3-5-sonnet-20241022");
    assert_eq!(validate(&att, 0, &caps), Ok(()));
}

#[test]
fn valid_audio_returns_ok() {
    let att = make_attachment("audio/wav", 100);
    let caps = test_caps("claude-3-5-sonnet-20241022");
    assert_eq!(validate(&att, 0, &caps), Ok(()));
}

#[test]
fn image_on_image_unsupported_model_rejects() {
    let att = make_attachment("image/png", 100);
    let caps = test_caps_none("some-model");
    let result = validate(&att, 0, &caps);
    assert_eq!(
        result,
        Err(AttachmentRejection::UnsupportedModality {
            modality: Modality::Image,
            model_id: "some-model".to_string(),
        })
    );
}

#[test]
fn pdf_on_pdf_unsupported_model_rejects() {
    let att = make_attachment("application/pdf", 100);
    let caps = test_caps_none("some-model");
    let result = validate(&att, 0, &caps);
    assert_eq!(
        result,
        Err(AttachmentRejection::UnsupportedModality {
            modality: Modality::Pdf,
            model_id: "some-model".to_string(),
        })
    );
}

#[test]
fn audio_on_audio_unsupported_model_rejects() {
    let att = make_attachment("audio/wav", 100);
    let caps = test_caps_none("some-model");
    let result = validate(&att, 0, &caps);
    assert_eq!(
        result,
        Err(AttachmentRejection::UnsupportedModality {
            modality: Modality::Audio,
            model_id: "some-model".to_string(),
        })
    );
}

#[test]
fn file_too_large_rejects_at_boundary() {
    let caps = test_caps("claude-3-5-sonnet-20241022");

    // Exactly at limit → Ok
    let att_ok = make_attachment("image/png", MAX_ATTACHMENT_BYTES);
    assert_eq!(validate(&att_ok, 0, &caps), Ok(()));

    // One byte over → FileTooLarge
    let over = MAX_ATTACHMENT_BYTES + 1;
    let att_over = make_attachment("image/png", over);
    assert_eq!(
        validate(&att_over, 0, &caps),
        Err(AttachmentRejection::FileTooLarge { actual_bytes: over })
    );
}

#[test]
fn turn_limit_reached_rejects_when_pile_at_max() {
    let att = make_attachment("image/png", 100);
    let caps = test_caps("claude-3-5-sonnet-20241022");

    // At the limit → TurnLimitReached
    assert_eq!(
        validate(&att, MAX_ATTACHMENTS_PER_TURN, &caps),
        Err(AttachmentRejection::TurnLimitReached)
    );

    // One below the limit → Ok (assuming all other validations pass)
    assert_eq!(validate(&att, MAX_ATTACHMENTS_PER_TURN - 1, &caps), Ok(()));
}

#[test]
fn unknown_mime_rejects() {
    let att = make_attachment("text/plain", 100);
    let caps = test_caps("claude-3-5-sonnet-20241022");
    assert_eq!(
        validate(&att, 0, &caps),
        Err(AttachmentRejection::UnknownMime {
            mime: "text/plain".to_string(),
        })
    );
}
