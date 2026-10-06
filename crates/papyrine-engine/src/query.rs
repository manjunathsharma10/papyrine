//! Model queries: typed read views turned into the plain DTOs of the protocol.

use std::collections::BTreeMap;

use papyrine_model::{
    AnnotSubtype, Annotation, Field, FieldKind, FieldValue, LabelStyle, Model, OutlineItem,
    PermissionSummary, XfaKind,
};

use crate::proto::{
    AnnotDto, AnnotSummary, FieldDto, FormSummary, InfoDto, LabelRangeDto, OutlineDto, PageDto,
    PermsDto, Result, SecurityDto, SignatureSummaryDto,
};

fn perms(p: PermissionSummary) -> PermsDto {
    PermsDto {
        print: p.print,
        print_high_quality: p.print_high_quality,
        copy: p.copy,
        accessibility: p.accessibility,
        modify_contents: p.modify_contents,
        annotate: p.annotate,
        fill_forms: p.fill_forms,
        assemble: p.assemble,
    }
}

pub fn pages(m: &Model, from: usize, count: usize) -> Result<Vec<PageDto>> {
    let n = m.page_count()?;
    let labels = m.page_labels()?;
    let end = from.saturating_add(count).min(n);
    let mut out = Vec::with_capacity(end.saturating_sub(from));
    for i in from..end {
        let p = m.page(i)?;
        let s = p.display_size();
        out.push(PageDto {
            index: i as u32,
            width_pt: s.w,
            height_pt: s.h,
            rotation: p.rotate,
            label: labels.label(i),
            annotation_count: p.annotation_count as u32,
        });
    }
    Ok(out)
}

fn outline_item(i: &OutlineItem) -> OutlineDto {
    OutlineDto {
        title: i.title.clone(),
        page: i
            .dest
            .as_ref()
            .and_then(|d| d.local_page())
            .map(|p| p as u32),
        open: i.open,
        children: i.children.iter().map(outline_item).collect(),
    }
}

pub fn outline(m: &Model) -> Result<Vec<OutlineDto>> {
    Ok(m.outlines()?.items.iter().map(outline_item).collect())
}

pub fn labels(m: &Model) -> Result<Vec<LabelRangeDto>> {
    Ok(m.page_labels()?
        .ranges
        .iter()
        .map(|r| LabelRangeDto {
            start_index: r.start_index as u32,
            style: r.style.map(|s| {
                match s {
                    LabelStyle::Decimal => "decimal",
                    LabelStyle::UpperRoman => "upper-roman",
                    LabelStyle::LowerRoman => "lower-roman",
                    LabelStyle::UpperAlpha => "upper-alpha",
                    LabelStyle::LowerAlpha => "lower-alpha",
                }
                .to_string()
            }),
            prefix: r.prefix.clone(),
            first_number: r.first_number,
        })
        .collect())
}

fn kind_name(k: &FieldKind) -> &'static str {
    match k {
        FieldKind::Text => "text",
        FieldKind::PushButton => "push-button",
        FieldKind::Checkbox => "checkbox",
        FieldKind::Radio => "radio",
        FieldKind::ComboBox => "combo-box",
        FieldKind::ListBox => "list-box",
        FieldKind::Signature => "signature",
        FieldKind::Unknown => "unknown",
    }
}

fn field(f: &Field) -> FieldDto {
    let (value, values) = match &f.value {
        FieldValue::None => (None, vec![]),
        FieldValue::Text(s) | FieldValue::Name(s) => (Some(s.clone()), vec![]),
        FieldValue::List(l) => (l.first().cloned(), l.clone()),
    };
    let mut pages: Vec<u32> = f
        .widgets
        .iter()
        .filter_map(|w| w.page.map(|p| p as u32))
        .collect();
    pages.dedup();
    FieldDto {
        id: (f.id.num, f.id.generation),
        name: f.qualified_name.clone(),
        kind: kind_name(&f.kind).to_string(),
        value,
        values,
        flags: f.flags,
        read_only: f.is_read_only(),
        required: f.is_required(),
        max_len: f.max_len,
        pages,
        options: f
            .options
            .iter()
            .map(|o| (o.export.clone(), o.display.clone()))
            .collect(),
    }
}

/// Terminal fields with their values.
pub fn fields(m: &Model) -> Result<Vec<FieldDto>> {
    Ok(m.form()?.terminal_fields().map(field).collect())
}

pub fn form_summary(m: &Model) -> Result<FormSummary> {
    let f = m.form()?;
    Ok(FormSummary {
        has_acroform: f.has_acroform,
        field_count: f.terminal_fields().count() as u32,
        widget_count: f.widget_count() as u32,
        signature_fields: f.signature_fields().count() as u32,
        need_appearances: f.need_appearances,
        xfa: match f.xfa {
            XfaKind::None => "none",
            XfaKind::Static => "static",
            XfaKind::Dynamic => "dynamic",
        }
        .to_string(),
        needs_rendering: f.needs_rendering,
    })
}

fn annot(a: &Annotation) -> AnnotDto {
    AnnotDto {
        id: a.id.map(|i| (i.num, i.generation)),
        page: a.page as u32,
        index: a.index as u32,
        subtype: a.subtype.name().to_string(),
        rect: a.rect.to_pdf_array(),
        flags: a.flags,
        contents: a.contents.clone(),
        author: a.title.clone(),
        subject: a.subject.clone(),
        name: a.name.clone(),
        modified: a.modified_raw.clone(),
        color: a.color.clone(),
        opacity: a.opacity,
        has_appearance: a.has_appearance,
        in_reply_to: a.in_reply_to.map(|i| (i.num, i.generation)),
        uri: a.uri().map(str::to_owned),
    }
}

pub fn annotations(m: &Model, page: usize) -> Result<Vec<AnnotDto>> {
    Ok(m.annotations(page)?.items.iter().map(annot).collect())
}

/// Counts over all pages that have annotations. Widgets are form controls, not annotations the
/// user sees in the comments list, but they are counted under `Widget`.
pub fn annot_summary(m: &Model, page_infos: &[PageDto]) -> Result<AnnotSummary> {
    let mut by: BTreeMap<String, u32> = BTreeMap::new();
    let mut total = 0u32;
    let mut with = 0u32;
    for p in page_infos.iter().filter(|p| p.annotation_count > 0) {
        with += 1;
        for a in &m.annotations(p.index as usize)?.items {
            total += 1;
            let name = match &a.subtype {
                AnnotSubtype::Other(s) => s.as_str(),
                s => s.name(),
            };
            *by.entry(name.to_string()).or_default() += 1;
        }
    }
    Ok(AnnotSummary {
        total,
        pages_with_annotations: with,
        by_subtype: by.into_iter().collect(),
    })
}

pub fn info(m: &Model) -> Result<InfoDto> {
    let i = m.info()?;
    let c = m.catalog_info()?;
    Ok(InfoDto {
        title: i.title.clone(),
        author: i.author.clone(),
        subject: i.subject.clone(),
        keywords: i.keywords.clone(),
        creator: i.creator.clone(),
        producer: i.producer.clone(),
        creation_date: i.creation_date_raw.clone(),
        mod_date: i.mod_date_raw.clone(),
        custom: i
            .custom
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        version: c.version.clone(),
        linearized: c.is_linearized,
        page_layout: c.page_layout.clone(),
        page_mode: c.page_mode.clone(),
        language: c.language.clone(),
        tagged: c.is_marked,
        has_xmp: c.has_xmp,
    })
}

pub fn security(m: &Model) -> Result<SecurityDto> {
    let s = m.security()?;
    Ok(SecurityDto {
        encrypted: s.encrypted,
        algorithm: s.algorithm,
        owner_authenticated: s.owner_authenticated,
        declared: perms(s.declared),
        effective: perms(s.effective),
    })
}

pub fn signatures(m: &Model) -> Result<SignatureSummaryDto> {
    let s = m.signatures()?;
    Ok(SignatureSummaryDto {
        signed: s.is_signed(),
        signed_fields: s.signed_count() as u32,
        unsigned_fields: s.unsigned_count() as u32,
        certified: s.certified,
        requires_incremental_save: s.requires_incremental_save(),
    })
}
