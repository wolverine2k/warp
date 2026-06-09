use super::{encode_base64, encode_data_uri, AgentAttachment, AttachmentMetadata};

fn png_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![0x89, 0x50, 0x4e, 0x47],
        display_name: Some("test.png".into()),
        thumbnail_bytes: None,
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

#[test]
fn metadata_from_attachment_drops_bytes() {
    let att = AgentAttachment {
        mime: "image/png".into(),
        bytes: vec![1, 2, 3, 4],
        display_name: Some("test.png".into()),
        thumbnail_bytes: None,
    };
    let md = AttachmentMetadata::from(&att);
    assert_eq!(md.mime, "image/png");
    assert_eq!(md.display_name, Some("test.png".into()));
}

#[test]
fn metadata_serde_round_trip() {
    let md = AttachmentMetadata {
        mime: "image/jpeg".into(),
        display_name: Some("photo.jpg".into()),
    };
    let json = serde_json::to_string(&md).unwrap();
    // bytes must not appear in the serialized form
    assert!(
        !json.contains("bytes"),
        "serialized metadata must not contain bytes field"
    );
    let restored: AttachmentMetadata = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, md);
}

#[test]
fn metadata_deserializes_with_missing_display_name() {
    let json = r#"{"mime":"image/png"}"#;
    let md: AttachmentMetadata = serde_json::from_str(json).unwrap();
    assert_eq!(md.mime, "image/png");
    assert_eq!(md.display_name, None);
}

#[test]
fn attachment_thumbnail_bytes_default_none() {
    let att = AgentAttachment {
        mime: "application/pdf".into(),
        bytes: vec![],
        display_name: None,
        thumbnail_bytes: None,
    };
    assert!(att.thumbnail_bytes.is_none());
    // confirm Debug and Clone work
    let cloned = att.clone();
    assert_eq!(format!("{att:?}"), format!("{cloned:?}"));
}
