//! The shared commit pipeline every value-changing command runs inside one `EditContext`:
//!
//! 1. XFA policy ([`crate::xfa`]), then the command writes its raw values (`/V`, `/AS`, `/I`...);
//! 2. calculations in `/CO` order (ADR-030: each calculate script once, fields outside `/CO`
//!    untouched, a throwing script leaves its field alone);
//! 3. appearance generation for every field whose value changed, using the format script's
//!    display text and colour;
//! 4. `/NeedAppearances`: when it was set and every text field can be regenerated, regenerate
//!    all of them and clear it; when we generated something we cannot show faithfully (missing
//!    glyphs, unusable font) set it so viewers redo the work.

use std::collections::HashMap;

use papyrine_cos::ObjId;
use papyrine_forms::{Call, DateEnv, TextColor, run_calculation_order};
use papyrine_ops::{EditContext, encode_text_string};

use crate::appearance::{Da, TextReq, button_ap, list_ap, parse_da, text_ap};
use crate::engine::{self, DocForm, FieldScripts, Script, ScriptTable};
use crate::error::{FillError, Result};
use crate::font::{self, FieldFont};
use crate::form::{Field, FieldRef, FormTree, Kind, RawValue, Widget};
use crate::{install, xfa};

pub struct Pipeline<'a, 'd> {
    pub cx: &'a mut EditContext<'d>,
    pub form: FormTree,
    pub scripts: ScriptTable,
    pub env: DateEnv,
    /// Fields whose appearance must be regenerated, in first-touched order.
    touched: Vec<ObjId>,
    lossy: bool,
}

/// What a finished pipeline did, for tests and diagnostics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub regenerated_fields: usize,
    pub calculated_fields: usize,
    pub need_appearances: bool,
}

impl<'a, 'd> Pipeline<'a, 'd> {
    pub fn begin(cx: &'a mut EditContext<'d>) -> Result<Self> {
        Self::begin_with(cx, true)
    }

    /// `edits_values: false` for commands that only regenerate appearances: they leave the XFA
    /// packet alone because no value changes.
    pub fn begin_with(cx: &'a mut EditContext<'d>, edits_values: bool) -> Result<Self> {
        let form = FormTree::load(cx.doc())?;
        if !form.has_form() {
            return Err(FillError::Other("the document has no AcroForm".into()));
        }
        if edits_values {
            xfa::prepare_for_edit(cx, &form)?;
        }
        let scripts = ScriptTable::load(&form);
        Ok(Pipeline {
            cx,
            form,
            scripts,
            env: engine::today(),
            touched: Vec::new(),
            lossy: false,
        })
    }

    pub fn field(&self, r: &FieldRef) -> Result<Field> {
        let f = self.form.resolve(r)?;
        if f.is_read_only() {
            return Err(FillError::ReadOnly(f.name.clone()));
        }
        Ok(f.clone())
    }

    pub fn form_values(&self) -> DocForm {
        DocForm::from_tree(&self.form)
    }

    pub fn scripts_of(&self, f: &Field) -> FieldScripts {
        self.scripts
            .get(f.id)
            .cloned()
            .unwrap_or_else(FieldScripts::none)
    }

    /// Write `/V` of `field` (and drop a stale rich-text value) and queue its appearance.
    pub fn write_value(&mut self, field: &Field, v: &RawValue) -> Result<()> {
        self.cx.touch(&field.obj)?;
        let doc = self.cx.doc();
        match v {
            RawValue::None => field.obj.dict_remove("V")?,
            RawValue::Text(s) => field
                .obj
                .dict_set("V", &doc.new_string(encode_text_string(s))?)?,
            RawValue::Name(n) => field.obj.dict_set("V", &doc.new_name(n)?)?,
            RawValue::List(l) => {
                let a = doc.new_array();
                for s in l {
                    a.array_push(&doc.new_string(encode_text_string(s))?)?;
                }
                field.obj.dict_set("V", &a)?;
            }
        }
        if field.kind == Kind::Text && field.obj.dict_has("RV")? {
            field.obj.dict_remove("RV")?;
        }
        self.queue(field.id);
        Ok(())
    }

    pub fn queue(&mut self, id: ObjId) {
        if !self.touched.contains(&id) {
            self.touched.push(id);
        }
    }

    /// Run calculations, regenerate appearances and settle `/NeedAppearances`.
    pub fn finish(mut self) -> Result<Summary> {
        let mut summary = Summary::default();
        // The tree read at `begin` predates the writes.
        self.form = FormTree::load(self.cx.doc())?;
        summary.calculated_fields = self.recalculate()?;
        self.form = FormTree::load(self.cx.doc())?;

        let was_set = self.form.need_appearances;
        let mut ids: Vec<ObjId> = std::mem::take(&mut self.touched);
        let regenerate_all = was_set && self.all_regenerable();
        if regenerate_all {
            for f in &self.form.fields {
                if f.shows_text() && !f.widgets.is_empty() && !ids.contains(&f.id) {
                    ids.push(f.id);
                }
            }
        }
        let lookup = self.form_values();
        for id in &ids {
            let Some(field) = self.form.by_id(*id).cloned() else {
                continue;
            };
            self.render_field(&field, &lookup)?;
            summary.regenerated_fields += 1;
        }

        let want = if regenerate_all {
            self.lossy
        } else {
            was_set || self.lossy
        };
        if want != was_set
            && let Some(acro) = self.form.acro.clone()
        {
            self.cx
                .set_key(&acro, "NeedAppearances", &self.cx.doc().new_bool(want))?;
        }
        summary.need_appearances = want;
        Ok(summary)
    }

    /// Calculate in `/CO` order; returns how many fields changed.
    fn recalculate(&mut self) -> Result<usize> {
        if self.form.calc_order.is_empty() {
            return Ok(0);
        }
        let mut order = Vec::new();
        let mut calls: HashMap<String, Vec<Call>> = HashMap::new();
        for id in &self.form.calc_order {
            let Some(f) = self.form.by_id(*id) else {
                continue;
            };
            order.push(f.name.clone());
            if let Some(FieldScripts {
                calculate: Script::Native(c),
                ..
            }) = self.scripts.get(f.id)
            {
                calls.insert(f.name.clone(), c.clone());
            }
        }
        if calls.is_empty() {
            return Ok(0);
        }
        let mut df = DocForm::from_tree(&self.form);
        let changed = run_calculation_order(&order, &calls, &mut df, &self.env);
        let mut n = 0;
        for name in changed {
            let Some(f) = self.form.by_name(&name).cloned() else {
                continue;
            };
            if !matches!(f.kind, Kind::Text | Kind::Combo) {
                continue;
            }
            let v = papyrine_forms::FormValues::get(&df, &name).unwrap_or_default();
            self.write_value(&f, &RawValue::Text(v))?;
            n += 1;
        }
        Ok(n)
    }

    fn all_regenerable(&self) -> bool {
        let lookup = self.form_values();
        self.form.fields.iter().all(|f| {
            if !f.shows_text() || f.widgets.is_empty() {
                return true;
            }
            let text = self.display_text(f, &lookup).0;
            if text.is_empty() {
                return true;
            }
            f.widgets.iter().all(|w| {
                self.resolve_font(f, w)
                    .is_ok_and(|font| font.can_show(&text))
            })
        })
    }

    /// The text a widget of `field` shows, plus the format script's colour.
    pub fn display_text(&self, f: &Field, lookup: &DocForm) -> (String, Option<TextColor>) {
        let raw = match (&f.kind, &f.value) {
            (Kind::List, _) => String::new(),
            (Kind::Combo, v) if !f.editable_combo() => {
                let exp = v.as_text().unwrap_or("");
                f.options
                    .iter()
                    .find(|(e, _)| e == exp)
                    .map_or_else(|| exp.to_string(), |(_, d)| d.clone())
            }
            (_, v) => v.as_text().unwrap_or("").to_string(),
        };
        let scripts = self.scripts_of(f);
        let fm = engine::format_value(f, &scripts, &raw, lookup, &self.env);
        (fm.display, fm.color)
    }

    pub fn resolve_font(&self, f: &Field, w: &Widget) -> Result<FieldFont> {
        let da = parse_da(f.da.as_deref());
        font::resolve(&da.font_name, Some(&w.obj), self.form.dr.as_ref())
    }

    /// Resolve with a fallback: an unusable document font becomes Helvetica and the result is
    /// flagged so `/NeedAppearances` gets set.
    fn font_or_fallback(&mut self, w: &Widget, da: &Da) -> FieldFont {
        match font::resolve(&da.font_name, Some(&w.obj), self.form.dr.as_ref()) {
            Ok(font) => font,
            Err(_) => {
                self.lossy = true;
                FieldFont::synthetic_helvetica("PapyrineHelv")
            }
        }
    }

    pub fn render_field(&mut self, field: &Field, lookup: &DocForm) -> Result<()> {
        match field.kind {
            Kind::Text | Kind::Combo => {
                let (display, color) = self.display_text(field, lookup);
                let da = parse_da(field.da.as_deref());
                let over = color.map(|c| match c {
                    TextColor::Red => [1.0, 0.0, 0.0],
                    TextColor::Black => [0.0, 0.0, 0.0],
                });
                for w in &field.widgets {
                    let font = self.font_or_fallback(w, &da);
                    let ap = text_ap(&TextReq {
                        field,
                        widget: w,
                        text: &display,
                        font: &font,
                        da: &da,
                        color_override: over,
                    })?;
                    self.lossy |= ap.lossy;
                    install::install(self.cx, self.form.acro.as_ref(), w, None, &ap)?;
                }
            }
            Kind::List => {
                let da = parse_da(field.da.as_deref());
                let selected = selected_indices(field);
                for w in &field.widgets {
                    let font = self.font_or_fallback(w, &da);
                    let ap = list_ap(field, w, &selected, &font, &da)?;
                    self.lossy |= ap.lossy;
                    install::install(self.cx, self.form.acro.as_ref(), w, None, &ap)?;
                }
            }
            Kind::Checkbox | Kind::Radio => {
                for (i, w) in field.widgets.iter().enumerate() {
                    self.ensure_button_aps(field, i, w)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Make sure a checkbox/radio widget has both an on and an `Off` appearance stream.
    pub fn ensure_button_aps(&mut self, field: &Field, idx: usize, w: &Widget) -> Result<()> {
        let da = parse_da(field.da.as_deref());
        let on = on_state_name(field, idx);
        let has = |s: &str| w.ap_states.iter().any(|k| k == s);
        if !has(&on) {
            let ap = button_ap(field, w, true, &da);
            install::install(self.cx, self.form.acro.as_ref(), w, Some(&on), &ap)?;
        }
        if !has("Off") {
            let ap = button_ap(field, w, false, &da);
            install::install(self.cx, self.form.acro.as_ref(), w, Some("Off"), &ap)?;
        }
        Ok(())
    }

    pub fn set_as(&mut self, w: &Widget, state: &str) -> Result<()> {
        if w.appearance_state.as_deref() == Some(state) {
            return Ok(());
        }
        self.cx
            .set_key(&w.obj, "AS", &self.cx.doc().new_name(state)?)?;
        Ok(())
    }
}

/// Indices of selected options of a list box (from `/I`, else matched through `/V`).
pub fn selected_indices(f: &Field) -> Vec<usize> {
    let vals: Vec<&str> = match &f.value {
        RawValue::Text(s) | RawValue::Name(s) => vec![s.as_str()],
        RawValue::List(l) => l.iter().map(String::as_str).collect(),
        RawValue::None => Vec::new(),
    };
    let mut out: Vec<usize> = Vec::new();
    for v in &vals {
        if let Some(i) = f.options.iter().position(|(e, _)| e == v) {
            out.push(i);
        }
    }
    if out.is_empty() && !f.selected_indices.is_empty() && vals.is_empty() {
        out = f.selected_indices.iter().map(|&i| i as usize).collect();
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The name of widget `idx`'s on state: what its appearance already uses, else the option
/// export value (radios), else `Yes`.
pub fn on_state_name(f: &Field, idx: usize) -> String {
    if let Some(s) = f.widgets.get(idx).and_then(|w| w.on_state.clone()) {
        return s;
    }
    match f.kind {
        Kind::Radio => f
            .options
            .get(idx)
            .map(|(e, _)| e.clone())
            .filter(|e| !e.is_empty() && e != "Off")
            .unwrap_or_else(|| idx.to_string()),
        _ => "Yes".to_string(),
    }
}
