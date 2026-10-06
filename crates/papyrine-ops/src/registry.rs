use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::command::{Command, CompositeCommand};
use crate::commands::*;
use crate::error::{Error, Result};

type Ctor = Box<dyn Fn(&CommandRegistry, &Value) -> Result<Box<dyn Command>> + Send + Sync>;

/// Name -> constructor from `params()` JSON, for replay, macros and the journal.
pub struct CommandRegistry {
    ctors: BTreeMap<&'static str, Ctor>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::with_builtin()
    }
}

fn from_json<C: Command + DeserializeOwned + 'static>(
    name: &str,
    p: &Value,
) -> Result<Box<dyn Command>> {
    serde_json::from_value::<C>(p.clone())
        .map(|c| Box::new(c) as Box<dyn Command>)
        .map_err(|e| Error::invalid(format!("{name}: {e}")))
}

impl CommandRegistry {
    pub fn empty() -> Self {
        CommandRegistry {
            ctors: BTreeMap::new(),
        }
    }

    pub fn with_builtin() -> Self {
        let mut r = Self::empty();
        r.register(SetInfoField::NAME, |_, p| {
            from_json::<SetInfoField>(SetInfoField::NAME, p)
        });
        r.register(RotatePages::NAME, |_, p| {
            from_json::<RotatePages>(RotatePages::NAME, p)
        });
        r.register(DeletePages::NAME, |_, p| {
            from_json::<DeletePages>(DeletePages::NAME, p)
        });
        r.register(MovePages::NAME, |_, p| {
            from_json::<MovePages>(MovePages::NAME, p)
        });
        r.register(DuplicatePages::NAME, |_, p| {
            from_json::<DuplicatePages>(DuplicatePages::NAME, p)
        });
        r.register(InsertBlankPage::NAME, |_, p| {
            from_json::<InsertBlankPage>(InsertBlankPage::NAME, p)
        });
        r.register(SetPageBox::NAME, |_, p| {
            from_json::<SetPageBox>(SetPageBox::NAME, p)
        });
        // Needs the host's blob store; hosts call `InsertPagesFromPdf::register` with theirs.
        InsertPagesFromPdf::register(&mut r, std::sync::Arc::new(MemoryBlobs::new()));
        r.register(CompositeCommand::NAME, CompositeCommand::from_params);
        r
    }

    pub fn register(
        &mut self,
        name: &'static str,
        ctor: impl Fn(&CommandRegistry, &Value) -> Result<Box<dyn Command>> + Send + Sync + 'static,
    ) {
        self.ctors.insert(name, Box::new(ctor));
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.ctors.keys().copied()
    }

    pub fn create(&self, name: &str, params: &Value) -> Result<Box<dyn Command>> {
        let ctor = self
            .ctors
            .get(name)
            .ok_or_else(|| Error::UnknownCommand(name.to_owned()))?;
        ctor(self, params)
    }

    /// Rebuild a command from the `{name, params}` form produced by [`describe_command`].
    pub fn create_from_record(&self, record: &Value) -> Result<Box<dyn Command>> {
        let name = record
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid("record has no `name`"))?;
        self.create(name, record.get("params").unwrap_or(&Value::Null))
    }
}

/// `{name, params}` record of a command, the inverse of `create_from_record`.
pub fn describe_command(c: &dyn Command) -> Value {
    serde_json::json!({"name": c.name(), "params": c.params()})
}
