use warpui::assets::asset_cache::AssetSource;
use warpui::elements::{
    Align, CacheOption, ConstrainedBox, Container, CrossAxisAlignment, Element, Flex, Image,
    MainAxisAlignment, MouseStateHandle, ParentElement, Text, Wrap,
};
use warpui::fonts::{Properties, Weight};
use warpui::ui_components::components::UiComponent;
use warpui::{AppContext, Entity, View, ViewContext, ViewHandle};

use super::settings_page::{
    MatchData, PageType, SettingsPageEvent, SettingsPageMeta, SettingsPageViewHandle,
    SettingsWidget,
};
use super::SettingsSection;
use crate::appearance::Appearance;
use crate::channel::ChannelState;
use crate::themes::theme::ColorScheme;
use crate::workspace::WorkspaceAction;

const LOCAL_WARP_PRODUCT_NAME: &str = "Local-Warp";
const ABOUT_VERSION_PLACEHOLDER: &str = "v#.##.###";
const LOCAL_WARP_TAGLINE: &str = "A fork of warp/openwarp supporting Bring Your Own Key (BYOK) and Bring Your Own Provider (BYOP).";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct AboutBranding<'a> {
    product_name: &'static str,
    tagline: &'static str,
    version: &'a str,
    copyright: &'static str,
}

impl<'a> AboutBranding<'a> {
    fn for_version(version: Option<&'a str>) -> Self {
        Self {
            product_name: LOCAL_WARP_PRODUCT_NAME,
            tagline: LOCAL_WARP_TAGLINE,
            version: version.unwrap_or(ABOUT_VERSION_PLACEHOLDER),
            copyright: "Copyright 2026 Local-Warp",
        }
    }
}

pub struct AboutPageView {
    page: PageType<Self>,
}

impl AboutPageView {
    pub fn new(_ctx: &mut ViewContext<AboutPageView>) -> Self {
        AboutPageView {
            page: PageType::new_monolith(AboutPageWidget::default(), None, false),
        }
    }
}

impl Entity for AboutPageView {
    type Event = SettingsPageEvent;
}

impl View for AboutPageView {
    fn ui_name() -> &'static str {
        "AboutPage"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        self.page.render(self, app)
    }
}

#[derive(Default)]
struct AboutPageWidget {
    copy_version_button_mouse_state: MouseStateHandle,
}

impl SettingsWidget for AboutPageWidget {
    type View = AboutPageView;

    fn search_terms(&self) -> &str {
        "about warp version"
    }

    fn render(
        &self,
        _view: &AboutPageView,
        appearance: &Appearance,
        _app: &AppContext,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let ui_builder = appearance.ui_builder();

        let image_path = if theme.inferred_color_scheme() == ColorScheme::LightOnDark {
            "bundled/svg/warp-logo-light.svg"
        } else {
            "bundled/svg/warp-logo-dark.svg"
        };

        let branding = AboutBranding::for_version(ChannelState::app_version());
        let version = branding.version;

        let brand_mark = ConstrainedBox::new(
            Image::new(
                AssetSource::Bundled { path: image_path },
                CacheOption::BySize,
            )
            .finish(),
        )
        .with_max_height(92.)
        .with_max_width(118.)
        .finish();

        let brand_name = Text::new_inline(branding.product_name, appearance.ui_font_family(), 64.)
            .with_color(theme.active_ui_text_color().into())
            .with_style(Properties::default().weight(Weight::Normal))
            .finish();

        let brand_row = Wrap::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_children([
                brand_mark,
                Container::new(brand_name).with_padding_left(24.).finish(),
            ]);

        let tagline_text = ConstrainedBox::new(
            ui_builder
                .span(branding.tagline)
                .with_soft_wrap()
                .build()
                .finish(),
        )
        .with_max_width(620.)
        .finish();

        let version_text = ui_builder
            .span(version.to_string())
            .with_soft_wrap()
            .build()
            .with_margin_top(16.)
            .finish();

        let copy_version_icon = appearance
            .ui_builder()
            .copy_button(16., self.copy_version_button_mouse_state.clone())
            .build()
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(WorkspaceAction::CopyVersion(version));
            })
            .finish();

        let version_row = Wrap::row()
            .with_main_axis_alignment(MainAxisAlignment::Center)
            .with_children([
                version_text,
                Container::new(copy_version_icon)
                    .with_margin_top(16.)
                    .with_padding_left(6.)
                    .finish(),
            ]);

        Align::new(
            Flex::column()
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(brand_row.finish())
                .with_child(Container::new(tagline_text).with_margin_top(16.).finish())
                .with_child(version_row.finish())
                .with_child(
                    ui_builder
                        .span(branding.copyright)
                        .build()
                        .with_margin_top(16.)
                        .finish(),
                )
                .finish(),
        )
        .finish()
    }
}

impl SettingsPageMeta for AboutPageView {
    fn section() -> SettingsSection {
        SettingsSection::About
    }

    fn should_render(&self, _ctx: &AppContext) -> bool {
        true
    }

    fn update_filter(&mut self, query: &str, ctx: &mut ViewContext<Self>) -> MatchData {
        self.page.update_filter(query, ctx)
    }

    fn scroll_to_widget(&mut self, widget_id: &'static str) {
        self.page.scroll_to_widget(widget_id)
    }

    fn clear_highlighted_widget(&mut self) {
        self.page.clear_highlighted_widget();
    }
}

impl From<ViewHandle<AboutPageView>> for SettingsPageViewHandle {
    fn from(view_handle: ViewHandle<AboutPageView>) -> Self {
        SettingsPageViewHandle::About(view_handle)
    }
}

#[cfg(test)]
#[path = "about_page_tests.rs"]
mod tests;
