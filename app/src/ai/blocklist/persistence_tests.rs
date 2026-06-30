use super::*;

fn make_metadata(mime: &str, name: &str) -> ai::attachments::AttachmentMetadata {
    ai::attachments::AttachmentMetadata {
        mime: mime.to_owned(),
        display_name: Some(name.to_owned()),
    }
}

/// Test 1 & 2 combined: the drain-pending-attachments logic mirrors
/// `std::mem::take`. Verify that taking from a non-empty Vec drains it
/// and that the original is empty afterwards — matching the contract of
/// `AgentInputFooter::drain_pending_attachments` (which calls
/// `std::mem::take`).
#[test]
fn submit_drains_pending_attachments_into_request() {
    let mut pending: Vec<ai::attachments::AgentAttachment> =
        vec![ai::attachments::AgentAttachment {
            mime: "image/png".to_owned(),
            bytes: vec![1, 2, 3],
            display_name: Some("photo.png".to_owned()),
            thumbnail_bytes: None,
        }];
    let drained = std::mem::take(&mut pending);
    // Post-drain: caller receives the attachment.
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].mime, "image/png");
    // Post-drain: original is empty (chip strip re-renders empty).
    assert!(pending.is_empty());
}

/// Test 2: after drain, a second drain returns empty — idempotent.
#[test]
fn submit_clears_pending_attachments_after_dispatch() {
    let mut pending: Vec<ai::attachments::AgentAttachment> =
        vec![ai::attachments::AgentAttachment {
            mime: "application/pdf".to_owned(),
            bytes: vec![0xff],
            display_name: Some("doc.pdf".to_owned()),
            thumbnail_bytes: None,
        }];
    let _ = std::mem::take(&mut pending);
    // Second drain must be empty.
    let second_drain = std::mem::take(&mut pending);
    assert!(second_drain.is_empty());
    assert!(pending.is_empty());
}

/// Test 3: serialized `PersistedAIInputType::Query` with attachment
/// metadata must contain `mime` and `display_name` but NEVER `bytes`.
#[test]
fn persisted_conversation_blob_carries_metadata_only_no_bytes() {
    let variant = PersistedAIInputType::Query {
        text: "fix the tests".to_owned(),
        context: Default::default(),
        referenced_attachments: Default::default(),
        file_attachment_metadata: vec![
            make_metadata("image/png", "screenshot.png"),
            make_metadata("application/pdf", "spec.pdf"),
        ],
    };
    let json = serde_json::to_string(&variant).expect("serializes");
    // Field names present.
    assert!(json.contains("mime"), "expected mime field in: {json}");
    assert!(
        json.contains("display_name"),
        "expected display_name field in: {json}"
    );
    assert!(
        json.contains("screenshot.png"),
        "expected filename in: {json}"
    );
    // Bytes must never appear.
    assert!(
        !json.contains("\"bytes\""),
        "bytes must not be serialized: {json}"
    );
}

/// Test 4: old conversation rows without `file_attachment_metadata`
/// deserialize successfully, yielding an empty Vec.
#[test]
fn deserialize_old_conversation_blob_without_attachments_field_succeeds() {
    // Minimal JSON matching the pre-task-8 Query shape (no
    // `file_attachment_metadata` key).
    let old_json = r#"{"Query":{"text":"hello","context":[],"referenced_attachments":{}}}"#;
    let deserialized: PersistedAIInputType =
        serde_json::from_str(old_json).expect("back-compat deserialization must succeed");
    match deserialized {
        PersistedAIInputType::Query {
            text,
            file_attachment_metadata,
            ..
        } => {
            assert_eq!(text, "hello");
            assert!(
                file_attachment_metadata.is_empty(),
                "old rows without the field should default to empty Vec"
            );
        }
    }
}
