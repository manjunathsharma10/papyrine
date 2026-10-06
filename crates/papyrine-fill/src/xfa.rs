//! XFA policy (ROADMAP 1.13).
//!
//! * No XFA: nothing to do.
//! * **Dynamic XFA**: the AcroForm is a "please wait" placeholder. Filling it would change
//!   nothing the user sees in an XFA viewer and is meaningless elsewhere, so every filling
//!   command refuses ([`crate::FillError::DynamicXfa`]) and the host shows a read-only notice
//!   ([`crate::FormStatus::dynamic_xfa`]).
//! * **Static XFA with an AcroForm fallback** (the hybrid IRS forms): the AcroForm is the live
//!   form, but XFA-aware viewers (Acrobat) prefer the XFA datasets packet and would silently
//!   show the old values. Updating the packet needs the XFA binding rules (relative and global
//!   binding, repeating subforms) and cannot be verified without Acrobat, and a stale packet is
//!   worse than none. So the first value-changing command removes `/AcroForm /XFA` (and the
//!   catalog's `/NeedsRendering`) as part of the same undoable step: every viewer then shows
//!   the AcroForm we edited. Undo restores the packet byte for byte (it is an ordinary
//!   before-image). Recorded as a proposed ADR in the task report.

use papyrine_cos::Document;
use papyrine_ops::EditContext;

use crate::error::{FillError, Result};
use crate::form::{FormTree, XfaState};

/// Apply the policy before a value-changing command edits the form.
pub fn prepare_for_edit(cx: &mut EditContext<'_>, form: &FormTree) -> Result<()> {
    match form.xfa {
        XfaState::None => Ok(()),
        XfaState::Dynamic => Err(FillError::DynamicXfa),
        XfaState::Static => remove_xfa(cx, form),
    }
}

fn remove_xfa(cx: &mut EditContext<'_>, form: &FormTree) -> Result<()> {
    let Some(acro) = &form.acro else {
        return Ok(());
    };
    cx.remove_key(acro, "XFA")?;
    let root = cx.doc().root()?;
    if root.dict_has("NeedsRendering")? {
        cx.remove_key(&root, "NeedsRendering")?;
    }
    Ok(())
}

/// Does the document carry an XFA packet at all (static or dynamic)?
pub fn has_xfa(doc: &Document) -> bool {
    FormTree::load(doc).is_ok_and(|f| f.xfa != XfaState::None)
}
