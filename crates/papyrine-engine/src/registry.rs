//! The engine's command registry: the built-in organise commands of `papyrine-ops` plus the
//! registration functions of the feature crates.
//!
//! `papyrine-annotate` and `papyrine-fill` expose `fn register(&mut CommandRegistry)`; they are
//! wired in here as they land (a path dependency on a missing crate would break the whole
//! workspace, so the calls are added with the crates).

use papyrine_ops::CommandRegistry;

pub fn default_registry() -> CommandRegistry {
    #[allow(unused_mut)]
    let mut r = CommandRegistry::with_builtin();
    // papyrine_annotate::register(&mut r);
    // papyrine_fill::register(&mut r);
    #[cfg(feature = "test-commands")]
    test_commands::register(&mut r);
    r
}

/// Commands that misbehave on purpose, for crash-recovery and quarantine tests that run the
/// engine as a real child process.
#[cfg(feature = "test-commands")]
pub mod test_commands {
    use papyrine_ops::{ChangeSet, Command, CommandRegistry, EditContext, LocalizedText, Result};
    use serde_json::{Value, json};

    /// `test_crash`: edits a page, then kills the process like a segfault would.
    /// `test_sleep {ms}`: sleeps, then does nothing (a long-running request).
    pub struct Misbehave {
        pub kind: &'static str,
        pub ms: u64,
    }

    impl Command for Misbehave {
        fn name(&self) -> &'static str {
            self.kind
        }
        fn describe(&self) -> LocalizedText {
            LocalizedText::new(format!("cmd.{}", self.kind.replace('_', "-")))
        }
        fn params(&self) -> Value {
            json!({"ms": self.ms})
        }
        fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
            if self.kind == "test_sleep" {
                std::thread::sleep(std::time::Duration::from_millis(self.ms));
                return cx.changeset();
            }
            let doc = cx.doc();
            let page = doc.page(0)?;
            cx.set_key(&page, "Rotate", &doc.new_int(90))?;
            std::process::abort();
        }
    }

    /// `test_probe`: tries things a sandboxed engine must not be able to do and reports each as
    /// `allowed` or `denied` in the error message (the only channel a command has).
    pub struct Probe;

    impl Command for Probe {
        fn name(&self) -> &'static str {
            "test_probe"
        }
        fn describe(&self) -> LocalizedText {
            LocalizedText::new("cmd.test-probe")
        }
        fn params(&self) -> Value {
            Value::Null
        }
        fn apply(&mut self, _cx: &mut EditContext<'_>) -> Result<ChangeSet> {
            let ad = |ok: bool| if ok { "allowed" } else { "denied" };
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            let read_home = std::fs::read_dir(&home).is_ok();
            let outside = std::env::current_dir()
                .unwrap_or_default()
                .join("papyrine-probe.tmp");
            let write_outside = std::fs::write(&outside, b"x").is_ok();
            if write_outside {
                let _ = std::fs::remove_file(&outside);
            }
            let inside = std::env::temp_dir().join("papyrine-probe-ok.tmp");
            let write_temp = std::fs::write(&inside, b"x").is_ok();
            let _ = std::fs::remove_file(&inside);
            let socket = std::net::UdpSocket::bind("127.0.0.1:0").is_ok();
            let spawn = std::process::Command::new(if cfg!(windows) { "cmd" } else { "/bin/echo" })
                .output()
                .is_ok();
            Err(papyrine_ops::Error::invalid(format!(
                "probe read_home={} write_outside_temp={} write_temp={} socket={} spawn={}",
                ad(read_home),
                ad(write_outside),
                ad(write_temp),
                ad(socket),
                ad(spawn)
            )))
        }
    }

    pub fn register(r: &mut CommandRegistry) {
        r.register("test_probe", |_, _| Ok(Box::new(Probe)));
        r.register("test_crash", |_, _| {
            Ok(Box::new(Misbehave {
                kind: "test_crash",
                ms: 0,
            }))
        });
        r.register("test_sleep", |_, p| {
            Ok(Box::new(Misbehave {
                kind: "test_sleep",
                ms: p["ms"].as_u64().unwrap_or(0),
            }))
        });
    }
}
