use papyrine_ops::{ChangeSet, Command, EditContext, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::pipeline::Pipeline;

/// Regenerate the appearance of every text, combo and list widget and clear
/// `/NeedAppearances` when that is faithful. Hosts run it when they open a form that sets the
/// flag, so the file carries real appearances (PDFium, Poppler and pdf.js then show what Acrobat
/// would). If some text cannot be shown with its font the flag stays (or becomes) set.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegenerateAppearances {}

impl RegenerateAppearances {
    pub const NAME: &'static str = "fill.regenerate_appearances";
}

impl Command for RegenerateAppearances {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.regenerate-appearances")
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin_with(cx, false)?;
        let ids: Vec<_> = p
            .form
            .fields
            .iter()
            .filter(|f| f.shows_text() && !f.widgets.is_empty())
            .map(|f| f.id)
            .collect();
        for id in ids {
            p.queue(id);
        }
        p.finish()?;
        cx.changeset()
    }
}
