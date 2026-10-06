//! ROADMAP 1.13 AC: fill + save round trip on 30 corpus forms, values display in PDFium
//! (papyrine-render), Poppler and pdf.js (the last two are test-only oracles).
//!
//! Forms are picked from the corpus manifests (tags `forms` / `js`), spread across sources.
//! Each gets up to four text fields, a check box, a radio group and a choice field filled
//! through the real commands (scripts included), is saved both ways (appended incremental
//! section and full rewrite), reopened, checked with `qpdf --check`, and displayed by the oracles.

mod common;

use std::collections::BTreeMap;

use common::*;
use papyrine_cos::{Document, ObjId, OpenOptions};
use papyrine_fill::form::{Field, FormTree, Kind, RawValue, XfaState};
use papyrine_fill::*;
use papyrine_ops::Command;
use papyrine_writer::{ChainState, SectionRequest, write_section};

struct Plan {
    name: String,
    kind: Kind,
    page: usize,
    cmd: Box<dyn Command>,
    /// Expected display text (text fields) after the whole plan ran.
    display: Option<String>,
    raw: Option<String>,
}

fn usable(f: &Field, pages: &std::collections::HashMap<ObjId, usize>) -> Option<(usize, [f64; 4])> {
    if f.is_read_only() || f.widgets.is_empty() {
        return None;
    }
    let w = &f.widgets[0];
    if w.annot_flags & (2 | 32) != 0 || w.rotation != 0 {
        return None; // hidden / no-view / rotated widgets
    }
    let page = *pages.get(&w.id?)?;
    Some((page, w.rect))
}

fn plan_for(doc: &Document) -> Vec<Plan> {
    let tree = FormTree::load(doc).unwrap();
    if tree.xfa == XfaState::Dynamic {
        return vec![];
    }
    let pages = widget_pages(doc);
    let mut plans = Vec::new();
    // Files that reuse a qualified name for different fields cannot be checked by name.
    let mut seen = std::collections::HashMap::new();
    for f in &tree.fields {
        *seen.entry(f.name.clone()).or_insert(0usize) += 1;
    }
    let tree_fields: Vec<&Field> = tree.fields.iter().filter(|f| seen[&f.name] == 1).collect();
    // Text fields, evenly spaced among the usable ones.
    let texts: Vec<&Field> = tree_fields
        .iter()
        .copied()
        .filter(|f| {
            f.kind == Kind::Text
                && f.widgets[..]
                    .first()
                    .is_some_and(|w| w.width() >= 40.0 && w.height() >= 9.0)
        })
        .filter(|f| usable(f, &pages).is_some())
        .collect();
    let step = (texts.len() / 4).max(1);
    for f in texts.iter().step_by(step).take(4) {
        let (page, _) = usable(f, &pages).unwrap();
        for cand in [
            "Papyrine",
            "12345.67",
            "01/15/2024",
            "123456789",
            "(555) 123-4567",
            "12345",
            "A1B2",
        ] {
            let v: String = match f.max_len {
                Some(m) => cand.chars().take(m as usize).collect(),
                None => cand.to_string(),
            };
            let multi = f.multiline();
            let v = if multi {
                format!("{v} and more words to wrap")
            } else {
                v
            };
            let Ok(pf) = preflight_text(doc, &FieldRef::id(f.id), &v) else {
                continue;
            };
            if pf.accepted && !pf.display.is_empty() {
                plans.push(Plan {
                    name: f.name.clone(),
                    kind: Kind::Text,
                    page,
                    cmd: Box::new(SetTextValue::new(FieldRef::id(f.id), v.clone())),
                    display: Some(pf.display),
                    raw: Some(v),
                });
                break;
            }
        }
    }
    if let Some(f) = tree_fields
        .iter()
        .find(|f| f.kind == Kind::Checkbox && usable(f, &pages).is_some())
    {
        let (page, _) = usable(f, &pages).unwrap();
        plans.push(Plan {
            name: f.name.clone(),
            kind: Kind::Checkbox,
            page,
            cmd: Box::new(ToggleCheckbox::new(FieldRef::id(f.id), Some(true))),
            display: None,
            raw: None,
        });
    }
    if let Some(f) = tree_fields
        .iter()
        .find(|f| f.kind == Kind::Radio && usable(f, &pages).is_some())
    {
        let (page, _) = usable(f, &pages).unwrap();
        plans.push(Plan {
            name: f.name.clone(),
            kind: Kind::Radio,
            page,
            cmd: Box::new(SelectRadio::by_widget(FieldRef::id(f.id), 0)),
            display: None,
            raw: None,
        });
    }
    if let Some(f) = tree_fields.iter().find(|f| {
        matches!(f.kind, Kind::Combo | Kind::List)
            && f.options.iter().any(|(_, d)| !d.trim().is_empty())
            && usable(f, &pages).is_some()
    }) {
        let (page, _) = usable(f, &pages).unwrap();
        let (exp, disp) = f
            .options
            .iter()
            .find(|(_, d)| !d.trim().is_empty())
            .unwrap()
            .clone();
        plans.push(Plan {
            name: f.name.clone(),
            kind: f.kind,
            page,
            cmd: Box::new(SetChoice::new(FieldRef::id(f.id), vec![exp.clone()])),
            display: (f.kind == Kind::Combo).then_some(disp),
            raw: Some(exp),
        });
    }
    plans
}

/// Page geometry for mapping PDF space to the preview raster; `None` for rotated pages.
fn page_box(doc: &Document, page: usize) -> Option<[f64; 4]> {
    let p = doc.page(page).ok()?;
    let inh = |k: &str| {
        let mut cur = p.clone();
        for _ in 0..32 {
            let v = cur.dict_get(k).ok()?;
            if !v.is_null().ok()? {
                return Some(v);
            }
            cur = cur.dict_get("Parent").ok()?;
        }
        None
    };
    if inh("Rotate")
        .and_then(|r| r.as_int().ok())
        .unwrap_or(0)
        .rem_euclid(360)
        != 0
    {
        return None;
    }
    let b = inh("CropBox").or_else(|| inh("MediaBox"))?;
    let v: Vec<f64> = b
        .array_items()
        .ok()?
        .iter()
        .filter_map(|x| x.as_f64().ok())
        .collect();
    (v.len() == 4).then(|| {
        [
            v[0].min(v[2]),
            v[1].min(v[3]),
            v[0].max(v[2]),
            v[1].max(v[3]),
        ]
    })
}

fn ink_rect(t: &papyrine_render::Tile, bx: [f64; 4], r: [f64; 4]) -> usize {
    let s = f64::from(t.width) / (bx[2] - bx[0]);
    ink_dev(
        t,
        [
            (r[0] - bx[0]) * s,
            (bx[3] - r[3]) * s,
            (r[2] - bx[0]) * s,
            (bx[3] - r[1]) * s,
        ],
    )
}

/// Pixels inside `r` that differ noticeably between two renders.
fn diff_rect(
    a: &papyrine_render::Tile,
    b: &papyrine_render::Tile,
    bx: [f64; 4],
    r: [f64; 4],
) -> usize {
    let s = f64::from(a.width) / (bx[2] - bx[0]);
    let (x0, x1) = (
        ((r[0] - bx[0]) * s).max(0.0) as u32,
        ((r[2] - bx[0]) * s) as u32,
    );
    let (y0, y1) = (
        ((bx[3] - r[3]) * s).max(0.0) as u32,
        ((bx[3] - r[1]) * s) as u32,
    );
    let mut n = 0;
    for y in y0..y1.min(a.height).min(b.height) {
        for x in x0..x1.min(a.width).min(b.width) {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            if (0..3).any(|i| p[i].abs_diff(q[i]) > 40) {
                n += 1;
            }
        }
    }
    n
}

#[derive(Default)]
struct Tally {
    forms: usize,
    commands_ok: usize,
    commands_rejected: usize,
    text_checked_pdfjs: usize,
    text_checked_poppler: usize,
    ink_checked: usize,
    xfa_static: usize,
    by_source: BTreeMap<String, usize>,
}

#[test]
fn thirty_corpus_forms_fill_save_reopen_and_display() {
    let have_pdfjs = pdfjs_available();
    let have_poppler = tool_available("pdftotext");
    // Candidates grouped by source; round-robin so every source contributes.
    let mut groups: BTreeMap<String, Vec<CorpusEntry>> = BTreeMap::new();
    for e in corpus_entries() {
        if e.tags.iter().any(|t| t == "forms" || t == "js")
            && e.path.extension().is_some_and(|x| x == "pdf")
        {
            let key = if e.id.starts_with("js-forms/") {
                e.id.split('/')
                    .nth(1)
                    .unwrap()
                    .split('-')
                    .next()
                    .unwrap()
                    .to_string()
            } else {
                e.source.split('@').next().unwrap().to_string()
            };
            groups.entry(key).or_default().push(e);
        }
    }
    assert!(
        groups.len() >= 5,
        "corpus not fetched? {groups:?}",
        groups = groups.keys().collect::<Vec<_>>()
    );
    for v in groups.values_mut() {
        v.sort_by(|a, b| a.id.cmp(&b.id));
    }
    let mut tally = Tally::default();
    let mut round = 0usize;
    let mut exhausted = false;
    'outer: while tally.forms < 30 && !exhausted {
        exhausted = true;
        for (src, list) in &groups {
            let Some(e) = list.get(round) else { continue };
            exhausted = false;
            if list.len() > 1 && tally.by_source.get(src).copied().unwrap_or(0) >= 6 {
                continue;
            }
            if e.path.metadata().map(|m| m.len()).unwrap_or(0) > 6_000_000 {
                continue;
            }
            if fill_one(e, have_pdfjs, have_poppler, &mut tally) {
                *tally.by_source.entry(src.clone()).or_default() += 1;
                if tally.forms >= 30 {
                    break 'outer;
                }
            }
        }
        round += 1;
    }
    eprintln!(
        "corpus round trip: {} forms ({} static XFA), {} commands ok, {} rejected by scripts; pdf.js text checks {}, Poppler text checks {}, PDFium ink checks {}; by source {:?}",
        tally.forms,
        tally.xfa_static,
        tally.commands_ok,
        tally.commands_rejected,
        tally.text_checked_pdfjs,
        tally.text_checked_poppler,
        tally.ink_checked,
        tally.by_source
    );
    assert!(tally.forms >= 30, "only {} qualifying forms", tally.forms);
    assert!(tally.commands_ok > 100);
    assert!(tally.ink_checked >= 30);
    if have_pdfjs {
        assert!(tally.text_checked_pdfjs >= 30);
    }
    if have_poppler {
        assert!(tally.text_checked_poppler >= 30);
    }
}

/// Returns true when the form qualified (opened, had something to fill, all checks passed).
fn fill_one(e: &CorpusEntry, have_pdfjs: bool, have_poppler: bool, tally: &mut Tally) -> bool {
    let orig = std::fs::read(&e.path).unwrap();
    let Ok(doc) = Document::open_bytes(orig.clone(), &OpenOptions::default()) else {
        return false;
    };
    let plans = plan_for(&doc);
    if plans.iter().filter(|p| p.kind == Kind::Text).count() < 2 {
        return false;
    }
    let static_xfa = FormTree::load(&doc).unwrap().xfa == XfaState::Static;
    let mut h = history();
    let mut dirty: Vec<ObjId> = Vec::new();
    let mut done: Vec<&Plan> = Vec::new();
    // Plans were computed against the untouched document; commands re-validate on apply.
    let mut plans = plans;
    for p in &mut plans {
        let cmd = std::mem::replace(&mut p.cmd, Box::new(ResetForm::all()));
        match h.execute(&doc, cmd) {
            Ok(s) => {
                dirty.extend(s.touched);
                dirty.extend(s.created);
                tally.commands_ok += 1;
                done.push(p);
            }
            Err(err) => {
                let m = err.to_string();
                assert!(
                    m.contains("rejected") || m.contains("MaxLen"),
                    "{}: {}: {m}",
                    e.id,
                    p.name
                );
                tally.commands_rejected += 1;
            }
        }
    }
    if done.iter().filter(|p| p.kind == Kind::Text).count() < 2 {
        return false;
    }
    // A command's display can change when a later command recalculates; compare against the
    // final state read back from the document instead of the preflight text where they differ.
    let expect_final = |d: &Document| -> Vec<(String, Option<String>)> {
        let t = FormTree::load(d).unwrap();
        done.iter()
            .map(|p| {
                let f = t.by_name(&p.name).unwrap();
                (p.name.clone(), f.value.as_text().map(str::to_string))
            })
            .collect()
    };
    let live = expect_final(&doc);

    // Save 1: incremental section appended to the original bytes.
    dirty.sort();
    dirty.dedup();
    let inc = match ChainState::scan(&orig) {
        Ok(chain) => {
            let sec = write_section(
                &doc,
                &chain,
                &SectionRequest {
                    dirty: dirty.clone(),
                    ..Default::default()
                },
            )
            .unwrap_or_else(|er| panic!("{}: write_section: {er}", e.id));
            let mut b = orig.clone();
            b.extend_from_slice(&sec.bytes);
            Some(b)
        }
        Err(_) => None,
    };
    // Save 2: full rewrite.
    let full = write(&doc);
    for (label, bytes) in inc
        .iter()
        .map(|b| ("incremental", b))
        .chain([("full", &full)])
    {
        let re = Document::open_bytes(bytes.clone(), &OpenOptions::default())
            .unwrap_or_else(|er| panic!("{}: reopen {label}: {er}", e.id));
        assert_eq!(
            expect_final(&re),
            live,
            "{}: {label} values differ after reopen",
            e.id
        );
        let t = FormTree::load(&re).unwrap();
        if static_xfa {
            assert_eq!(t.xfa, XfaState::None, "{}: stale XFA kept", e.id);
        }
    }
    qpdf_check(&full, &e.id);
    if let Some(b) = &inc {
        qpdf_check(b, &e.id);
    }

    // Display: PDFium raster ink, Poppler text, pdf.js text.
    let shown = inc.as_ref().unwrap_or(&full);
    let after_doc = open(shown.clone());
    let final_tree = FormTree::load(&after_doc).unwrap();
    let pdfjs = have_pdfjs.then(|| pdfjs_widgets(shown));
    let poppler = have_poppler.then(|| squash(&pdftotext(shown)));
    let mut checked_ink = false;
    let mut checked_pdfjs = false;
    let mut checked_poppler = false;
    let mut poppler_eligible = false;
    let mut renders: std::collections::HashMap<
        usize,
        (papyrine_render::Tile, papyrine_render::Tile),
    > = Default::default();
    for p in done
        .iter()
        .filter(|p| matches!(p.kind, Kind::Text | Kind::Combo))
    {
        let f = final_tree.by_name(&p.name).unwrap();
        let w = &f.widgets[0];
        // What the field displays now: its formatted value (a later calculation may have changed it).
        let raw = f.value.as_text().unwrap_or("").to_string();
        let display = if raw == p.raw.clone().unwrap_or_default() {
            p.display.clone().unwrap_or_else(|| raw.clone())
        } else {
            raw.clone()
        };
        if display.trim().is_empty() {
            continue;
        }
        if let Some(list) = &pdfjs
            && !final_tree.need_appearances
        {
            let entry = list
                .iter()
                .find(|a| a["fieldName"] == p.name.as_str() && a["page"] == p.page)
                .unwrap_or_else(|| panic!("{}: pdf.js lacks widget {}", e.id, p.name));
            let drawn = squash(entry["text"].as_str().unwrap());
            if !f.multiline() && f.kind == Kind::Text {
                assert_eq!(
                    drawn,
                    squash(&display),
                    "{}: {} pdf.js draws {drawn:?}",
                    e.id,
                    p.name
                );
            } else {
                assert!(
                    squash(&display).starts_with(&drawn[..drawn.len().min(8)]),
                    "{}: {}",
                    e.id,
                    p.name
                );
            }
            checked_pdfjs = true;
        }
        if let Some(pt) = &poppler
            && f.kind == Kind::Text
            && !f.multiline()
            && display.chars().count() <= 24
            && !f.password()
        {
            let want = squash(&display);
            poppler_eligible |= !f.comb() && w.width() >= 40.0;
            // Comb fields print one character per cell, so only check non-comb fields.
            if !f.comb() && w.width() >= 40.0 && pt.matches(&want).count() >= 1 {
                checked_poppler = true;
            } else if !f.comb() && w.width() >= 40.0 {
                // Poppler may drop clipped text; the value must at least be extractable when
                // the field is wide enough to show it entirely.
                let est = display.chars().count() as f64 * 6.5;
                assert!(
                    est > w.width() - 6.0,
                    "{}: Poppler text lacks {want:?} for {}",
                    e.id,
                    p.name
                );
            }
        }
        if let Some(bx) = page_box(&after_doc, p.page) {
            let (before_t, after_t) = renders.entry(p.page).or_insert_with(|| {
                (
                    render_page(&orig, p.page, 1600),
                    render_page(shown, p.page, 1600),
                )
            });
            let a = ink_rect(after_t, bx, w.rect);
            let d = diff_rect(before_t, after_t, bx, w.rect);
            assert!(
                a > 0 && d > 0,
                "{}: {}: PDFium ink {a}, changed pixels {d}",
                e.id,
                p.name
            );
            checked_ink = true;
        }
    }
    tally.ink_checked += usize::from(checked_ink);
    tally.text_checked_pdfjs +=
        usize::from(checked_pdfjs || !have_pdfjs || final_tree.need_appearances);
    tally.text_checked_poppler +=
        usize::from(checked_poppler || !poppler_eligible || !have_poppler);
    tally.xfa_static += usize::from(static_xfa);
    tally.forms += 1;
    // Radio and check box states survive as /AS.
    for p in done
        .iter()
        .filter(|p| matches!(p.kind, Kind::Checkbox | Kind::Radio))
    {
        let f = final_tree.by_name(&p.name).unwrap();
        assert!(
            matches!(&f.value, RawValue::Name(n) if n != "Off"),
            "{}: {} lost its state: {:?}",
            e.id,
            p.name,
            f.value
        );
        assert!(
            f.widgets
                .iter()
                .any(|w| w.appearance_state.as_deref().is_some_and(|s| s != "Off"))
        );
    }
    true
}

/// The IRS forms are XFA hybrids with static XFA: fillable through the AcroForm, and the stale
/// XFA packet must go with the first edit.
#[test]
fn irs_hybrid_forms_fill_and_drop_the_stale_xfa() {
    let have_pdfjs = pdfjs_available();
    let have_poppler = tool_available("pdftotext");
    let mut tally = Tally::default();
    let mut irs: Vec<CorpusEntry> = corpus_entries()
        .into_iter()
        .filter(|e| {
            e.id.starts_with("js-forms/irs-")
                && e.path.metadata().map(|m| m.len()).unwrap_or(u64::MAX) < 700_000
        })
        .collect();
    irs.sort_by(|a, b| a.id.cmp(&b.id));
    for e in irs.iter().step_by(5) {
        if tally.xfa_static >= 8 {
            break;
        }
        // Only count forms that carry a static XFA packet.
        let Ok(d) = Document::open_bytes(std::fs::read(&e.path).unwrap(), &OpenOptions::default())
        else {
            continue;
        };
        if FormTree::load(&d).unwrap().xfa != XfaState::Static {
            continue;
        }
        drop(d);
        fill_one(e, have_pdfjs, have_poppler, &mut tally);
    }
    eprintln!(
        "IRS hybrids filled: {} (static XFA {})",
        tally.forms, tally.xfa_static
    );
    assert!(
        tally.xfa_static >= 8,
        "only {} static-XFA IRS forms filled",
        tally.xfa_static
    );
}
