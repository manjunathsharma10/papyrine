#![no_main]

use libfuzzer_sys::fuzz_target;
use papyrine_cos::{DecodeLevel, Document, ObjectKind, ObjectStreams, OpenOptions, WriteOptions};

fuzz_target!(|data: &[u8]| {
    // Cap the input so one iteration stays fast.
    if data.len() > 1 << 20 {
        return;
    }
    for password in [None, Some("password")] {
        let opts = match password {
            Some(p) => OpenOptions::with_password(p),
            None => OpenOptions::default(),
        };
        let Ok(doc) = Document::open_bytes(data.to_vec(), &opts) else { continue };

        let _ = doc.repair_log();
        let _ = doc.encryption();
        if let Ok(n) = doc.page_count() {
            for i in 0..n.min(8) {
                if let Ok(page) = doc.page(i) {
                    let _ = page.unparse();
                    if let Ok(c) = page.dict_get("Contents")
                        && c.kind().ok() == Some(ObjectKind::Stream)
                        && let Ok(b) = c.stream_decoded(DecodeLevel::Generalized)
                    {
                        let _ = b.as_slice().len();
                    }
                }
            }
        }
        if let Ok(ids) = doc.object_ids() {
            for id in ids.into_iter().take(64) {
                if let Ok(o) = doc.object(id) {
                    let _ = doc.fingerprint(&o);
                }
            }
        }
        let _ = doc.write(&WriteOptions {
            object_streams: ObjectStreams::Generate,
            static_id: true,
            ..WriteOptions::default()
        });
    }
});
