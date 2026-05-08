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
use super::settings_page::{HEADER_PADDING, SettingsWidget, build_sub_header, render_separator};

const CARD_BUTTON_FONT_SIZE: f32 = 12.0;
const CARD_BUTTON_PADDING: f32 = 6.0;
const FIELD_LABEL_MARGIN_TOP: f32 = 6.0;
const FIELD_LABEL_MARGIN_BOTTOM: f32 = 2.0;
const MODEL_ROW_GAP: f32 = 6.0;

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
            let options = single_line_editor_options(&appearance, false);
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
                ctx.dispatch_typed_action(AISettingsPageAction::UpdateAgentProviderName {
                    provider_index,
                    name: buffer_text,
                });
            }
        });

        // Base URL editor
        let initial_base_url = provider.base_url.clone();
        let base_url_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(&appearance, false);
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
                ctx.dispatch_typed_action(AISettingsPageAction::UpdateAgentProviderBaseUrl {
                    provider_index,
                    base_url: buffer_text,
                });
            }
        });

        // API key editor (password mode)
        let initial_api_key: String = ::ai::local_provider::AgentProviderSecrets::as_ref(ctx)
            .get(&provider.id)
            .map(str::to_owned)
            .unwrap_or_default();
        let api_key_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(&appearance, true);
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
                ctx.dispatch_typed_action(AISettingsPageAction::UpdateAgentProviderApiKey {
                    provider_index,
                    api_key: buffer_text,
                });
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
            let options = single_line_editor_options(&appearance, false);
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
                ctx.dispatch_typed_action(AISettingsPageAction::UpdateAgentProviderModelName {
                    provider_index,
                    model_index,
                    name: buffer_text,
                });
            }
        });

        // ID editor
        let initial_id = model.id.clone();
        let id_editor = ctx.add_typed_action_view(move |ctx| {
            let appearance = Appearance::handle(ctx).as_ref(ctx);
            let options = single_line_editor_options(&appearance, false);
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
                ctx.dispatch_typed_action(AISettingsPageAction::UpdateAgentProviderModelId {
                    provider_index,
                    model_index,
                    id: buffer_text,
                });
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
            let options = single_line_editor_options(&appearance, false);
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
                ctx.dispatch_typed_action(
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
                    .with_color(appearance.theme().warning().into())
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
        let remove_button = Self::render_card_button(
            "Remove",
            card.remove_button_state.clone(),
            AISettingsPageAction::RemoveAgentProvider { provider_index },
            appearance,
        );

        let bottom_row = Flex::row()
            .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(add_model_button)
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
        _view: &Self::View,
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

        let description_text = "Configure your own OpenAI-compatible LLM endpoints (Ollama, LM Studio, \
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
