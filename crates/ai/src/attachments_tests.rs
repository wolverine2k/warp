use super::{encode_base64, encode_data_uri, AgentAttachment};

fn png_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![0x89, 0x50, 0x4e, 0x47],
        display_name: Some("test.png".into()),
    }
}

#[test]
fn is_image_recognizes_image_mimes() {
    assert!(png_attachment().is_image());
    assert!(AgentAttachment {
        mime: "image/jpeg".into(),
        ..png_attachment()
    }
    .is_image());
    assert!(!AgentAttachment {
        mime: "application/pdf".into(),
        ..png_attachment()
    }
    .is_image());
}

#[test]
fn is_pdf_matches_exact_mime() {
    assert!(AgentAttachment {
        mime: "application/pdf".into(),
        ..png_attachment()
    }
    .is_pdf());
    assert!(!png_attachment().is_pdf());
}

#[test]
fn is_audio_recognizes_audio_mimes() {
    assert!(AgentAttachment {
        mime: "audio/wav".into(),
        ..png_attachment()
    }
    .is_audio());
    assert!(AgentAttachment {
        mime: "audio/mpeg".into(),
        ..png_attachment()
    }
    .is_audio());
    assert!(!png_attachment().is_audio());
}

#[test]
fn encode_base64_round_trip() {
    let encoded = encode_base64(&[0x89, 0x50, 0x4e, 0x47]);
    assert_eq!(encoded, "iVBORw==");
}

#[test]
fn encode_data_uri_format() {
    let uri = encode_data_uri("image/png", &[0x89, 0x50, 0x4e, 0x47]);
    assert_eq!(uri, "data:image/png;base64,iVBORw==");
}
