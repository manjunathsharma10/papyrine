use papyrine_content::{Object, Op, SerializeOptions, ops_equal, parse, serialize};
use proptest::prelude::*;

fn arb_real() -> impl Strategy<Value = Object> {
    // Up to 4 decimals, so both lossless and default formatting preserve it.
    (-2_000_000i64..2_000_000, 1u32..10_000).prop_map(|(a, b)| {
        let v = a as f64 / 10_000.0 + b as f64 / 1e8 * 0.0;
        Object::from(v)
    })
}

fn arb_leaf() -> impl Strategy<Value = Object> {
    prop_oneof![
        Just(Object::Null),
        any::<bool>().prop_map(Object::Bool),
        any::<i32>().prop_map(|i| Object::Int(i64::from(i))),
        any::<i64>().prop_map(|i| Object::Int(
            i.clamp(-9_000_000_000_000_000_000, 9_000_000_000_000_000_000)
        )),
        arb_real(),
        proptest::collection::vec(any::<u8>(), 0..24).prop_map(Object::Str),
        proptest::collection::vec(any::<u8>(), 0..12).prop_map(Object::Name),
    ]
}

fn arb_object() -> impl Strategy<Value = Object> {
    arb_leaf().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Object::Array),
            proptest::collection::vec((proptest::collection::vec(any::<u8>(), 0..6), inner), 0..3)
                .prop_map(Object::Dict),
        ]
    })
}

fn arb_operator() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("q".to_string()),
        Just("Q".to_string()),
        Just("cm".to_string()),
        Just("Tj".to_string()),
        Just("TJ".to_string()),
        Just("T*".to_string()),
        Just("'".to_string()),
        Just("\"".to_string()),
        Just("b*".to_string()),
        Just("BDC".to_string()),
        "[A-Za-z][A-Za-z0-9*']{0,3}".prop_filter("reserved words", |s| {
            !matches!(s.as_str(), "true" | "false" | "null" | "BI" | "ID" | "EI")
        }),
    ]
}

fn arb_op() -> impl Strategy<Value = Op> {
    (
        arb_operator(),
        proptest::collection::vec(arb_object(), 0..7),
    )
        .prop_map(|(name, operands)| Op::new(&name, operands))
}

fn arb_inline_image() -> impl Strategy<Value = Op> {
    (
        1i64..8,
        1i64..8,
        proptest::collection::vec(any::<u8>(), 0..64),
        any::<bool>(),
    )
        .prop_map(|(w, h, mut data, sized)| {
            let mut dict = vec![
                (b"W".to_vec(), Object::Int(w)),
                (b"H".to_vec(), Object::Int(h)),
                (b"BPC".to_vec(), Object::Int(8)),
                (b"CS".to_vec(), Object::name("G")),
            ];
            if sized {
                data.resize((w * h) as usize, 7);
            } else {
                dict.push((b"F".to_vec(), Object::name("Fl")));
                // Avoid whitespace + "EI" + ASCII, which is inherently ambiguous
                // without a length: keep the first byte binary.
                data.insert(0, 0x80);
                data.retain(|&b| b != b'E' && b != b'I');
            }
            Op {
                operator: b"BI".to_vec(),
                operands: vec![Object::Dict(dict)],
                inline_data: Some(data),
                span: 0..0,
            }
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn generated_ops_roundtrip(ops in proptest::collection::vec(prop_oneof![9 => arb_op(), 1 => arb_inline_image()], 0..12)) {
        for opts in [SerializeOptions::lossless(), SerializeOptions::default(), SerializeOptions { newline_between_ops: false, ..Default::default() }] {
            let bytes = serialize(&ops, &opts);
            let back = parse(&bytes);
            prop_assert!(back.errors.is_empty(), "errors {:?} in {:?}", back.errors, String::from_utf8_lossy(&bytes));
            prop_assert!(ops_equal(&ops, &back.ops), "{:?}\nvs\n{:?}\nbytes {:?}", ops, back.ops, String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic_and_are_idempotent(data in proptest::collection::vec(any::<u8>(), 0..400)) {
        let a = parse(&data);
        let bytes = serialize(&a.ops, &SerializeOptions::lossless());
        let b = parse(&bytes);
        prop_assert!(ops_equal(&a.ops, &b.ops), "{:?}\nvs\n{:?}", a.ops, b.ops);
        // Spans stay inside the input.
        for op in &a.ops {
            prop_assert!(op.span.start <= op.span.end && op.span.end <= data.len());
        }
    }

    #[test]
    fn tokenish_garbage_roundtrips(parts in proptest::collection::vec(prop_oneof![
        Just("("), Just(")"), Just("<"), Just(">"), Just("<<"), Just(">>"), Just("["), Just("]"),
        Just("/N"), Just("1"), Just("-.5"), Just("4."), Just("Tj"), Just("BI"), Just("ID"), Just("EI"),
        Just("q"), Just("%c\n"), Just("true"), Just("\\"), Just("{"), Just("#"), Just(" "), Just("\n"),
    ], 0..60)) {
        let src = parts.join(" ");
        let a = parse(src.as_bytes());
        let bytes = serialize(&a.ops, &SerializeOptions::lossless());
        let b = parse(&bytes);
        prop_assert!(ops_equal(&a.ops, &b.ops), "src {:?}\n{:?}\nvs\n{:?}", src, a.ops, b.ops);
    }
}
