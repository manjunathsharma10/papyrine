mod common;
use common::*;
use papyrine_text::*;

#[test]
fn basic_case_insensitive_and_sensitive() {
    let o = opts();
    assert_eq!(matched("Hello hello HELLO", "hello", &o).len(), 3);
    let cs = SearchOptions {
        case_sensitive: true,
        ..opts()
    };
    assert_eq!(matched("Hello hello HELLO", "hello", &cs), ["hello"]);
    assert!(matched("abc", "", &o).is_empty());
    assert!(matched("abc", "   ", &o).is_empty());
}

#[test]
fn full_case_folding() {
    let o = opts();
    assert_eq!(matched("Straße STRASSE", "strasse", &o).len(), 2);
    assert_eq!(matched("Σίσυφος ΣΊΣΥΦΟΣ", "σίσυφοσ", &o).len(), 2);
    assert_eq!(matched("ПРИВЕТ привет", "Привет", &o).len(), 2);
}

#[test]
fn turkish_dotted_dotless() {
    let tr = SearchOptions {
        turkish_case: true,
        ..opts()
    };
    assert_eq!(matched("ISI ısı", "ısı", &tr).len(), 2);
    assert_eq!(matched("İstanbul istanbul", "istanbul", &tr).len(), 2);
    // Without the locale, I matches i, and ı is a distinct letter.
    let d = opts();
    assert_eq!(matched("ISI ısı", "isi", &d), ["ISI"]);
}

#[test]
fn ligatures_and_presentation_forms() {
    let o = opts();
    assert_eq!(
        matched(
            "the \u{fb01}rst of\u{fb02}ine e\u{fb03}cient \u{fb00}",
            "first",
            &o
        ),
        ["\u{fb01}rst"]
    );
    assert_eq!(matched("of\u{fb02}ine", "offline", &o).len(), 1);
    assert_eq!(matched("e\u{fb03}cient", "efficient", &o).len(), 1);
    assert_eq!(
        matched("e\u{fb04}uent \u{fb05}\u{fb06}", "effluent", &o).len(),
        1
    );
    assert_eq!(matched("\u{fb05}", "st", &o).len(), 1);
    // query typed with a ligature matches plain text
    assert_eq!(matched("a first b", "\u{fb01}rst", &o).len(), 1);
    // Arabic presentation forms (lam-alef, isolated/medial letters)
    assert_eq!(
        matched("\u{fefb}\u{fe8d}", "\u{644}\u{627}\u{627}", &o).len(),
        1
    );
    assert_eq!(
        matched("\u{fe8e}\u{fee3}\u{fea3}", "\u{627}\u{645}\u{62d}", &o).len(),
        1
    );
    // fullwidth Latin and wide digits
    assert_eq!(
        matched("\u{ff21}\u{ff22}\u{ff23} \u{ff11}\u{ff12}", "abc 12", &o).len(),
        1
    );
}

#[test]
fn hyphenation_joining() {
    let o = opts();
    // hyphen + newline + lowercase: joined
    let t = "the coop-\nerate plan";
    assert_eq!(matched(t, "cooperate", &o), ["coop-\nerate"]);
    // CRLF as PDFium emits
    assert_eq!(matched("exam-\r\nple text", "example", &o).len(), 1);
    // uppercase continuation is not joined
    assert!(matched("Anglo-\nSaxon", "anglosaxon", &o).is_empty());
    assert_eq!(matched("Anglo-\nSaxon", "anglo- saxon", &o).len(), 1);
    // a hyphen mid-line is kept
    assert_eq!(matched("well-known", "well-known", &o).len(), 1);
    assert!(matched("well-known", "wellknown", &o).is_empty());
    // soft hyphen anywhere, and PDFium's U+0002 marker at a line end
    assert_eq!(matched("co\u{ad}operate", "cooperate", &o).len(), 1);
    assert_eq!(matched("co\u{ad}\nOperate", "cooperate", &o).len(), 1);
    assert_eq!(matched("hy\u{2}\r\nphen", "hyphen", &o).len(), 1);
    // query containing a soft hyphen
    assert_eq!(matched("cooperate", "co\u{ad}operate", &o).len(), 1);
}

#[test]
fn whitespace_collapsing_across_lines() {
    let o = opts();
    let t = "red\r\n  green \u{a0}\t blue";
    assert_eq!(matched(t, "red green blue", &o).len(), 1);
    assert_eq!(matched(t, "  green   blue ", &o).len(), 1);
    let hits = run("one two\nthree four", "two three", &o);
    assert_eq!(hits[0].quads.len(), 2, "one quad per line");
}

#[test]
fn diacritic_insensitive() {
    let di = SearchOptions {
        diacritic_insensitive: true,
        ..opts()
    };
    assert_eq!(matched("Café cafe CAFÉ", "cafe", &di).len(), 3);
    assert_eq!(matched("naïve résumé", "resume", &di).len(), 1);
    // composed vs decomposed input
    assert_eq!(matched("e\u{301}t\u{e9}", "ete", &di).len(), 1);
    assert_eq!(matched("Ångström", "angstrom", &di).len(), 1);
    // sensitive: e does not match é, and never splits the cluster
    let o = opts();
    assert!(matched("café", "cafe", &o).is_empty());
    assert_eq!(matched("café", "café", &o).len(), 1);
    assert_eq!(
        matched("cafe\u{301}", "café", &o).len(),
        1,
        "NFD text, NFC query"
    );
    assert!(matched("cafe\u{301}", "cafe", &o).is_empty());
    // Arabic harakat and hamza carriers, Hebrew niqqud
    assert_eq!(
        matched(
            "\u{643}\u{64e}\u{62a}\u{64e}\u{628}\u{64e}",
            "\u{643}\u{62a}\u{628}",
            &di
        )
        .len(),
        1
    );
    assert_eq!(
        matched(
            "\u{623}\u{62d}\u{645}\u{62f}",
            "\u{627}\u{62d}\u{645}\u{62f}",
            &di
        )
        .len(),
        1
    );
    assert_eq!(
        matched(
            "\u{5e9}\u{5b8}\u{5dc}\u{5d5}\u{5b9}\u{5dd}",
            "\u{5e9}\u{5dc}\u{5d5}\u{5dd}",
            &di
        )
        .len(),
        1
    );
    // kana voiced marks are letters, not accents
    assert!(matched("\u{304c}", "\u{304b}", &di).is_empty());
}

#[test]
fn whole_word() {
    let w = SearchOptions {
        whole_word: true,
        ..opts()
    };
    assert_eq!(matched("cat concat cats cat.", "cat", &w).len(), 2);
    assert_eq!(matched("a cat's tail", "cat", &w).len(), 0);
    assert_eq!(matched("don't stop", "don't", &w).len(), 1);
    assert_eq!(matched("(cat)", "cat", &w).len(), 1);
    assert_eq!(matched("scat cat", "cat", &w).len(), 1);
    // multi-word, start/end on boundaries
    assert_eq!(matched("a big cat sat", "big cat", &w).len(), 1);
    assert!(matched("a bigcat sat", "big cat", &w).is_empty());
    let nw = opts();
    assert_eq!(matched("cat concat cats cat.", "cat", &nw).len(), 4);
}

#[test]
fn arabic_hebrew_logical_order() {
    let o = opts();
    let ar = "انواع الخطوط العربية";
    assert_eq!(matched(ar, "الخطوط", &o), ["الخطوط"]);
    assert_eq!(matched(ar, "الخطوط العربية", &o).len(), 1);
    assert_eq!(matched(ar, "لخطوط العربية", &o).len(), 1);
    assert_eq!(matched(ar, "عربيةال", &o).len(), 0);
    let he = "שלום עולם, hello 2024 בנימין";
    assert_eq!(matched(he, "עולם", &o).len(), 1);
    assert_eq!(matched(he, "בנימין", &o).len(), 1);
    assert_eq!(matched(he, "hello 2024", &o).len(), 1);
    // tatweel and bidi marks are transparent
    assert_eq!(matched("الـــعربية", "العربية", &o).len(), 1);
    assert_eq!(
        matched("\u{200f}שלום\u{200f} עולם", "שלום עולם", &o).len(),
        1
    );
}

#[test]
fn visual_order_input_is_reordered() {
    // "שלום עולם" stored visually reversed, as a naive extractor would.
    let visual: String = "שלום עולם".chars().rev().collect();
    let v = SearchOptions {
        bidi: BidiOrder::Visual,
        ..opts()
    };
    assert_eq!(run(&visual, "שלום עולם", &v).len(), 1);
    assert_eq!(run(&visual, "עולם", &v).len(), 1);
    // and logical mode would not find it
    assert!(run(&visual, "שלום עולם", &opts()).is_empty());
    // mixed: LTR runs keep their order
    let line: String = "abc שלום".chars().rev().collect::<String>();
    let hits = run(&line, "שלום", &v);
    assert_eq!(hits.len(), 1);
    assert_eq!(run("hello world", "hello world", &v).len(), 1);
}

#[test]
fn cjk() {
    let o = opts();
    assert_eq!(matched("日本語 テキスト", "日本語", &o).len(), 1);
    assert_eq!(matched("日本語のテキストです。", "テキスト", &o).len(), 1);
    assert_eq!(matched("中华人民共和国成立了", "共和国", &o).len(), 1);
    assert_eq!(matched("中华人民共和国成立了", "共和 国", &o).len(), 0);
    // fullwidth/halfwidth katakana fold together under NFKD
    assert_eq!(matched("ﾃｷｽﾄ", "テキスト", &o).len(), 1);
    // whole word: ideographs break per char, so a CJK substring is a "word"
    let w = SearchOptions {
        whole_word: true,
        ..opts()
    };
    assert_eq!(matched("日本語のテキストです。", "日本語", &w).len(), 1);
    // Hangul: never match inside a syllable
    assert_eq!(matched("한국어 하나", "한국", &o).len(), 1);
    assert_eq!(matched("한국어", "하", &o).len(), 0);
    assert_eq!(matched("하나 한", "하", &o).len(), 1);
    // a line break inside CJK text is not a separator (but is in Latin text)
    assert_eq!(matched("リスク\r\n中立確率", "リスク中立", &o).len(), 1);
    assert_eq!(matched("リスク\r\n中立確率", "リスク 中立", &o).len(), 0);
    assert_eq!(matched("foo\nbar", "foobar", &o).len(), 0);
    // CJK in a Latin sentence
    assert_eq!(matched("Tokyo 東京 Japan", "東京", &o).len(), 1);
}

#[test]
fn ranges_quads_and_snippets() {
    let o = opts();
    let text = "The quick brown fox jumps over the lazy dog";
    let hits = run(text, "brown fox", &o);
    assert_eq!(hits.len(), 1);
    let h = &hits[0];
    assert_eq!(h.range, 10..19);
    assert_eq!(h.quads.len(), 1);
    let b = h.quads[0].bounds();
    assert_eq!((b.x0, b.x1), (100.0, 190.0));
    assert_eq!(h.snippet.before, "The quick ");
    assert_eq!(h.snippet.matched, "brown fox");
    assert_eq!(h.snippet.after, " jumps over the lazy dog");
    // short context, snapped to word starts
    let o2 = SearchOptions {
        snippet_context: 6,
        ..opts()
    };
    let h = &run(text, "brown fox", &o2)[0];
    assert_eq!(h.snippet.before, "quick ");
    assert_eq!(h.snippet.after, " jumps");

    // multi-line hit: one merged quad per line, newline chars excluded
    let h = &run("alpha beta\r\ngamma delta", "beta gamma", &o)[0];
    // (page() only advances lines on '\n')
    assert_eq!(h.quads.len(), 2, "{:?}", h.quads);
    let q = h.quads[0].to_array();
    assert_eq!(q.len(), 8);
    // ordinal numbering
    let hits = run("aa aa aa", "aa", &o);
    assert_eq!(
        hits.iter().map(|h| h.ordinal).collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn ligature_hit_covers_whole_glyph() {
    let h = &run("x \u{fb01}sh", "fi", &opts())[0];
    assert_eq!(h.range, 2..3);
    assert_eq!(h.quads.len(), 1);
}

#[test]
fn vertical_cjk_column_is_one_quad() {
    use papyrine_core::Rect;
    // three chars stacked in a column, then a second column
    let chars: Vec<char> = "縦書きテスト".chars().collect();
    let mut boxes = vec![];
    for i in 0..3 {
        boxes.push(Rect::new(
            200.0,
            700.0 - 12.0 * i as f64,
            212.0,
            712.0 - 12.0 * i as f64,
        ));
    }
    for i in 0..3 {
        boxes.push(Rect::new(
            180.0,
            700.0 - 12.0 * i as f64,
            192.0,
            712.0 - 12.0 * i as f64,
        ));
    }
    let p = PageInput {
        page: 0,
        text: PageChars::new(chars, boxes),
        extras: vec![],
    };
    let hits = run_pages(vec![p], "きテ", &opts());
    assert_eq!(hits[0].quads.len(), 2);
    let hits = run_pages(vec![page("x")], "縦", &opts());
    assert!(hits.is_empty());
}

#[test]
fn extra_sources() {
    let mut p = page("body text here");
    p.extras.push(ExtraText {
        source: TextSource::Annotation("annot-7".into()),
        text: "Please review the body carefully".into(),
        anchor: Some(papyrine_core::Rect::new(10.0, 10.0, 50.0, 30.0)),
    });
    p.extras.push(ExtraText {
        source: TextSource::FormField("form.name".into()),
        text: "Bodhi Body".into(),
        anchor: None,
    });
    let hits = run_pages(vec![p], "body", &opts());
    assert_eq!(hits.len(), 3);
    assert_eq!(hits[0].source, TextSource::PageText);
    assert_eq!(hits[1].source, TextSource::Annotation("annot-7".into()));
    assert_eq!(hits[1].quads.len(), 1);
    assert_eq!(hits[1].snippet.before, "Please review the ");
    assert_eq!(hits[2].source, TextSource::FormField("form.name".into()));
    assert!(hits[2].quads.is_empty());
    assert_eq!(hits[2].ordinal, 0);
}

#[test]
fn streaming_cancel_and_limits() {
    use papyrine_core::CancelToken;
    let pages = |n| {
        (0..n).map(|i| {
            let mut p = page("needle in a haystack");
            p.page = i;
            p
        })
    };
    // events: hits stream before the next page is pulled
    let pulled = std::cell::Cell::new(0usize);
    let lazy = (0..5).map(|i| {
        pulled.set(pulled.get() + 1);
        let mut p = page("needle");
        p.page = i;
        p
    });
    let mut seen_at_first_hit = None;
    let cancel = CancelToken::new();
    let sum = search_document(lazy, "needle", &opts(), &cancel, |e| {
        if let SearchEvent::Hit(h) = e {
            if seen_at_first_hit.is_none() {
                seen_at_first_hit = Some(pulled.get());
            }
            if h.page == 1 {
                cancel.cancel();
            }
        }
    });
    assert_eq!(seen_at_first_hit, Some(1));
    assert!(sum.cancelled);
    assert_eq!(sum.hits, 2);
    assert_eq!(pulled.get(), 2, "no page pulled after cancel");
    assert!(sum.first_hit_at.is_some());

    let o = SearchOptions {
        max_hits: Some(3),
        ..opts()
    };
    let mut n = 0;
    let sum = search_document(pages(10), "needle", &o, &CancelToken::new(), |e| {
        if matches!(e, SearchEvent::Hit(_)) {
            n += 1
        }
    });
    assert_eq!((n, sum.hits, sum.truncated), (3, 3, true));

    // pre-cancelled token
    let c = CancelToken::new();
    c.cancel();
    let sum = search_document(pages(3), "needle", &opts(), &c, |_| panic!());
    assert!(sum.cancelled && sum.pages_searched == 0);

    // PageDone events carry counts
    let mut done = vec![];
    search_document(pages(2), "a", &opts(), &CancelToken::new(), |e| {
        if let SearchEvent::PageDone { page, hits } = e {
            done.push((page, hits))
        }
    });
    assert_eq!(done, [(0, 3), (1, 3)]);
}

#[test]
fn random_input_never_panics_and_ranges_are_valid() {
    let pool: Vec<char> = "ab Zz\n\r-\u{ad}\u{2}ﬁßİıé\u{301}\u{5b8}שלם مرحبا\u{64e}日本ｱ한\u{11a8}\u{200f}\u{a0}\u{3099}か"
        .chars()
        .collect();
    let mut rng = Rng(42);
    for round in 0..300 {
        let n = 1 + (rng.next() % 60) as usize;
        let text: String = (0..n)
            .map(|_| pool[(rng.next() % pool.len() as u64) as usize])
            .collect();
        let q: String = (0..1 + rng.next() % 3)
            .map(|_| pool[(rng.next() % pool.len() as u64) as usize])
            .collect();
        let o = SearchOptions {
            whole_word: round % 2 == 0,
            case_sensitive: round % 3 == 0,
            diacritic_insensitive: round % 5 == 0,
            turkish_case: round % 7 == 0,
            bidi: if round % 4 == 0 {
                BidiOrder::Visual
            } else {
                BidiOrder::Logical
            },
            ..opts()
        };
        for h in run(&text, &q, &o) {
            assert!(
                h.range.start < h.range.end && h.range.end <= n,
                "{text:?} {q:?} {:?}",
                h.range
            );
        }
    }
}
