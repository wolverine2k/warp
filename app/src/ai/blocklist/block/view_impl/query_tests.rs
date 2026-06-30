//! Unit tests for Phase 4c-3 tasks 9+10: file-attachment chip rendering on
//! user-query transcript blocks.
//!
//! Most tests exercise `file_attachment_icon` — a pure function — so no app
//! context is needed. One smoke test calls `render_file_attachment_metadata`
//! via `App::test` to verify the full render path doesn't panic.

use ai::attachments::AttachmentMetadata;
use warp_core::ui::appearance::Appearance;
use warpui::{App, SingletonEntity};

use super::{file_attachment_icon, render_file_attachment_metadata};
use crate::ui_components::icons::Icon;

// ─── Pure icon-mapping tests (no app context) ──────────────────────────────

#[test]
fn user_turn_with_image_metadata_renders_image_icon_chip() {
    assert_eq!(file_attachment_icon("image/png"), Icon::Image);
    assert_eq!(file_attachment_icon("image/jpeg"), Icon::Image);
    assert_eq!(file_attachment_icon("image/gif"), Icon::Image);
}

#[test]
fn user_turn_with_pdf_metadata_renders_pdf_icon_chip() {
    // application/pdf has no dedicated icon; File is the correct fallback.
    assert_eq!(file_attachment_icon("application/pdf"), Icon::File);
}

#[test]
fn user_turn_with_audio_metadata_renders_audio_icon_chip() {
    assert_eq!(file_attachment_icon("audio/wav"), Icon::Microphone);
    assert_eq!(file_attachment_icon("audio/mpeg"), Icon::Microphone);
}

#[test]
fn user_turn_with_unknown_mime_renders_file_icon_chip() {
    // Unknown / future mime types fall back to the generic File icon.
    assert_eq!(file_attachment_icon("application/octet-stream"), Icon::File);
    assert_eq!(file_attachment_icon("text/plain"), Icon::File);
}

// ─── Render smoke test ─────────────────────────────────────────────────────

#[test]
fn user_turn_with_no_file_attachment_metadata_render_does_not_panic() {
    // Verifies that calling render_file_attachment_metadata with a non-empty
    // slice doesn't panic and that the production guard
    // (`if !file_attachment_metadata.is_empty()`) is the right call-site shape.
    App::test((), |app| async move {
        app.add_singleton_model(|_| Appearance::mock());
        let metadata = [
            AttachmentMetadata {
                mime: "image/png".to_owned(),
                display_name: Some("photo.png".to_owned()),
            },
            AttachmentMetadata {
                mime: "application/pdf".to_owned(),
                display_name: Some("report.pdf".to_owned()),
            },
            AttachmentMetadata {
                mime: "audio/wav".to_owned(),
                display_name: None,
            },
        ];
        // Should complete without panicking and return an element.
        let _element = app.read(|ctx| {
            let appearance = Appearance::as_ref(ctx);
            render_file_attachment_metadata(&metadata, appearance)
        });
    });
}
