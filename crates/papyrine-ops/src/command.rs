use serde_json::{Value, json};

use crate::changeset::ChangeSet;
use crate::context::EditContext;
use crate::error::Result;
use crate::registry::CommandRegistry;
use crate::text::LocalizedText;

/// One undoable edit. Commands hold only their parameters; undo and redo restore recorded
/// images, they never re-run `apply`.
pub trait Command: Send {
    /// Stable identifier, the key in the [`CommandRegistry`].
    fn name(&self) -> &'static str;
    fn describe(&self) -> LocalizedText;
    /// Everything needed to rebuild this command through the registry (replay, macros).
    fn params(&self) -> Value;
    /// Mutate the document through `cx` and return `cx.changeset()`.
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet>;
    /// Default: restore the before-images.
    fn revert(&mut self, cx: &mut EditContext<'_>, cs: &ChangeSet) -> Result<()> {
        cx.restore_before(cs)
    }
    /// Redo. Default: restore the after-images.
    fn reapply(&mut self, cx: &mut EditContext<'_>, cs: &ChangeSet) -> Result<()> {
        cx.restore_after(cs)
    }
}

/// Several commands as one undo step. They run against one `EditContext`, so the combined
/// change set has one before-image per object (the state before the whole composite).
pub struct CompositeCommand {
    label: Option<String>,
    commands: Vec<Box<dyn Command>>,
}

impl CompositeCommand {
    pub const NAME: &'static str = "composite";

    pub fn new(commands: Vec<Box<dyn Command>>) -> Self {
        CompositeCommand {
            label: None,
            commands,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub(crate) fn from_params(reg: &CommandRegistry, p: &Value) -> Result<Box<dyn Command>> {
        let mut cmds = Vec::new();
        for c in p
            .get("commands")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = c.get("name").and_then(Value::as_str).unwrap_or_default();
            cmds.push(reg.create(name, c.get("params").unwrap_or(&Value::Null))?);
        }
        let mut cc = CompositeCommand::new(cmds);
        cc.label = p.get("label").and_then(Value::as_str).map(str::to_owned);
        Ok(Box::new(cc))
    }
}

impl Command for CompositeCommand {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        let mut t = LocalizedText::new("cmd.composite").arg("count", self.commands.len());
        if let Some(l) = &self.label {
            t = t.arg("label", l);
        }
        t
    }

    fn params(&self) -> Value {
        json!({
            "label": self.label,
            "commands": self.commands.iter()
                .map(|c| json!({"name": c.name(), "params": c.params()}))
                .collect::<Vec<_>>(),
        })
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        for c in &mut self.commands {
            c.apply(cx)?;
        }
        cx.changeset()
    }
}
