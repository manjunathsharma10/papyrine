//! Second-instance forwarding without a plugin: the first process listens on a localhost
//! socket and records `{port, token}` in the app-data dir (owner-only); a later launch
//! connects, hands over its file arguments and exits. The token keeps other local
//! users or programs from injecting open requests.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Lock {
    port: u16,
    token: String,
    pid: u32,
}

pub enum Instance {
    /// We are the first instance; poll `listener.accept` through [`serve`].
    Primary(TcpListener, String),
    /// Another instance took the arguments; this process should exit.
    Forwarded,
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join("instance.json")
}

fn try_forward(dir: &Path, files: &[String]) -> bool {
    let Ok(bytes) = std::fs::read(lock_path(dir)) else {
        return false;
    };
    let Ok(lock) = serde_json::from_slice::<Lock>(&bytes) else {
        return false;
    };
    let Ok(mut s) = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], lock.port)),
        Duration::from_millis(500),
    ) else {
        return false;
    };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let msg = serde_json::json!({"token": lock.token, "files": files}).to_string();
    if writeln!(s, "{msg}").is_err() {
        return false;
    }
    // The primary answers "ok" once it has queued the request.
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).is_ok() && line.trim() == "ok"
}

/// Become the primary instance, or forward `files` to the running one.
pub fn acquire(data_dir: &Path, files: &[String]) -> std::io::Result<Instance> {
    std::fs::create_dir_all(data_dir)?;
    if try_forward(data_dir, files) {
        return Ok(Instance::Forwarded);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let token = crate::util::random_hex(16);
    let lock = Lock {
        port: listener.local_addr()?.port(),
        token: token.clone(),
        pid: std::process::id(),
    };
    let tmp = lock_path(data_dir).with_extension("tmp");
    write_private(&tmp, &serde_json::to_vec(&lock)?)?;
    std::fs::rename(&tmp, lock_path(data_dir))?;
    Ok(Instance::Primary(listener, token))
}

fn write_private(p: &Path, data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(p)?;
        f.write_all(data)
    }
    #[cfg(not(unix))]
    {
        // The app-data dir is per-user on Windows.
        std::fs::write(p, data)
    }
}

/// Accept forwarded launches forever; `on_files` gets each batch of file arguments.
pub fn serve(
    listener: TcpListener,
    token: String,
    on_files: impl Fn(Vec<String>) + Send + 'static,
) {
    let _ = std::thread::Builder::new()
        .name("papyrine-instance".into())
        .spawn(move || {
            for mut s in listener.incoming().flatten() {
                let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(&s).read_line(&mut line).is_err() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if v.get("token").and_then(|t| t.as_str()) != Some(token.as_str()) {
                    continue;
                }
                let files: Vec<String> = v
                    .get("files")
                    .and_then(|f| serde_json::from_value(f.clone()).ok())
                    .unwrap_or_default();
                on_files(files);
                let _ = writeln!(s, "ok");
            }
        });
}

/// Remove our lock file on a clean exit (a stale one is harmless: connecting fails).
pub fn release(data_dir: &Path) {
    let _ = std::fs::remove_file(lock_path(data_dir));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn second_launch_forwards_its_files_and_exits() {
        let dir = tempfile::tempdir().unwrap();
        let Instance::Primary(l, tok) = acquire(dir.path(), &[]).unwrap() else {
            panic!("first launch is primary");
        };
        let (tx, rx) = mpsc::channel();
        serve(l, tok.clone(), move |f| {
            let _ = tx.send(f);
        });
        let r = acquire(dir.path(), &["/a/b.pdf".into(), "/c.pdf".into()]).unwrap();
        assert!(matches!(r, Instance::Forwarded));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            vec!["/a/b.pdf".to_string(), "/c.pdf".to_string()]
        );
        // A wrong token is ignored.
        let lock: Lock =
            serde_json::from_slice(&std::fs::read(lock_path(dir.path())).unwrap()).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", lock.port)).unwrap();
        writeln!(s, r#"{{"token":"nope","files":["/evil.pdf"]}}"#).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        // A stale lock (no listener) makes the next launch primary again.
        release(dir.path());
        std::fs::write(
            lock_path(dir.path()),
            serde_json::to_vec(&Lock {
                port: 1,
                token: "x".into(),
                pid: 0,
            })
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            acquire(dir.path(), &[]).unwrap(),
            Instance::Primary(..)
        ));
    }
}
