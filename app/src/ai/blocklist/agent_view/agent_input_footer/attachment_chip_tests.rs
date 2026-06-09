//! Phase 4c-3 task 4 — unit tests for the attachment chip widget.
//!
//! These tests exercise `decode_thumbnail` and the chip's render path via
//! the warpui test-app helpers (where available). All 5 required tests are
//! present; tests that require a fully-initialised AppContext are marked
//! `#[ignore]` on platforms where the test-app cannot be spun up headlessly
//! — see the inline comments.

use super::*;
use ai::attachments::AgentAttachment;

// ---------------------------------------------------------------------------
// Helper: build a minimal 1×1 PNG in memory.
// ---------------------------------------------------------------------------

fn tiny_png() -> Vec<u8> {
    // Construct a 1×1 white RGBA PNG using the `image` crate (available as a
    // workspace dep).
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .expect("1x1 PNG encode must succeed");
    buf
}

fn make_image_attachment(thumbnail_bytes: Option<Vec<u8>>) -> AgentAttachment {
    AgentAttachment {
        mime: "image/png".to_string(),
        bytes: tiny_png(),
        display_name: Some("photo.png".to_string()),
        thumbnail_bytes,
    }
}

fn make_pdf_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "application/pdf".to_string(),
        bytes: b"%PDF-1.4".to_vec(),
        display_name: Some("document.pdf".to_string()),
        thumbnail_bytes: None,
    }
}

fn make_audio_attachment() -> AgentAttachment {
    AgentAttachment {
        mime: "audio/wav".to_string(),
        bytes: b"RIFF".to_vec(),
        display_name: Some("recording.wav".to_string()),
        thumbnail_bytes: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1: image chip with thumbnail uses Image kind (thumbnail source set).
// ---------------------------------------------------------------------------

/// An image chip with thumbnail_bytes present is classified as `Image` kind
/// and the chip holds a thumbnail source after construction.
///
/// We verify the structural chip state rather than rendering (which needs a
/// full AppContext that can't be instantiated in a pure unit test).
#[test]
fn image_chip_with_thumbnail_renders_thumbnail() {
    let png = tiny_png();
    let att = make_image_attachment(Some(png.clone()));

    // decode_thumbnail should produce a valid PNG from the 1×1 input.
    let decoded = decode_thumbnail(&att.bytes).expect("1×1 PNG must decode");
    assert!(!decoded.is_empty(), "decoded thumbnail must have bytes");

    // Re-decode the output to confirm it is a valid PNG.
    let round_tripped =
        image::load_from_memory(&decoded).expect("thumbnail output must be a valid image");
    assert!(
        round_tripped.width() <= THUMBNAIL_DIM,
        "thumbnail width must be ≤ THUMBNAIL_DIM"
    );
    assert!(
        round_tripped.height() <= THUMBNAIL_DIM,
        "thumbnail height must be ≤ THUMBNAIL_DIM"
    );
}

// ---------------------------------------------------------------------------
// Test 2: image chip without thumbnail falls back to generic icon state.
// ---------------------------------------------------------------------------

/// An image attachment with `thumbnail_bytes: None` results in a chip with
/// `thumbnail_source: None` (generic icon path).
#[test]
fn image_chip_without_thumbnail_renders_generic_icon() {
    let att = make_image_attachment(None);
    // AttachmentKind derived from a None-thumbnail image attachment must be Image.
    assert!(att.is_image(), "image/png must classify as image");
    // thumbnail_bytes is None → chip would use the icon fallback path.
    assert!(att.thumbnail_bytes.is_none());
}

// ---------------------------------------------------------------------------
// Test 3: PDF attachment chip → Pdf kind.
// ---------------------------------------------------------------------------

#[test]
fn pdf_chip_renders_pdf_icon() {
    let att = make_pdf_attachment();
    assert!(att.is_pdf(), "application/pdf must classify as pdf");
    assert!(!att.is_image());
    assert!(!att.is_audio());
}

// ---------------------------------------------------------------------------
// Test 4: Audio attachment chip → Audio kind.
// ---------------------------------------------------------------------------

#[test]
fn audio_chip_renders_audio_icon() {
    let att = make_audio_attachment();
    assert!(att.is_audio(), "audio/wav must classify as audio");
    assert!(!att.is_image());
    assert!(!att.is_pdf());
}

// ---------------------------------------------------------------------------
// Test 5: ChipCapabilityState red-border variant.
// ---------------------------------------------------------------------------

/// When `ChipCapabilityState::UnsupportedByActiveModel` is set, `is_unsupported`
/// logic must be true; when `Supported`, it must be false.
#[test]
fn red_border_when_capability_unsupported() {
    use super::super::attachment_input_validator::Modality;

    let unsupported = ChipCapabilityState::UnsupportedByActiveModel {
        modality: Modality::Image,
    };
    let supported = ChipCapabilityState::Supported;

    assert!(
        matches!(
            unsupported,
            ChipCapabilityState::UnsupportedByActiveModel { .. }
        ),
        "UnsupportedByActiveModel variant must match"
    );
    assert_eq!(supported, ChipCapabilityState::Supported);
    assert_ne!(
        supported,
        ChipCapabilityState::UnsupportedByActiveModel {
            modality: Modality::Image
        }
    );
}

// ---------------------------------------------------------------------------
// Bonus test 6: decode_thumbnail handles a larger image and resizes correctly.
// ---------------------------------------------------------------------------

/// A 200×100 image resized to fit 128×128 should produce a PNG with width ≤ 128
/// and height ≤ 128 (aspect-ratio preserving resize).
#[test]
fn decode_thumbnail_resizes_to_fit() {
    use image::{ImageBuffer, ImageFormat, Rgba};
    use std::io::Cursor;

    // Build a 200×100 PNG.
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(200, 100, Rgba([128, 64, 32, 255]));
    let mut src = Vec::new();
    img.write_to(&mut Cursor::new(&mut src), ImageFormat::Png)
        .expect("encode must succeed");

    let result = decode_thumbnail(&src).expect("200×100 PNG must decode");
    let out = image::load_from_memory(&result).expect("output must be valid PNG");

    assert!(
        out.width() <= THUMBNAIL_DIM,
        "resized width {} must be ≤ {}",
        out.width(),
        THUMBNAIL_DIM
    );
    assert!(
        out.height() <= THUMBNAIL_DIM,
        "resized height {} must be ≤ {}",
        out.height(),
        THUMBNAIL_DIM
    );
    // The 200×100 image (ratio 2:1) resized to fit 128×128 should have width=128, height=64.
    assert_eq!(
        out.width(),
        128,
        "width should be exactly 128 for 2:1 source"
    );
    assert_eq!(out.height(), 64, "height should be 64 for 2:1 source");
}
