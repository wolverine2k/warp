//! Multi-provider settings widget for custom Agent providers.
//!
//! Renders a list of provider cards under Settings → AI, each with
//! Name / Base URL / API Key inputs, an api_type chip selector, and a
//! models table with per-row Display Name / Model ID / Context Window
//! inputs and a tool_call toggle.
//!
//! Phase 4 features (models.dev quick-add, /models fetch, expand/collapse,
//! multimodal toggles, reasoning UI) are intentionally omitted.

use std::collections::HashMap;

use settings::Setting;
use strum::IntoEnumIterator;
use warpui::elements::{
    ChildView, Container, CornerRadius, CrossAxisAlignment, Expanded, Flex, MainAxisAlignment,
    MouseStateHandle, ParentElement, Radius, Text,
};
use warpui::ui_components::{
    button::ButtonVariant,
    components::{Coords, UiComponent, UiComponentStyles},
};
use warpui::{AppContext, Element, SingletonEntity, ViewContext, ViewHandle};

use crate::appearance::Appearance;
use crate::editor::{
    EditorView, Event as EditorEvent, SingleLineEditorOptions, TextColors, TextOptions,
};
use crate::settings::{AISettings, AgentProvider, AgentProviderApiType, AgentProviderModel};

use super::ai_page::{AISettingsPageAction, AISettingsPageView};
use super::fetched_models_modal::FetchedModelsModalState;
use super::settings_page::{build_sub_header, render_separator, SettingsWidget, HEADER_PADDING};
use crate::ai::agent_providers::fetch_models::MAX_ENTRIES as FETCH_MODELS_MAX_ENTRIES;

const CARD_BUTTON_FONT_SIZE: f32 = 12.0;
const CARD_BUTTON_PADDING: f32 = 6.0;
const FIELD_LABEL_MARGIN_TOP: f32 = 6.0;
const FIELD_LABEL_MARGIN_BOTTOM: f32 = 2.0;
const MODEL_ROW_GAP: f32 = 6.0;

// ---------------------------------------------------------------------------
// Probe UI state (Phase 2)
// ---------------------------------------------------------------------------

/// Visual state of the per-provider "Test connection" button. Keyed by
/// `AgentProvider.id` on `AISettingsPageView.agent_provider_probe_states`.
/// Reset to `Idle` when the user edits the provider's base URL, API key,
/// or api_type — a stale `Ok` would lie about the post-edit config.
#[derive(Debug, Clone, Default)]
pub(super) enum ProbeUiState {
    #[default]
    Idle,
    Probing,
    Ok,
    Failed(String),
}

/// Maximum number of characters of the failure reason rendered on the button
/// label. Tooltips would be a Phase 4 polish; for now the label itself is the
/// user-visible surface so we cap to keep the bottom row layout stable.
const PROBE_FAILED_LABEL_BUDGET: usize = 60;

impl ProbeUiState {
    pub(super) fn button_label(&self) -> String {
        match self {
            ProbeUiState::Idle => "Test connection".to_string(),
            ProbeUiState::Probing => "Testing…".to_string(),
            ProbeUiState::Ok => "✓ Connected".to_string(),
            ProbeUiState::Failed(msg) => {
                let trimmed = msg.trim();
                if trimmed.is_empty() {
                    "✗ Failed".to_string()
                } else {
                    let excerpt: String = trimmed.chars().take(PROBE_FAILED_LABEL_BUDGET).collect();
                    format!("✗ {excerpt}")
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-model-row view handles
// ---------------------------------------------------------------------------

struct ModelRowHandles {
    name_editor: ViewHandle<EditorView>,
    id_editor: ViewHandle<EditorView>,
    context_editor: ViewHandle<EditorView>,
    tool_call_chip_state: MouseStateHandle,
    remove_button_state: MouseStateHandle,
}

// ---------------------------------------------------------------------------
// Per-provider-card view handles
// ---------------------------------------------------------------------------

struct ProviderCardHandles {
    name_editor: ViewHandle<EditorView>,
    base_url_editor: ViewHandle<EditorView>,
    api_key_editor: ViewHandle<EditorView>,
    remove_button_state: MouseStateHandle,
    add_model_button_state: MouseStateHandle,
    test_connection_button_state: MouseStateHandle,
    /// Phase 4a. Mouse state for the "Fetch models" button rendered next
    /// to "Test connection" in the card footer.
    fetch_models_button_state: MouseStateHandle,
    api_type_chip_states: HashMap<AgentProviderApiType, MouseStateHandle>,
    model_rows: Vec<ModelRowHandles>,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// Multi-provider list-view widget for the AI settings page.
pub(super) struct AgentProvidersWidget {
    add_button_state: MouseStateHandle,
    cards: Vec<ProviderCardHandles>,
    /// Phase 4a. Mouse-state handles for the "Fetched models" modal.
    /// Pre-allocated at widget construction so the render path never
    /// builds `MouseStateHandle::default()` inline (per `CLAUDE.md`'s
    /// repeated-init warning). Lives on the widget rather than on
    /// `FetchedModelsModalState` so the modal-state module stays pure
    /// and unit-testable; the widget rebuilds independently of modal
    /// open/close cycles.
    fetch_modal: FetchModalHandles,
}

/// Mouse-state handles for the "Fetched models" modal. `row_states` is
/// sized to `MAX_ENTRIES` so the modal can address up to 200 rows by
/// positional index — the same cap that bounds `fetch_models()`.
struct FetchModalHandles {
    select_all_state: MouseStateHandle,
    select_none_state: MouseStateHandle,
    cancel_state: MouseStateHandle,
    commit_state: MouseStateHandle,
    row_states: Vec<MouseStateHandle>,
}

impl FetchModalHandles {
    fn new() -> Self {
        Self {
            select_all_state: MouseStateHandle::default(),
            select_none_state: MouseStateHandle::default(),
            cancel_state: MouseStateHandle::default(),
            commit_state: MouseStateHandle::default(),
            row_states: (0..FETCH_MODELS_MAX_ENTRIES)
                .map(|_| MouseStateHandle::default())
                .collect(),
        }
    }
}

impl AgentProvidersWidget {
    pub(super) fn new(ctx: &mut ViewContext<AISettingsPageView>) -> Self {
        let providers = AISettings::as_ref(ctx).agent_providers.value().clone();
        let cards: Vec<ProviderCardHandles> = providers
            .iter()
            .enumerate()
            .map(|(provider_index, provider)| {
                Self::build_provider_card(provider, provider_index, ctx)
            })
            .collect();

        Self {
            add_button_state: MouseStateHandle::default(),
            cards,
            fetch_modal: FetchModalHandles::new(),
        }
    }

    // ---- Construction helpers ------------------------------------------------

    fn build_provider_card(
        provider: &AgentProvider,
        provider_index: usize,
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ProviderCardHandles {
        // Name editor
        let initial_name = provider.name.clone();
        let name_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("Provider name", ctx);
            if !initial_name.is_empty() {
                editor.set_buffer_text(&initial_name, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&name_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(AISettingsPageAction::UpdateAgentProviderName {
                    provider_index,
                    name: buffer_text,
                });
            }
        });

        // Base URL editor
        let initial_base_url = provider.base_url.clone();
        let base_url_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("http://localhost:11434/v1", ctx);
            if !initial_base_url.is_empty() {
                editor.set_buffer_text(&initial_base_url, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&base_url_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::UpdateAgentProviderBaseUrl {
                        provider_index,
                        base_url: buffer_text,
                    },
                );
            }
        });

        // API key editor (password mode)
        let initial_api_key: String = ::ai::local_provider::AgentProviderSecrets::as_ref(ctx)
            .get(&provider.id)
            .map(str::to_owned)
            .unwrap_or_default();
        let api_key_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, true);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("optional bearer token", ctx);
            if !initial_api_key.is_empty() {
                editor.set_buffer_text(&initial_api_key, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&api_key_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::UpdateAgentProviderApiKey {
                        provider_index,
                        api_key: buffer_text,
                    },
                );
            }
        });

        // API type chip mouse states — one per variant, created during construction
        let mut api_type_chip_states = HashMap::new();
        for variant in AgentProviderApiType::iter() {
            api_type_chip_states.insert(variant, MouseStateHandle::default());
        }

        // Model rows
        let model_rows: Vec<ModelRowHandles> = provider
            .models
            .iter()
            .enumerate()
            .map(|(model_index, model)| {
                Self::build_model_row(provider_index, model_index, model, ctx)
            })
            .collect();

        ProviderCardHandles {
            name_editor,
            base_url_editor,
            api_key_editor,
            remove_button_state: MouseStateHandle::default(),
            add_model_button_state: MouseStateHandle::default(),
            test_connection_button_state: MouseStateHandle::default(),
            fetch_models_button_state: MouseStateHandle::default(),
            api_type_chip_states,
            model_rows,
        }
    }

    fn build_model_row(
        provider_index: usize,
        model_index: usize,
        model: &AgentProviderModel,
        ctx: &mut ViewContext<AISettingsPageView>,
    ) -> ModelRowHandles {
        // Name editor
        let initial_name = model.name.clone();
        let name_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("Display name", ctx);
            if !initial_name.is_empty() {
                editor.set_buffer_text(&initial_name, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&name_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::UpdateAgentProviderModelName {
                        provider_index,
                        model_index,
                        name: buffer_text,
                    },
                );
            }
        });

        // ID editor
        let initial_id = model.id.clone();
        let id_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("model-id", ctx);
            if !initial_id.is_empty() {
                editor.set_buffer_text(&initial_id, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&id_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::UpdateAgentProviderModelId {
                        provider_index,
                        model_index,
                        id: buffer_text,
                    },
                );
            }
        });

        // Context window editor
        let initial_context = if model.context_window == 0 {
            String::new()
        } else {
            model.context_window.to_string()
        };
        let context_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(appearance, false);
            let mut editor = EditorView::single_line(options, ctx);
            editor.set_placeholder_text("32768", ctx);
            if !initial_context.is_empty() {
                editor.set_buffer_text(&initial_context, ctx);
            }
            editor
        });
        ctx.subscribe_to_view(&context_editor, move |_, editor, event, ctx| {
            if matches!(event, EditorEvent::Blurred | EditorEvent::Enter) {
                let buffer_text = editor.as_ref(ctx).buffer_text(ctx);
                let value = parse_token_count(&buffer_text);
                ctx.dispatch_typed_action_deferred(
                    AISettingsPageAction::UpdateAgentProviderModelContextWindow {
                        provider_index,
                        model_index,
                        context_window: value,
                    },
                );
            }
        });

        ModelRowHandles {
            name_editor,
            id_editor,
            context_editor,
            tool_call_chip_state: MouseStateHandle::default(),
            remove_button_state: MouseStateHandle::default(),
        }
    }

    // ---- Render helpers ------------------------------------------------------

    fn render_card_button(
        label: impl Into<String>,
        mouse_state: MouseStateHandle,
        action: AISettingsPageAction,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        appearance
            .ui_builder()
            .button(ButtonVariant::Secondary, mouse_state)
            .with_style(UiComponentStyles {
                font_size: Some(CARD_BUTTON_FONT_SIZE),
                padding: Some(Coords::uniform(CARD_BUTTON_PADDING)),
                ..Default::default()
            })
            .with_centered_text_label(label.into())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(action.clone());
            })
            .finish()
    }

    fn render_api_type_field(
        provider: &AgentProvider,
        provider_index: usize,
        card: &ProviderCardHandles,
        label_color: warp_core::ui::theme::Fill,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let label_text = Container::new(
            Text::new(
                "API type".to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let mut chip_row = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        for variant in AgentProviderApiType::iter() {
            let state = card
                .api_type_chip_states
                .get(&variant)
                .cloned()
                .unwrap_or_default();
            let is_selected = provider.api_type == variant;
            let variant_label = api_type_display_name(variant);
            let label = if is_selected {
                format!("● {variant_label}")
            } else {
                variant_label.to_owned()
            };
            let chip = Self::render_card_button(
                label,
                state,
                AISettingsPageAction::UpdateAgentProviderApiType {
                    provider_index,
                    api_type: variant,
                },
                appearance,
            );
            chip_row = chip_row.with_child(Container::new(chip).with_margin_right(6.).finish());
        }

        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(label_text)
            .with_child(chip_row.finish())
            .finish()
    }

    /// Phase 4a. Renders the "Fetched models" modal as a card-style
    /// panel above the provider cards. Returns `None` when no modal is
    /// open. The panel hosts the row list, an optional empty/truncation
    /// caption, and the Select all / Select none / Cancel / Commit
    /// footer.
    fn render_fetched_models_modal(
        &self,
        modal: &FetchedModelsModalState,
        providers: &[AgentProvider],
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        // Header label includes the provider name when we can still find
        // it. If the provider was removed mid-flight we render a generic
        // header — the modal will be torn down on the next user action.
        let provider_label = providers
            .get(modal.provider_index)
            .map(|p| {
                if p.name.is_empty() {
                    "(unnamed provider)".to_string()
                } else {
                    p.name.clone()
                }
            })
            .unwrap_or_else(|| "(removed provider)".into());
        let header_text = format!("Fetched models — {provider_label}");
        let header_node = Container::new(
            Text::new(
                header_text,
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(appearance.theme().active_ui_text_color().into())
            .finish(),
        )
        .with_margin_bottom(8.)
        .finish();

        let mut column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(header_node);

        // Caption: empty / truncation / count summary. Plain text, dim.
        let caption_text = if modal.fetched.is_empty() {
            Some("Upstream returned 0 models.".to_string())
        } else if modal.fetched.len() >= FETCH_MODELS_MAX_ENTRIES {
            Some(format!(
                "Showing first {} models — narrow your provider's catalog or wait for Phase 4b.",
                FETCH_MODELS_MAX_ENTRIES
            ))
        } else {
            Some(format!(
                "{} model(s) returned. {} already on this provider.",
                modal.fetched.len(),
                modal.already_added.len()
            ))
        };
        if let Some(text) = caption_text {
            column = column.with_child(
                Container::new(
                    Text::new(text, appearance.ui_font_family(), appearance.ui_font_size())
                        .with_color(appearance.theme().disabled_ui_text_color().into())
                        .soft_wrap(true)
                        .finish(),
                )
                .with_margin_bottom(8.)
                .finish(),
            );
        }

        // Row list. Each row is a Secondary-themed button labeled
        // "[☐/☑] {id}  {display}  {metadata}". Already-added rows
        // render as a flat Container (no on_click) labeled "✓ {id}".
        for (row_index, model) in modal.fetched.iter().enumerate() {
            let is_already = modal.already_added.contains(&model.id);
            let is_checked = modal.checked.contains(&model.id);
            let display_part = match model.display_name.as_deref() {
                Some(name) if name != model.id => format!("  ({name})"),
                _ => String::new(),
            };
            let metadata_part = match (model.context_window, model.max_output_tokens) {
                (Some(c), Some(o)) => format!("  · {c} ctx · {o} out"),
                (Some(c), None) => format!("  · {c} ctx"),
                (None, Some(o)) => format!("  · {o} out"),
                (None, None) => String::new(),
            };
            let row_label = if is_already {
                format!("✓ {}{display_part}{metadata_part}", model.id)
            } else if is_checked {
                format!("☑ {}{display_part}{metadata_part}", model.id)
            } else {
                format!("☐ {}{display_part}{metadata_part}", model.id)
            };

            let row_element: Box<dyn Element> = if is_already {
                Container::new(
                    Text::new(row_label, appearance.ui_font_family(), CARD_BUTTON_FONT_SIZE)
                        .with_color(appearance.theme().disabled_ui_text_color().into())
                        .finish(),
                )
                .with_uniform_padding(CARD_BUTTON_PADDING)
                .finish()
            } else {
                // Bound the per-row mouse-state index by the pre-allocated
                // pool. Modal.fetched is capped to MAX_ENTRIES by the
                // helper so we never overrun, but guard defensively.
                let mouse_state = self
                    .fetch_modal
                    .row_states
                    .get(row_index)
                    .cloned()
                    .unwrap_or_default();
                let model_id = model.id.clone();
                Self::render_card_button(
                    row_label,
                    mouse_state,
                    AISettingsPageAction::ToggleFetchedModelInModal {
                        model_id,
                        checked: !is_checked,
                    },
                    appearance,
                )
            };

            column = column.with_child(
                Container::new(row_element)
                    .with_margin_bottom(2.)
                    .finish(),
            );
        }

        // Footer: [Select all] [Select none] / [Cancel] [Add N models].
        let select_all_button = Self::render_card_button(
            "Select all",
            self.fetch_modal.select_all_state.clone(),
            AISettingsPageAction::SetAllFetchedModelsChecked { checked: true },
            appearance,
        );
        let select_none_button = Self::render_card_button(
            "Select none",
            self.fetch_modal.select_none_state.clone(),
            AISettingsPageAction::SetAllFetchedModelsChecked { checked: false },
            appearance,
        );
        let cancel_button = Self::render_card_button(
            "Cancel",
            self.fetch_modal.cancel_state.clone(),
            AISettingsPageAction::CancelFetchedAgentProviderModelsModal,
            appearance,
        );
        let commit_label = format!("Add {} models", modal.checked.len());
        let commit_button = Self::render_card_button(
            commit_label,
            self.fetch_modal.commit_state.clone(),
            AISettingsPageAction::CommitFetchedAgentProviderModels {
                provider_index: modal.provider_index,
            },
            appearance,
        );

        let left_footer = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(select_all_button)
            .with_child(
                Container::new(select_none_button)
                    .with_margin_left(8.)
                    .finish(),
            )
            .finish();
        let right_footer = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(cancel_button)
            .with_child(
                Container::new(commit_button)
                    .with_margin_left(8.)
                    .finish(),
            )
            .finish();
        let footer = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(left_footer)
            .with_child(right_footer)
            .finish();
        column = column.with_child(Container::new(footer).with_margin_top(10.).finish());

        Container::new(column.finish())
            .with_background(appearance.theme().surface_1())
            .with_uniform_padding(12.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_margin_bottom(12.)
            .finish()
    }

    fn render_model_row(
        provider_index: usize,
        model_index: usize,
        model: &AgentProviderModel,
        row: &ModelRowHandles,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let cell = |flex: f32, view: &ViewHandle<EditorView>| -> Box<dyn Element> {
            Expanded::new(
                flex,
                Container::new(ChildView::new(view).finish())
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
            )
            .finish()
        };

        // Tool call chip: shows current state
        let tool_label = if model.tool_call {
            "● Tools"
        } else {
            "○ Tools"
        };
        let tool_chip = Self::render_card_button(
            tool_label,
            row.tool_call_chip_state.clone(),
            AISettingsPageAction::ToggleAgentProviderModelToolCall {
                provider_index,
                model_index,
            },
            appearance,
        );

        // Remove button
        let remove_button = Self::render_card_button(
            "\u{00d7}",
            row.remove_button_state.clone(),
            AISettingsPageAction::RemoveAgentProviderModel {
                provider_index,
                model_index,
            },
            appearance,
        );

        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(cell(2., &row.name_editor))
            .with_child(cell(2., &row.id_editor))
            .with_child(cell(1., &row.context_editor))
            .with_child(
                Container::new(tool_chip)
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
            )
            .with_child(remove_button)
            .finish()
    }

    fn render_provider_card(
        &self,
        provider: &AgentProvider,
        provider_index: usize,
        view: &AISettingsPageView,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        let label_color = if is_any_ai_enabled {
            appearance.theme().active_ui_text_color()
        } else {
            appearance.theme().disabled_ui_text_color()
        };

        let card = match self.cards.get(provider_index) {
            Some(card) => card,
            None => {
                // Safety: during rebuild the widget is reconstructed, so indices
                // should always match. Return a placeholder if they don't.
                return Container::new(
                    Text::new(
                        format!("(missing card state for provider {})", provider_index),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(label_color.into())
                    .finish(),
                )
                .with_margin_bottom(8.)
                .finish();
            }
        };

        let name_field = field_block(
            "Name",
            ChildView::new(&card.name_editor).finish(),
            label_color,
            appearance,
        );
        let api_type_field =
            Self::render_api_type_field(provider, provider_index, card, label_color, appearance);
        let base_url_field = field_block(
            "Base URL",
            ChildView::new(&card.base_url_editor).finish(),
            label_color,
            appearance,
        );
        let api_key_field = field_block(
            "API key",
            ChildView::new(&card.api_key_editor).finish(),
            label_color,
            appearance,
        );

        // ---- Models section ----
        let models_label = Container::new(
            Text::new(
                format!("Models ({})", provider.models.len()),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(label_color.into())
            .finish(),
        )
        .with_margin_top(FIELD_LABEL_MARGIN_TOP)
        .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
        .finish();

        let mut models_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(models_label);

        if provider.models.is_empty() {
            let empty_hint = Container::new(
                Text::new(
                    "No models configured. Click \"+ Add Model\" to add one.".to_string(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().disabled_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .with_margin_bottom(MODEL_ROW_GAP)
            .finish();
            models_column.add_child(empty_hint);
        } else {
            // Table header
            let dim = appearance.theme().disabled_ui_text_color();
            let header_cell = |flex: f32, label: &str| -> Box<dyn Element> {
                Expanded::new(
                    flex,
                    Container::new(
                        Text::new(
                            label.to_string(),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(dim.into())
                        .finish(),
                    )
                    .with_margin_right(MODEL_ROW_GAP)
                    .finish(),
                )
                .finish()
            };
            let header = Container::new(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(header_cell(2., "Display Name"))
                    .with_child(header_cell(2., "Model ID"))
                    .with_child(header_cell(1., "Ctx (tokens)"))
                    // Spacer for tools + remove columns
                    .with_child(
                        Text::new(
                            "  ".to_string(),
                            appearance.ui_font_family(),
                            appearance.ui_font_size(),
                        )
                        .with_color(dim.into())
                        .finish(),
                    )
                    .finish(),
            )
            .with_margin_bottom(2.)
            .finish();
            models_column.add_child(header);

            for (model_index, model_row_handles) in card.model_rows.iter().enumerate() {
                let model = match provider.models.get(model_index) {
                    Some(m) => m,
                    None => continue,
                };
                models_column.add_child(
                    Container::new(Self::render_model_row(
                        provider_index,
                        model_index,
                        model,
                        model_row_handles,
                        appearance,
                    ))
                    .with_margin_bottom(MODEL_ROW_GAP)
                    .finish(),
                );
            }
        }

        // ---- Warning banner for incomplete providers ----
        let needs_warning = provider.base_url.is_empty() || provider.models.is_empty();
        let warning_banner = if needs_warning {
            Some(
                Container::new(
                    Text::new(
                        "Configure base URL and at least one model to use this provider."
                            .to_string(),
                        appearance.ui_font_family(),
                        appearance.ui_font_size(),
                    )
                    .with_color(appearance.theme().ui_warning_color())
                    .soft_wrap(true)
                    .finish(),
                )
                .with_margin_top(4.)
                .with_margin_bottom(4.)
                .finish(),
            )
        } else {
            None
        };

        // ---- Bottom button row ----
        let add_model_button = Self::render_card_button(
            "+ Add Model",
            card.add_model_button_state.clone(),
            AISettingsPageAction::AddAgentProviderModel { provider_index },
            appearance,
        );
        // Phase 2: per-provider probe state drives the button label.
        // Reset to Idle when the user edits base_url / api_key / api_type
        // (see the corresponding action handlers in ai_page.rs).
        let probe_label = view
            .agent_provider_probe_states
            .borrow()
            .get(&provider.id)
            .cloned()
            .unwrap_or_default()
            .button_label();
        let test_connection_button = Self::render_card_button(
            probe_label,
            card.test_connection_button_state.clone(),
            AISettingsPageAction::TestAgentProviderConnection { provider_index },
            appearance,
        );
        // Phase 4a. Tri-state label: Fetching… while in-flight, "Failed" if
        // the last attempt errored (re-click retries), else "Fetch models".
        // A truncated failure reason rides along to make the cause readable
        // without a tooltip.
        let fetch_models_label = if view.fetch_models_in_flight.contains(&provider_index) {
            "Fetching…".to_string()
        } else if let Some(reason) = view.last_fetch_failure.get(&provider_index) {
            let excerpt: String = reason
                .trim()
                .chars()
                .take(PROBE_FAILED_LABEL_BUDGET)
                .collect();
            if excerpt.is_empty() {
                "✗ Failed".to_string()
            } else {
                format!("✗ {excerpt}")
            }
        } else {
            "Fetch models".to_string()
        };
        let fetch_models_button = Self::render_card_button(
            fetch_models_label,
            card.fetch_models_button_state.clone(),
            AISettingsPageAction::FetchAgentProviderModels { provider_index },
            appearance,
        );
        let remove_button = Self::render_card_button(
            "Remove",
            card.remove_button_state.clone(),
            AISettingsPageAction::RemoveAgentProvider { provider_index },
            appearance,
        );

        // Group additive actions on the left (Add Model, Test connection,
        // Fetch models), with Remove on the right so the destructive action
        // stays separated.
        let left_buttons = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(add_model_button)
            .with_child(
                Container::new(test_connection_button)
                    .with_margin_left(8.)
                    .finish(),
            )
            .with_child(
                Container::new(fetch_models_button)
                    .with_margin_left(8.)
                    .finish(),
            )
            .finish();

        let bottom_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(left_buttons)
            .with_child(remove_button)
            .finish();

        let mut card_column = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(name_field)
            .with_child(api_type_field)
            .with_child(base_url_field)
            .with_child(api_key_field)
            .with_child(
                Container::new(models_column.finish())
                    .with_margin_top(8.)
                    .finish(),
            );

        if let Some(banner) = warning_banner {
            card_column.add_child(banner);
        }

        card_column.add_child(Container::new(bottom_row).with_margin_top(10.).finish());

        Container::new(card_column.finish())
            .with_background(appearance.theme().surface_1())
            .with_uniform_padding(12.)
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_margin_bottom(8.)
            .finish()
    }
}

impl SettingsWidget for AgentProvidersWidget {
    type View = AISettingsPageView;

    fn search_terms(&self) -> &str {
        "local llm provider custom ollama lm studio vllm openai compatible api key bearer token endpoint agent providers"
    }

    fn should_render(&self, _app: &AppContext) -> bool {
        warp_core::features::FeatureFlag::LocalLlmProvider.is_enabled()
    }

    fn render(
        &self,
        view: &Self::View,
        appearance: &Appearance,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let is_any_ai_enabled = AISettings::as_ref(app).is_any_ai_enabled(app);
        let providers = AISettings::as_ref(app).agent_providers.value().clone();

        let title_color = if is_any_ai_enabled {
            appearance.theme().active_ui_text_color()
        } else {
            appearance.theme().disabled_ui_text_color()
        };

        let title_node =
            build_sub_header(appearance, "Custom AI Providers", Some(title_color)).finish();

        let header_add_button = Self::render_card_button(
            "+ Add Provider",
            self.add_button_state.clone(),
            AISettingsPageAction::AddAgentProvider,
            appearance,
        );

        let header = Container::new(
            Flex::row()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(Expanded::new(1., title_node).finish())
                .with_child(header_add_button)
                .finish(),
        )
        .with_padding_bottom(HEADER_PADDING)
        .finish();

        let description_text =
            "Configure your own OpenAI-compatible LLM endpoints (Ollama, LM Studio, \
             vLLM, llama.cpp, NIM, etc.) so their models appear in the Agent Mode picker. \
             Requests to these providers bypass warp.dev for the LLM call.";
        let description = Container::new(
            Text::new(
                description_text.to_string(),
                appearance.ui_font_family(),
                appearance.ui_font_size(),
            )
            .with_color(if is_any_ai_enabled {
                appearance.theme().foreground().into()
            } else {
                appearance.theme().disabled_ui_text_color().into()
            })
            .soft_wrap(true)
            .finish(),
        )
        .with_margin_bottom(12.)
        .finish();

        let mut column = Flex::column()
            .with_child(render_separator(appearance))
            .with_child(header)
            .with_child(description);

        // Phase 4a. Render the "Fetched models" modal at the top of the
        // widget column when open. Provider cards stay rendered below
        // for context — there's no Stack/Overlay primitive in this
        // setting-page layer, so positional precedence is "above cards"
        // rather than a true z-axis float.
        if let Some(modal) = view.fetched_models_modal.as_ref() {
            column.add_child(self.render_fetched_models_modal(modal, &providers, appearance));
        }

        if providers.is_empty() {
            let empty = Container::new(
                Text::new(
                    "No custom providers configured. Click \"+ Add Provider\" to get started."
                        .to_string(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(appearance.theme().disabled_ui_text_color().into())
                .finish(),
            )
            .with_margin_bottom(12.)
            .finish();
            column.add_child(empty);
        } else {
            for (provider_index, provider) in providers.iter().enumerate() {
                column.add_child(self.render_provider_card(
                    provider,
                    provider_index,
                    view,
                    appearance,
                    app,
                ));
            }
        }

        Container::new(column.finish())
            .with_margin_bottom(HEADER_PADDING)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers (module-private)
// ---------------------------------------------------------------------------

/// Build a single-line editor with AI-settings styling.
fn single_line_editor_options(
    appearance: &Appearance,
    is_password: bool,
) -> SingleLineEditorOptions {
    SingleLineEditorOptions {
        is_password,
        text: TextOptions {
            font_size_override: Some(appearance.ui_font_size()),
            font_family_override: Some(appearance.monospace_font_family()),
            text_colors_override: Some(TextColors {
                default_color: appearance.theme().active_ui_text_color(),
                disabled_color: appearance.theme().disabled_ui_text_color(),
                hint_color: appearance.theme().disabled_ui_text_color(),
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Render a `<label> + <editor element>` vertical block.
fn field_block(
    label: &str,
    editor_element: Box<dyn Element>,
    label_color: warp_core::ui::theme::Fill,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let label_text = Container::new(
        Text::new(
            label.to_string(),
            appearance.ui_font_family(),
            appearance.ui_font_size(),
        )
        .with_color(label_color.into())
        .finish(),
    )
    .with_margin_top(FIELD_LABEL_MARGIN_TOP)
    .with_margin_bottom(FIELD_LABEL_MARGIN_BOTTOM)
    .finish();

    Flex::column()
        .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
        .with_child(label_text)
        .with_child(editor_element)
        .finish()
}

/// Parse a token count from user input. Tolerates `128k`, `128K`, `128,000`,
/// `128_000`, whitespace. Returns 0 for empty or unparseable input.
fn parse_token_count(input: &str) -> u32 {
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',' && *c != '_')
        .collect();
    if cleaned.is_empty() {
        return 0;
    }
    let lower = cleaned.to_lowercase();
    let (num_part, multiplier): (&str, u64) = if let Some(stripped) = lower.strip_suffix('k') {
        (stripped, 1_000)
    } else if let Some(stripped) = lower.strip_suffix('m') {
        (stripped, 1_000_000)
    } else {
        (lower.as_str(), 1)
    };
    num_part
        .parse::<f64>()
        .ok()
        .map(|n| (n * multiplier as f64).round() as u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

/// Display name for an `AgentProviderApiType` variant. Kept here to avoid
/// adding a method to the settings crate (which lives outside
/// `app/src/settings_view/`).
fn api_type_display_name(api_type: AgentProviderApiType) -> &'static str {
    match api_type {
        AgentProviderApiType::OpenAi => "OpenAI",
        AgentProviderApiType::OpenAiResp => "OpenAI Resp",
        AgentProviderApiType::Gemini => "Gemini",
        AgentProviderApiType::Anthropic => "Anthropic",
        AgentProviderApiType::Ollama => "Ollama",
        AgentProviderApiType::DeepSeek => "DeepSeek",
    }
}
