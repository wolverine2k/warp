//! Phase 4c-3 task 4. Per-attachment chip rendered in the strip above the
//! agent editor. Used by the input footer (Tasks 4-8) and by the transcript
//! renderer (Task 9).
//!
//! One chip = one `AgentAttachment`. Displays:
//!  - Image with thumbnail: 32×32 thumbnail.
//!  - Image without thumbnail (decode in-flight / failed): generic image icon.
//!  - PDF: generic file icon.
//!  - Audio: generic microphone icon.
//!  All: truncated filename + × remove button.
//!  Red-border state when the currently active model no longer accepts this
//!  attachment's modality.

use ai::attachments::AgentAttachment;
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use warp_core::ui::theme::color::internal_colors;
use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Border, CacheOption, ChildAnchor, ChildView, ConstrainedBox, CornerRadius, CrossAxisAlignment,
    Element, Flex, Hoverable, Image, MainAxisSize, MouseStateHandle, OffsetPositioning,
    ParentAnchor, ParentElement, ParentOffsetBounds, Radius, Stack, Text,
};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle};

use crate::appearance::Appearance;
use crate::context_chips::display_chip::{chip_container, CHIP_BORDER_WIDTH};
use crate::ui_components::icons::Icon;
use crate::view_components::action_button::{ActionButton, ButtonSize, NakedTheme};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Whether the attachment modality is accepted by the currently active model.
/// This drives the red-border state on the chip.
// `UnsupportedByActiveModel` is constructed by the re-validation path wired in
// Task 8 (model-switch handler). Suppress the dead-code lint here.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipCapabilityState {
    /// The active model accepts this attachment's modality.
    Supported,
    /// The active model no longer accepts this modality (e.g. user switched
    /// to a text-only model after attaching an image).
    UnsupportedByActiveModel {
        modality: super::attachment_input_validator::Modality,
    },
}

/// Event emitted by `AttachmentChip`. Observed by the parent footer via
/// `ctx.subscribe_to_view`.
#[derive(Debug, Clone)]
pub enum AttachmentChipEvent {
    RemoveRequested,
}

/// Action dispatched when the user clicks the × button on a chip.
#[derive(Debug, Clone)]
pub enum AttachmentChipAction {
    Remove,
}

// ---------------------------------------------------------------------------
// Modality classification
// ---------------------------------------------------------------------------

/// Modality classification derived once from `AgentAttachment` at chip
/// construction time. Avoids keeping the full attachment bytes alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachmentKind {
    Image,
    Pdf,
    Audio,
}

impl AttachmentKind {
    fn from_attachment(att: &AgentAttachment) -> Self {
        if att.is_image() {
            Self::Image
        } else if att.is_pdf() {
            Self::Pdf
        } else {
            Self::Audio
        }
    }
}

// ---------------------------------------------------------------------------
// AttachmentChip view
// ---------------------------------------------------------------------------

/// A single attachment chip. Holds the display data and a stable
/// `MouseStateHandle` (constructed once during `new`).
pub struct AttachmentChip {
    /// `thumbnail_source` is `Some(AssetSource::Raw { id })` when thumbnail
    /// bytes have been inserted into the asset cache; `None` until then or for
    /// non-image attachments.
    thumbnail_source: Option<AssetSource>,
    display_name: String,
    kind: AttachmentKind,
    capability_state: ChipCapabilityState,
    /// Constructed once during `new`; cloned into each `Hoverable` during render.
    mouse_state: MouseStateHandle,
    /// × remove button — constructed once so its `MouseStateHandle` is stable.
    remove_button: ViewHandle<ActionButton>,
}

impl Entity for AttachmentChip {
    type Event = AttachmentChipEvent;
}

impl AttachmentChip {
    pub fn new(
        attachment: &AgentAttachment,
        thumbnail_source: Option<AssetSource>,
        capability_state: ChipCapabilityState,
        ctx: &mut ViewContext<Self>,
    ) -> Self {
        let display_name = truncate_name(
            attachment.display_name.as_deref().unwrap_or("attachment"),
            24,
        );

        let remove_button = ctx.add_typed_action_view(|_ctx| {
            ActionButton::new("", NakedTheme)
                .with_icon(Icon::X)
                .with_size(ButtonSize::XSmall)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(AttachmentChipAction::Remove);
                })
        });

        Self {
            thumbnail_source,
            display_name,
            kind: AttachmentKind::from_attachment(attachment),
            capability_state,
            mouse_state: Default::default(),
            remove_button,
        }
    }

    /// Update the thumbnail source after background decode completes.
    pub fn set_thumbnail_source(&mut self, source: AssetSource) {
        self.thumbnail_source = Some(source);
    }

    /// Update capability state (e.g. when the user switches models).
    /// Called from the model-switch handler wired in Task 8.
    #[allow(dead_code)]
    pub fn set_capability_state(&mut self, state: ChipCapabilityState) {
        self.capability_state = state;
    }
}

impl View for AttachmentChip {
    fn ui_name() -> &'static str {
        "AttachmentChip"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let is_unsupported = matches!(
            self.capability_state,
            ChipCapabilityState::UnsupportedByActiveModel { .. }
        );

        // Build the icon / thumbnail element.
        let icon_element: Box<dyn Element> = match self.kind {
            AttachmentKind::Image => {
                if let Some(source) = &self.thumbnail_source {
                    // Decoded thumbnail available — show it at 32×32.
                    ConstrainedBox::new(
                        Image::new(source.clone(), CacheOption::BySize)
                            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(3.)))
                            .finish(),
                    )
                    .with_width(32.)
                    .with_height(32.)
                    .finish()
                } else {
                    // Decode still in-flight or failed — generic icon.
                    small_icon(Icon::Image, internal_colors::neutral_6(theme))
                }
            }
            AttachmentKind::Pdf => small_icon(Icon::File, internal_colors::neutral_6(theme)),
            AttachmentKind::Audio => {
                small_icon(Icon::Microphone, internal_colors::neutral_6(theme))
            }
        };

        // Filename label.
        let label = Text::new(
            self.display_name.clone(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(internal_colors::neutral_6(theme))
        .finish();

        // Content row: icon + label.
        let icon_and_label = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_main_axis_size(MainAxisSize::Min)
            .with_spacing(4.)
            .with_child(icon_element)
            .with_child(label)
            .finish();

        let border_color = if is_unsupported {
            theme.ui_error_color()
        } else {
            internal_colors::neutral_3(theme)
        };

        let mouse_state = self.mouse_state.clone();
        let remove_button = self.remove_button.clone();
        let tooltip_text: Option<String> = match &self.capability_state {
            ChipCapabilityState::UnsupportedByActiveModel { modality } => {
                let modality_str = match modality {
                    super::attachment_input_validator::Modality::Image => "images",
                    super::attachment_input_validator::Modality::Pdf => "PDFs",
                    super::attachment_input_validator::Modality::Audio => "audio",
                };
                Some(format!(
                    "Active model doesn't accept {modality_str}; remove or switch model."
                ))
            }
            ChipCapabilityState::Supported => None,
        };

        Hoverable::new(mouse_state, move |state| {
            // Full chip content: icon+label row + × button.
            let mut chip_content = Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_main_axis_size(MainAxisSize::Min)
                .with_spacing(4.)
                .with_child(icon_and_label);
            chip_content.add_child(ChildView::new(&remove_button).finish());

            let border = Border::all(CHIP_BORDER_WIDTH).with_border_color(border_color);
            let mut chip = chip_container(chip_content.finish(), Some(border), appearance);
            if state.is_hovered() {
                chip = chip.with_background(theme.surface_2());
            }
            let chip_el = chip.finish();

            if state.is_hovered() {
                if let Some(ref text) = tooltip_text {
                    let tooltip_el = appearance
                        .ui_builder()
                        .tool_tip(text.clone())
                        .build()
                        .finish();
                    let mut stack = Stack::new().with_child(chip_el);
                    stack.add_positioned_overlay_child(tooltip_el, chip_tooltip_positioning());
                    return stack.finish();
                }
            }

            chip_el
        })
        .finish()
    }
}

impl TypedActionView for AttachmentChip {
    type Action = AttachmentChipAction;

    fn handle_action(&mut self, _action: &Self::Action, ctx: &mut ViewContext<Self>) {
        // Re-emit as an event so the parent footer can react via
        // `ctx.subscribe_to_view`.
        ctx.emit(AttachmentChipEvent::RemoveRequested);
        ctx.notify();
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Truncate `name` to at most `max_chars` characters, inserting "…" in the
/// middle if the name is longer.
fn truncate_name(name: &str, max_chars: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max_chars {
        return name.to_owned();
    }
    let half = max_chars / 2;
    let left: String = chars[..half].iter().collect();
    let right: String = chars[chars.len() - half..].iter().collect();
    format!("{left}…{right}")
}

fn small_icon(icon: Icon, color: ColorU) -> Box<dyn Element> {
    ConstrainedBox::new(warpui::elements::Icon::new(icon.into(), color).finish())
        .with_width(16.)
        .with_height(16.)
        .finish()
}

fn chip_tooltip_positioning() -> OffsetPositioning {
    OffsetPositioning::offset_from_parent(
        vec2f(0., -8.),
        ParentOffsetBounds::WindowByPosition,
        ParentAnchor::TopLeft,
        ChildAnchor::BottomLeft,
    )
}

// ---------------------------------------------------------------------------
// Background thumbnail decode helper
// ---------------------------------------------------------------------------

/// Maximum thumbnail dimension (width OR height). Aspect ratio is preserved.
pub const THUMBNAIL_DIM: u32 = 128;

/// Decode `bytes` as an image, resize to fit within `THUMBNAIL_DIM × THUMBNAIL_DIM`
/// (preserving aspect ratio), and re-encode as PNG.
///
/// Returns `None` when:
/// - the bytes are not a recognisable image format,
/// - decoding fails (corrupt file, unsupported codec),
/// - PNG re-encoding fails.
///
/// # Known gaps (to be revisited in a later task if needed)
/// - **Animated GIF**: only the first frame is decoded.
/// - **EXIF orientation**: not applied — JPEG files with an EXIF orientation
///   tag may appear rotated.
/// - **HEIC / HEIF**: not supported by the `image` crate; returns `None`.
pub fn decode_thumbnail(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Cursor;

    use image::imageops::FilterType;
    use image::ImageReader;

    let img = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    let resized = img.resize(THUMBNAIL_DIM, THUMBNAIL_DIM, FilterType::Triangle);

    let mut buf = Vec::new();
    resized
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    Some(buf)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "attachment_chip_tests.rs"]
mod tests;
