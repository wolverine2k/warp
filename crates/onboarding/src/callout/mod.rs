mod model;
mod view;

pub use model::{FinalState, OnboardingQuery};
pub use view::{
    set_localizer, OnboardingCalloutView, OnboardingCalloutViewEvent, OnboardingKeybindings,
};

pub fn init(app: &mut warpui::AppContext) {
    view::init(app);
}
