//! The annotation commands.

use std::collections::BTreeSet;

use papyrine_cos::{ObjId, Object, ObjectKind};
use papyrine_ops::{
    ChangeSet, Command, CommandRegistry, EditContext, Error, LocalizedText, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appearance::{self, Appearance, NOTE_SIZE};
use crate::erase::erase_strokes;
use crate::geometry::{Geometry, MarkupKind, Pt, Quad, TextStyle, normalize_rect};
use crate::objects::{Fixed, fill_annotation, find_in_page, read_spec, write_appearance};
use crate::props::{
    AnnotProps, Color, PropKey, PropsPatch, flags, new_annotation_name, pdf_date_now,
};
use crate::spec::Spec;
use crate::text::FontReport;

/// How a command names an existing annotation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnnotRef {
    /// Object number and generation (stable across undo and redo).
    Id { num: u32, generation: u16 },
    /// The unique `/NM` name.
    Name(String),
}

impl AnnotRef {
    pub fn id(id: ObjId) -> AnnotRef {
        AnnotRef::Id {
            num: id.num,
            generation: id.generation,
        }
    }
    pub fn named(n: impl Into<String>) -> AnnotRef {
        AnnotRef::Name(n.into())
    }
}

fn from_json<C: Command + serde::de::DeserializeOwned + 'static>(
    name: &str,
    p: &Value,
) -> Result<Box<dyn Command>> {
    serde_json::from_value::<C>(p.clone())
        .map(|c| Box::new(c) as Box<dyn Command>)
        .map_err(|e| Error::invalid(format!("{name}: {e}")))
}

/// Register every annotation command.
pub fn register(reg: &mut CommandRegistry) {
    reg.register(AddAnnotation::NAME, |_, p| {
        from_json::<AddAnnotation>(AddAnnotation::NAME, p)
    });
    reg.register(UpdateAnnotation::NAME, |_, p| {
        from_json::<UpdateAnnotation>(UpdateAnnotation::NAME, p)
    });
    reg.register(DeleteAnnotations::NAME, |_, p| {
        from_json::<DeleteAnnotations>(DeleteAnnotations::NAME, p)
    });
    reg.register(EraseInk::NAME, |_, p| {
        from_json::<EraseInk>(EraseInk::NAME, p)
    });
    reg.register(AddReply::NAME, |_, p| {
        from_json::<AddReply>(AddReply::NAME, p)
    });
}

// --- shared helpers ------------------------------------------------------------------------

fn page_at(cx: &EditContext<'_>, page: usize) -> Result<Object> {
    let n = cx.doc().page_count()?;
    if page >= n {
        return Err(Error::invalid(format!(
            "page {page} out of range (document has {n})"
        )));
    }
    Ok(cx.doc().page(page)?)
}

/// The object a mutation of `/Annots` (or of a direct annotation inside it) is recorded on.
fn annots_holder(page: &Object) -> Result<Object> {
    let a = page.dict_get("Annots")?;
    Ok(if a.kind()? == ObjectKind::Array && a.is_indirect() {
        a
    } else {
        page.clone()
    })
}

fn append_annot(cx: &mut EditContext<'_>, page: &Object, annot: &Object) -> Result<()> {
    let doc = cx.doc();
    let cur = page.dict_get("Annots")?;
    if cur.kind()? == ObjectKind::Array {
        cx.touch(&annots_holder(page)?)?;
        cur.array_push(annot)?;
    } else {
        cx.touch(page)?;
        let arr = doc.new_array();
        arr.array_push(annot)?;
        page.dict_set("Annots", &arr)?;
    }
    Ok(())
}

fn ref_id(o: &Object) -> Option<ObjId> {
    o.id()
}

/// Remove the annotations at `indices` (and, transitively, their replies and popups).
/// Returns how many entries were removed.
fn remove_annotations(cx: &mut EditContext<'_>, page: &Object, seed: &[usize]) -> Result<usize> {
    let arr = page.dict_get("Annots")?;
    let items = arr.array_items()?;
    let mut set: BTreeSet<usize> = seed.iter().copied().collect();
    loop {
        let ids: BTreeSet<ObjId> = set.iter().filter_map(|&i| ref_id(&items[i])).collect();
        let mut grew = false;
        for (i, a) in items.iter().enumerate() {
            if set.contains(&i) {
                // Its popup goes with it.
                if let Some(p) = a.dict_get("Popup").ok().and_then(|p| ref_id(&p)) {
                    for (j, b) in items.iter().enumerate() {
                        if ref_id(b) == Some(p) && set.insert(j) {
                            grew = true;
                        }
                    }
                }
                continue;
            }
            let reply_to = a.dict_get("IRT").ok().and_then(|p| ref_id(&p));
            let parent = a.dict_get("Parent").ok().and_then(|p| ref_id(&p));
            let is_popup = a
                .dict_get("Subtype")
                .ok()
                .and_then(|s| s.name().ok())
                .is_some_and(|n| n == b"Popup");
            if reply_to.is_some_and(|r| ids.contains(&r))
                || (is_popup && parent.is_some_and(|r| ids.contains(&r)))
            {
                set.insert(i);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    cx.touch(&annots_holder(page)?)?;
    for &i in set.iter().rev() {
        arr.array_remove(i)?;
    }
    Ok(set.len())
}

fn subtype_of(a: &Object) -> String {
    a.dict_get("Subtype")
        .ok()
        .and_then(|s| s.name().ok())
        .map(|n| String::from_utf8_lossy(&n).into_owned())
        .unwrap_or_default()
}

fn touch_annotation(cx: &mut EditContext<'_>, page: &Object, annot: &Object) -> Result<()> {
    if annot.is_indirect() {
        cx.touch(annot)
    } else {
        cx.touch(&annots_holder(page)?)
    }
}

/// Regenerate dictionary entries and appearance of an existing annotation from `spec`.
fn rewrite(
    cx: &mut EditContext<'_>,
    page: &Object,
    annot: &Object,
    spec: &Spec,
) -> Result<FontReport> {
    let app = appearance::build(spec)?;
    touch_annotation(cx, page, annot)?;
    fill_annotation(
        cx.doc(),
        annot,
        spec,
        &app,
        &Fixed {
            page: None,
            irt: None,
        },
    )?;
    write_appearance(cx, annot, &app)?;
    Ok(app.report)
}

fn is_reply(a: &Object) -> bool {
    a.dict_has("IRT").unwrap_or(false)
}

// --- add -----------------------------------------------------------------------------------

/// Create one annotation with an appearance stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddAnnotation {
    pub page: usize,
    pub geometry: Geometry,
    #[serde(default)]
    pub props: AnnotProps,
    #[serde(skip)]
    report: Option<FontReport>,
}

impl AddAnnotation {
    pub const NAME: &'static str = "annot_add";

    /// Fills in the creation date, modification date and unique name now, so the command's
    /// `params()` replay to the same annotation.
    pub fn new(page: usize, geometry: Geometry, mut props: AnnotProps) -> Self {
        let now = pdf_date_now();
        props.created.get_or_insert_with(|| now.clone());
        props.modified.get_or_insert(now);
        props.name.get_or_insert_with(new_annotation_name);
        if geometry.is_arrow() && props.fill.is_none() {
            props.fill = Some(
                props
                    .color
                    .clone()
                    .unwrap_or_else(|| crate::spec::default_color(&geometry)),
            );
        }
        AddAnnotation {
            page,
            geometry,
            props,
            report: None,
        }
    }

    pub fn highlight(page: usize, quads: Vec<Quad>, props: AnnotProps) -> Self {
        Self::new(page, Geometry::highlight(quads), props)
    }
    pub fn underline(page: usize, quads: Vec<Quad>, props: AnnotProps) -> Self {
        Self::new(page, Geometry::underline(quads), props)
    }
    pub fn strike_out(page: usize, quads: Vec<Quad>, props: AnnotProps) -> Self {
        Self::new(page, Geometry::strike_out(quads), props)
    }
    pub fn squiggly(page: usize, quads: Vec<Quad>, props: AnnotProps) -> Self {
        Self::new(page, Geometry::squiggly(quads), props)
    }
    pub fn sticky_note(page: usize, x: f64, y: f64, props: AnnotProps) -> Self {
        Self::new(page, Geometry::note(x, y), props)
    }
    pub fn text_box(
        page: usize,
        rect: [f64; 4],
        style: TextStyle,
        text: &str,
        mut props: AnnotProps,
    ) -> Self {
        props.contents = Some(text.to_owned());
        Self::new(page, Geometry::text_box(rect, style), props)
    }
    pub fn pen(page: usize, strokes: Vec<Vec<Pt>>, props: AnnotProps) -> Self {
        Self::new(page, Geometry::Ink { strokes }, props)
    }
    pub fn rectangle(page: usize, rect: [f64; 4], props: AnnotProps) -> Self {
        Self::new(page, Geometry::Square { rect }, props)
    }
    pub fn oval(page: usize, rect: [f64; 4], props: AnnotProps) -> Self {
        Self::new(page, Geometry::Circle { rect }, props)
    }
    pub fn line(page: usize, from: Pt, to: Pt, props: AnnotProps) -> Self {
        Self::new(page, Geometry::line(from, to), props)
    }
    pub fn arrow(page: usize, from: Pt, to: Pt, props: AnnotProps) -> Self {
        Self::new(page, Geometry::arrow(from, to), props)
    }

    /// Fonts used for the text box, set by `apply`: which system fonts were embedded and which
    /// characters could not be drawn.
    pub fn font_report(&self) -> Option<&FontReport> {
        self.report.as_ref()
    }
}

impl Command for AddAnnotation {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.annot-add")
            .arg("type", self.geometry.subtype())
            .arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let page = page_at(cx, self.page)?;
        let spec = Spec::new(self.geometry.clone(), self.props.clone());
        let app = appearance::build(&spec)?;
        let doc = cx.doc();
        // The annotation dictionary is created first so it is `created[0]` in the change set.
        let annot = doc.make_indirect(&doc.new_dict())?;
        fill_annotation(
            doc,
            &annot,
            &spec,
            &app,
            &Fixed {
                page: Some(&page),
                irt: None,
            },
        )?;
        write_appearance(cx, &annot, &app)?;
        append_annot(cx, &page, &annot)?;
        cx.note_page(self.page);
        self.report = Some(app.report);
        cx.changeset()
    }
}

// --- update --------------------------------------------------------------------------------

/// Change properties and/or geometry of an annotation; the appearance is regenerated.
///
/// Annotation types Papyrine does not draw (stamps, links, widgets, ...) and replies accept
/// metadata-only changes (contents, author, subject, flags, modification date).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateAnnotation {
    pub page: usize,
    pub target: AnnotRef,
    #[serde(default)]
    pub patch: PropsPatch,
    #[serde(default)]
    pub geometry: Option<Geometry>,
}

impl UpdateAnnotation {
    pub const NAME: &'static str = "annot_update";

    pub fn new(page: usize, target: AnnotRef, mut patch: PropsPatch) -> Self {
        patch.set.modified.get_or_insert_with(pdf_date_now);
        UpdateAnnotation {
            page,
            target,
            patch,
            geometry: None,
        }
    }

    pub fn with_geometry(mut self, g: Geometry) -> Self {
        self.geometry = Some(g);
        self
    }
}

impl Command for UpdateAnnotation {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.annot-update").arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let page = page_at(cx, self.page)?;
        let (_, annot) = find_in_page(&page, &self.target)?;
        self.patch.set.validate()?;
        match read_spec(&annot) {
            Some(cur) if !is_reply(&annot) => {
                let mut props = cur.props.clone();
                self.patch.apply_to(&mut props);
                let geometry = match &self.geometry {
                    Some(g) => {
                        if g.subtype() != cur.geometry.subtype() {
                            return Err(Error::invalid(format!(
                                "cannot change a {} into a {}",
                                cur.geometry.subtype(),
                                g.subtype()
                            )));
                        }
                        g.clone()
                    }
                    None => cur.geometry.clone(),
                };
                let spec = Spec::new(geometry, props);
                rewrite(cx, &page, &annot, &spec)?;
            }
            _ => {
                if self.geometry.is_some() {
                    return Err(Error::invalid(
                        "this annotation's geometry cannot be edited here",
                    ));
                }
                metadata_only(cx, &page, &annot, &self.patch)?;
            }
        }
        cx.note_page(self.page);
        cx.changeset()
    }
}

fn metadata_only(
    cx: &mut EditContext<'_>,
    page: &Object,
    annot: &Object,
    patch: &PropsPatch,
) -> Result<()> {
    let s = &patch.set;
    if s.color.is_some()
        || s.opacity.is_some()
        || s.width.is_some()
        || s.fill.is_some()
        || !s.dash.is_empty()
    {
        return Err(Error::invalid(
            "only contents, author, subject, flags and date can be changed on this annotation",
        ));
    }
    let doc = cx.doc();
    touch_annotation(cx, page, annot)?;
    let text =
        |t: &str| -> Result<Object> { Ok(doc.new_string(papyrine_ops::encode_text_string(t))?) };
    if let Some(v) = &s.contents {
        annot.dict_set("Contents", &text(v)?)?;
    }
    if let Some(v) = &s.author {
        annot.dict_set("T", &text(v)?)?;
    }
    if let Some(v) = &s.subject {
        annot.dict_set("Subj", &text(v)?)?;
    }
    if let Some(v) = s.flags {
        annot.dict_set("F", &doc.new_int(i64::from(v)))?;
    }
    if let Some(v) = &s.modified {
        annot.dict_set("M", &doc.new_string(v.as_bytes())?)?;
    }
    for k in &patch.clear {
        match k {
            PropKey::Contents => annot.dict_remove("Contents")?,
            PropKey::Subject => annot.dict_remove("Subj")?,
            PropKey::Author => annot.dict_remove("T")?,
            PropKey::Fill | PropKey::Dash => {}
        }
    }
    Ok(())
}

// --- delete --------------------------------------------------------------------------------

/// Delete annotations together with their replies and popups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteAnnotations {
    pub page: usize,
    pub targets: Vec<AnnotRef>,
}

impl DeleteAnnotations {
    pub const NAME: &'static str = "annot_delete";

    pub fn new(page: usize, targets: Vec<AnnotRef>) -> Self {
        DeleteAnnotations { page, targets }
    }
}

impl Command for DeleteAnnotations {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.annot-delete")
            .arg("count", self.targets.len())
            .arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        if self.targets.is_empty() {
            return Err(Error::invalid("nothing to delete"));
        }
        let page = page_at(cx, self.page)?;
        let mut seed = Vec::new();
        for t in &self.targets {
            let (i, a) = find_in_page(&page, t)?;
            match subtype_of(&a).as_str() {
                "Widget" => {
                    return Err(Error::invalid(
                        "form fields are removed with the form tools, not as annotations",
                    ));
                }
                "Popup" => {
                    return Err(Error::invalid("delete the annotation a popup belongs to"));
                }
                _ => seed.push(i),
            }
        }
        remove_annotations(cx, &page, &seed)?;
        cx.note_page(self.page);
        cx.changeset()
    }
}

// --- eraser --------------------------------------------------------------------------------

/// The eraser tool on one ink annotation: erase `radius` points around the eraser path. An
/// annotation with nothing left is deleted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EraseInk {
    pub page: usize,
    pub target: AnnotRef,
    pub path: Vec<Pt>,
    pub radius: f64,
    #[serde(default)]
    pub modified: Option<String>,
}

impl EraseInk {
    pub const NAME: &'static str = "annot_erase_ink";

    pub fn new(page: usize, target: AnnotRef, path: Vec<Pt>, radius: f64) -> Self {
        EraseInk {
            page,
            target,
            path,
            radius,
            modified: Some(pdf_date_now()),
        }
    }
}

impl Command for EraseInk {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.annot-erase-ink").arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        if self.path.is_empty() || !(self.radius.is_finite() && self.radius > 0.0) {
            return Err(Error::invalid(
                "the eraser needs a path and a positive radius",
            ));
        }
        if self
            .path
            .iter()
            .any(|p| !p[0].is_finite() || !p[1].is_finite())
        {
            return Err(Error::invalid("eraser coordinates must be finite"));
        }
        let page = page_at(cx, self.page)?;
        let (idx, annot) = find_in_page(&page, &self.target)?;
        let spec = read_spec(&annot)
            .filter(|s| matches!(s.geometry, Geometry::Ink { .. }))
            .ok_or_else(|| Error::invalid("the eraser works on ink annotations"))?;
        let Geometry::Ink { strokes } = &spec.geometry else {
            unreachable!("filtered above");
        };
        let left = erase_strokes(strokes, &self.path, self.radius);
        if left == *strokes {
            return cx.changeset(); // nothing under the eraser
        }
        if left.is_empty() {
            remove_annotations(cx, &page, &[idx])?;
        } else {
            let mut props = spec.props.clone();
            if let Some(m) = &self.modified {
                props.modified = Some(m.clone());
            }
            rewrite(
                cx,
                &page,
                &annot,
                &Spec::new(Geometry::Ink { strokes: left }, props),
            )?;
        }
        cx.note_page(self.page);
        cx.changeset()
    }
}

// --- reply ---------------------------------------------------------------------------------

/// Add a reply (`/IRT` + `/RT /R`) to an annotation. Replies are listed in the comments pane
/// and have an empty appearance, so they do not draw a second icon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddReply {
    pub page: usize,
    pub parent: AnnotRef,
    #[serde(default)]
    pub props: AnnotProps,
}

impl AddReply {
    pub const NAME: &'static str = "annot_reply";

    pub fn new(page: usize, parent: AnnotRef, text: &str, mut props: AnnotProps) -> Self {
        let now = pdf_date_now();
        props.contents = Some(text.to_owned());
        props.created.get_or_insert_with(|| now.clone());
        props.modified.get_or_insert(now);
        props.name.get_or_insert_with(new_annotation_name);
        AddReply {
            page,
            parent,
            props,
        }
    }
}

impl Command for AddReply {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.annot-reply").arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        self.props.validate()?;
        let page = page_at(cx, self.page)?;
        let (_, parent) = find_in_page(&page, &self.parent)?;
        if !parent.is_indirect() {
            return Err(Error::invalid(
                "only an indirect annotation can be replied to",
            ));
        }
        if subtype_of(&parent) == "Popup" {
            return Err(Error::invalid("reply to the annotation, not its popup"));
        }
        let r = crate::objects::read_rect(&parent).unwrap_or([0.0, 0.0, NOTE_SIZE, NOTE_SIZE]);
        let r = normalize_rect(r);
        let geometry = Geometry::Note {
            pos: [r[0], r[3]],
            icon: "Comment".into(),
        };
        let mut props = self.props.clone();
        props.flags.get_or_insert(flags::NOTE_DEFAULT);
        let spec = Spec::new(geometry, props);
        spec.validate()?;
        let rect = [r[0], r[3] - NOTE_SIZE, r[0] + NOTE_SIZE, r[3]];
        let app = Appearance {
            rect,
            bbox: rect,
            content: Vec::new(),
            opacity: None,
            multiply: false,
            fonts: Vec::new(),
            rd: None,
            report: FontReport::default(),
        };
        let doc = cx.doc();
        let annot = doc.make_indirect(&doc.new_dict())?;
        fill_annotation(
            doc,
            &annot,
            &spec,
            &app,
            &Fixed {
                page: Some(&page),
                irt: Some(&parent),
            },
        )?;
        write_appearance(cx, &annot, &app)?;
        append_annot(cx, &page, &annot)?;
        cx.note_page(self.page);
        cx.changeset()
    }
}

#[allow(dead_code)]
fn _unused(_: MarkupKind, _: Color) {}
