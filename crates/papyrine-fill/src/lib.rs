//! Form filling for Papyrine (ROADMAP 1.13; ADR-006, ADR-030, ADR-031).
//!
//! * [`commands`]: undoable commands (`fill.*`) for text, check boxes, radio buttons, combo and
//!   list boxes, reset, appearance regeneration, and flat-form text and marks. Register them
//!   with [`register`].
//! * [`appearance`] and [`font`]: our own appearance-stream generator (single, multi-line, comb,
//!   password, auto size, ZapfDingbats marks, rotation, all border styles) with rustybuzz
//!   shaping for embedded Type0 fonts.
//! * [`engine`]: the event model over the native AF subset: keystroke, validate, calculate in
//!   `/CO` order, format. Scripts the recognizer rejects never run and are reported
//!   ([`form_status`]) for the banner.
//! * [`nav`]: tab order and field navigation.
//! * [`xfa`]: the XFA policy (static: drop the stale packet on first edit; dynamic: read-only).

pub mod api;
pub mod appearance;
pub mod commands;
mod cosx;
pub mod engine;
mod error;
pub mod font;
pub mod form;
mod install;
pub mod nav;
pub mod pipeline;
pub mod xfa;

pub use api::{
    FieldSummary, FormStatus, Preflight, SkippedScript, form_status, keystroke_filter, list_fields,
    preflight_text,
};
pub use commands::{
    AddFlatText, AddMark, MarkKind, RegenerateAppearances, ResetForm, SelectRadio, SetChoice,
    SetTextValue, ToggleCheckbox,
};
pub use error::{FillError, Result};
pub use form::{FieldRef, FormTree, Kind, XfaState};
pub use nav::{TabStop, TabsMode, document_tab_order, next_field, tab_order};

use papyrine_ops::CommandRegistry;

/// Register every `fill.*` command so the journal, macros and replay can rebuild them.
pub fn register(reg: &mut CommandRegistry) {
    use commands::from_json;
    reg.register(SetTextValue::NAME, |_, p| {
        from_json::<SetTextValue>(SetTextValue::NAME, p)
    });
    reg.register(ToggleCheckbox::NAME, |_, p| {
        from_json::<ToggleCheckbox>(ToggleCheckbox::NAME, p)
    });
    reg.register(SelectRadio::NAME, |_, p| {
        from_json::<SelectRadio>(SelectRadio::NAME, p)
    });
    reg.register(SetChoice::NAME, |_, p| {
        from_json::<SetChoice>(SetChoice::NAME, p)
    });
    reg.register(ResetForm::NAME, |_, p| {
        from_json::<ResetForm>(ResetForm::NAME, p)
    });
    reg.register(RegenerateAppearances::NAME, |_, p| {
        from_json::<RegenerateAppearances>(RegenerateAppearances::NAME, p)
    });
    reg.register(AddFlatText::NAME, |_, p| {
        from_json::<AddFlatText>(AddFlatText::NAME, p)
    });
    reg.register(AddMark::NAME, |_, p| from_json::<AddMark>(AddMark::NAME, p));
}
