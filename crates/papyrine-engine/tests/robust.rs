mod common;

use common::*;
use papyrine_engine::proto::*;
use papyrine_ipc::{BlobRef, CommandRequest, DocId, ErrorCode};
use papyrine_ops::{
    ChangeSet, Command, CommandRegistry, EditContext, Error as OpsError, LocalizedText, Result,
    encode_text_string,
};
use serde_json::{Value, json};

/// Edits a page, then does something bad.
struct Bad {
    how: &'static str,
}

impl Command for Bad {
    fn name(&self) -> &'static str {
        "bad"
    }
    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.bad")
    }
    fn params(&self) -> Value {
        json!({"how": self.how})
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let page = doc.page(0)?;
        cx.set_key(&page, "Rotate", &doc.new_int(90))?;
        // Also allocate an object, like a command that fails halfway through.
        doc.make_indirect(&doc.parse_object("<< /Half true >>")?)?;
        match self.how {
            "panic" => panic!("boom"),
            _ => Err(OpsError::invalid("failed halfway")),
        }
    }
}

/// Sets the title to a text argument (used to check blob substitution).
struct TitleFrom {
    data: String,
}

impl Command for TitleFrom {
    fn name(&self) -> &'static str {
        "title_from"
    }
    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.title-from")
    }
    fn params(&self) -> Value {
        json!({"data": self.data})
    }
    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let info = doc.trailer()?.dict_get("Info")?;
        cx.set_key(
            &info,
            "Title",
            &doc.new_string(encode_text_string(&self.data))?,
        )?;
        cx.changeset()
    }
}

fn registry() -> CommandRegistry {
    let mut r = papyrine_engine::default_registry();
    r.register("bad", |_, p| {
        let how = if p["how"] == "panic" {
            "panic"
        } else {
            "error"
        };
        Ok(Box::new(Bad { how }))
    });
    r.register("title_from", |_, p| {
        Ok(Box::new(TitleFrom {
            data: p["data"].as_str().unwrap_or_default().to_string(),
        }))
    });
    r
}

#[test]
fn a_panicking_command_is_rolled_back_and_reported() {
    let mut e = Eng::with_registry(registry());
    e.open(1, build_pdf(3));
    let before = e.digest(1);
    let err = e.try_exec(1, "bad", json!({"how": "panic"})).unwrap_err();
    assert_eq!(err.code, ErrorCode::Internal);
    assert!(err.message.contains("boom"), "{}", err.message);
    // Page 0 is not rotated, nothing was recorded, and the engine keeps working.
    let after = e.digest(1);
    // The half-made object is an unreachable orphan: compare every pre-existing object.
    for (n, g, h) in &before.per_object {
        assert!(
            after.per_object.contains(&(*n, *g, *h)),
            "object {n} changed"
        );
    }
    match e.query(1, Query::Pages { from: 0, count: 1 }) {
        QueryResult::Pages(p) => assert_eq!(p[0].rotation, 0),
        _ => unreachable!(),
    }
    assert!(
        e.try_call(Request::Undo {
            doc: DocId(1),
            out_section: None
        })
        .is_err()
    );
    let c = e.exec(1, "rotate_pages", json!({"pages": [1], "delta": 90}));
    assert!(c.summary.can_undo);
    // And an ordinary error behaves the same.
    let err = e.try_exec(1, "bad", json!({"how": "error"})).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidRequest);
    match e.query(1, Query::Pages { from: 0, count: 3 }) {
        QueryResult::Pages(p) => {
            assert_eq!(p.iter().map(|p| p.rotation).collect::<Vec<_>>(), [0, 90, 0])
        }
        _ => unreachable!(),
    }
}

#[test]
fn blob_arguments_reach_the_command() {
    let mut e = Eng::with_registry(registry());
    e.open(1, build_pdf(1));
    let r = e.try_call(Request::Execute {
        doc: DocId(1),
        command: CommandRequest {
            name: "title_from".into(),
            params_json: json!({"data": {"$blob": 0}}).to_string(),
        },
        blobs: vec![BlobRef::Inline(b"hello".to_vec())],
        out_section: None,
    });
    assert!(r.is_ok(), "{r:?}");
    match e.query(1, Query::Info) {
        QueryResult::Info(i) => assert_eq!(i.title.as_deref(), Some("aGVsbG8=")),
        _ => unreachable!(),
    }
    // A reference to a blob that was not sent is a clean error.
    let r = e.try_call(Request::Execute {
        doc: DocId(1),
        command: CommandRequest {
            name: "title_from".into(),
            params_json: json!({"data": {"$blob": 3}}).to_string(),
        },
        blobs: vec![],
        out_section: None,
    });
    assert_eq!(r.unwrap_err().code, ErrorCode::InvalidRequest);
}

#[test]
fn history_spills_and_undo_stays_exact() {
    let mut e = Eng::new();
    let orig = build_pdf(6);
    let r = e.try_call(Request::Open {
        doc: DocId(1),
        source: papyrine_ipc::DocSource::Bytes(orig),
        password: None,
        params: OpenParams {
            skip_pages: true,
            spill_budget_bytes: Some(1),
        },
    });
    let Response::Opened(s) = r.unwrap() else {
        panic!()
    };
    assert!(
        s.pages.is_empty() && s.page_count == 6,
        "skip_pages leaves the list empty"
    );
    let start = e.digest(1);
    let mut digests = vec![start.digest];
    for i in 0..12 {
        e.exec(1, "rotate_pages", json!({"pages": [i % 6], "delta": 90}));
        digests.push(e.digest(1).digest);
    }
    for want in digests.iter().rev().skip(1) {
        e.undo(1);
        assert_eq!(&e.digest(1).digest, want);
    }
    for want in digests.iter().skip(1) {
        e.redo(1);
        assert_eq!(&e.digest(1).digest, want);
    }
}

#[test]
fn model_queries_cover_the_document() {
    let mut e = Eng::new();
    e.open(1, build_pdf(3));
    match e.query(1, Query::Labels) {
        QueryResult::Labels(l) => assert!(l.is_empty()),
        _ => unreachable!(),
    }
    match e.query(1, Query::Fields) {
        QueryResult::Fields(f) => assert!(f.is_empty()),
        _ => unreachable!(),
    }
    match e.query(1, Query::Annotations { page: 0 }) {
        QueryResult::Annotations(a) => assert!(a.is_empty()),
        _ => unreachable!(),
    }
    assert!(
        e.try_call(Request::Query {
            doc: DocId(1),
            query: Query::Annotations { page: 9 }
        })
        .is_err()
    );
    match e.query(1, Query::Security) {
        QueryResult::Security(s) => assert!(!s.encrypted && s.effective.print),
        _ => unreachable!(),
    }
    match e.query(1, Query::Outline) {
        QueryResult::Outline(o) => assert!(o.is_empty()),
        _ => unreachable!(),
    }
    match e.query(1, Query::Info) {
        QueryResult::Info(i) => {
            assert_eq!(i.producer.as_deref(), Some("engine-tests"));
            assert_eq!(i.version, "1.4");
        }
        _ => unreachable!(),
    }
    // Edits show up in later queries (the model's caches are invalidated).
    e.exec(
        1,
        "set_info_field",
        json!({"field": "Author", "value": "Ada"}),
    );
    match e.query(1, Query::Info) {
        QueryResult::Info(i) => assert_eq!(i.author.as_deref(), Some("Ada")),
        _ => unreachable!(),
    }
    e.undo(1);
    match e.query(1, Query::Info) {
        QueryResult::Info(i) => assert_eq!(i.author, None),
        _ => unreachable!(),
    }
}

#[test]
fn corpus_documents_open_with_rich_metadata() {
    for name in [
        "typical-20p.pdf",
        "js-forms/ca-100-7b86.pdf",
        "js-forms/ca-3506-e0fe.pdf",
    ] {
        let Some(bytes) = corpus(name) else { continue };
        let mut e = Eng::new();
        let s = e.open(1, bytes);
        assert!(s.page_count > 0, "{name}");
        assert_eq!(s.pages.len() as u32, s.page_count);
    }
}

#[test]
fn forms_are_summarised_with_values() {
    let Some(bytes) = corpus("js-forms/ca-100-7b86.pdf") else {
        return;
    };
    let mut e = Eng::new();
    let s = e.open(1, bytes);
    assert!(s.form.has_acroform, "{:?}", s.form);
    assert!(s.form.field_count > 0 && s.form.widget_count >= s.form.field_count);
    let QueryResult::Fields(f) = e.query(1, Query::Fields) else {
        unreachable!()
    };
    assert_eq!(f.len() as u32, s.form.field_count);
    assert!(f.iter().any(|f| f.kind == "text"));
    assert!(f.iter().all(|f| !f.name.is_empty()));
    let with_pages = f.iter().filter(|f| !f.pages.is_empty()).count();
    assert!(with_pages > 0);
    if s.annotations.total > 0 {
        let p = s
            .pages
            .iter()
            .find(|p| p.annotation_count > 0)
            .unwrap()
            .index;
        let QueryResult::Annotations(a) = e.query(1, Query::Annotations { page: p }) else {
            unreachable!()
        };
        assert!(!a.is_empty());
    }
}
