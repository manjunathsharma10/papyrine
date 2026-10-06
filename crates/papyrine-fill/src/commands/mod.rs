//! The undoable form-filling commands. Each is a `papyrine_ops::Command` rebuilt from its
//! `params()` through the registry ([`crate::register`]).

mod buttons;
mod choice;
mod flat;
mod regenerate;
mod reset;
mod text;

pub use buttons::{SelectRadio, ToggleCheckbox};
pub use choice::SetChoice;
pub use flat::{AddFlatText, AddMark, MarkKind};
pub use regenerate::RegenerateAppearances;
pub use reset::ResetForm;
pub use text::SetTextValue;

use serde::de::DeserializeOwned;
use serde_json::Value;

use papyrine_ops::{Command, Error, Result};

pub(crate) fn from_json<C: Command + DeserializeOwned + 'static>(
    name: &str,
    p: &Value,
) -> Result<Box<dyn Command>> {
    serde_json::from_value::<C>(p.clone())
        .map(|c| Box::new(c) as Box<dyn Command>)
        .map_err(|e| Error::invalid(format!("{name}: {e}")))
}

pub(crate) fn yes() -> bool {
    true
}
