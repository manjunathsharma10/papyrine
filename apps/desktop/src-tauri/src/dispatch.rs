//! One entry point for every UI method (`src/ipc/contract.ts` `HostApi`): the Tauri
//! command and the dev bridge both call [`call`] with the contract method name and its
//! positional arguments. Tiles and raw byte uploads are the only things that do not go
//! through here (they are binary).

use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::{EngineCommand, SaveOptions, SearchOptions};
use crate::broker::Broker;
use crate::error::{Code, HostErr, Result};

fn arg<T: DeserializeOwned>(args: &[Value], i: usize, method: &str) -> Result<T> {
    let v = args.get(i).cloned().unwrap_or(Value::Null);
    serde_json::from_value(v)
        .map_err(|e| HostErr::internal(format!("{method}: bad argument {i}: {e}")))
}

fn opt<T: DeserializeOwned>(args: &[Value], i: usize, method: &str) -> Result<Option<T>> {
    match args.get(i) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| HostErr::internal(format!("{method}: bad argument {i}: {e}"))),
    }
}

fn to<T: serde::Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

fn unavailable(what: &str) -> HostErr {
    HostErr::internal(format!("{what} is not available in this build yet."))
}

/// Run one contract method. Unknown names are an error, not a panic.
pub fn call(b: &Broker, method: &str, args: &[Value]) -> Result<Value> {
    let m = method;
    match method {
        // --- Files ---------------------------------------------------------------
        "showOpenDialog" => {
            let sources: Vec<Value> = b
                .show_open_dialog()
                .into_iter()
                .map(|p| json!({"kind": "path", "path": p.to_string_lossy()}))
                .collect();
            Ok(json!({ "sources": sources }))
        }
        "openDocument" => {
            let src: Value = arg(args, 0, m)?;
            let password: Option<String> = opt(args, 1, m)?;
            match src.get("kind").and_then(Value::as_str) {
                Some("path") => {
                    let p = src
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| HostErr::internal("openDocument: missing path"))?;
                    to(b.open_path(&PathBuf::from(p), password)?)
                }
                _ => Err(HostErr::internal(
                    "openDocument: byte sources are sent with openBytes",
                )),
            }
        }
        "closeDocument" => {
            b.close_document(&arg::<String>(args, 0, m)?)?;
            Ok(Value::Null)
        }
        "recentFiles" => to(b.recent_files()),
        "clearRecentFiles" => {
            b.clear_recent_files();
            Ok(Value::Null)
        }
        "pickPdfs" => {
            let o: Value = arg(args, 0, m)?;
            let multiple = o.get("multiple").and_then(Value::as_bool).unwrap_or(false);
            to(b.pick_pdfs(multiple)?)
        }
        "pickSavePath" => {
            let o: Value = arg(args, 0, m)?;
            let name = o
                .get("suggestedName")
                .and_then(Value::as_str)
                .unwrap_or("document.pdf");
            let dir = o.get("directory").and_then(Value::as_bool).unwrap_or(false);
            Ok(b.pick_save_path(name, dir)
                .map_or(Value::Null, |p| json!(p.to_string_lossy())))
        }
        "reloadFromDisk" => to(b.reload_from_disk(&arg::<String>(args, 0, m)?)?),
        "keepMine" => {
            b.acknowledge_external(&arg::<String>(args, 0, m)?)?;
            Ok(Value::Null)
        }
        "resolveActionPrompt" => {
            b.resolve_action_prompt(
                &arg::<String>(args, 0, m)?,
                &arg::<String>(args, 1, m)?,
                arg::<bool>(args, 2, m)?,
            )?;
            Ok(Value::Null)
        }

        // --- Reading -------------------------------------------------------------
        "getPageInfo" => to(b.get_page_info(&arg::<String>(args, 0, m)?, arg(args, 1, m)?)?),
        "getOutline" => to(b.get_outline(&arg::<String>(args, 0, m)?)?),
        "getPageText" => to(b.get_page_text(&arg::<String>(args, 0, m)?, arg(args, 1, m)?)?),

        // --- Search --------------------------------------------------------------
        "search" => {
            let opts: SearchOptions = arg(args, 2, m)?;
            to(b.search(
                &arg::<String>(args, 0, m)?,
                &arg::<String>(args, 1, m)?,
                opts,
            )?)
        }
        "cancelJob" => {
            b.cancel_job(&arg::<String>(args, 0, m)?);
            Ok(Value::Null)
        }

        // --- Annotations and forms: the engine does not register those commands yet.
        // Reads answer "none" so the panes stay empty instead of failing.
        "listAnnotations" | "getFormWidgets" => Ok(json!([])),
        "setShowAnnotations" => {
            let id: String = arg(args, 0, m)?;
            let s = b.session_info(&id)?;
            Ok(json!({"info": s, "invalidatedPages": []}))
        }
        "fieldKeystroke" => {
            let req: Value = arg(args, 2, m)?;
            let change = req.get("change").and_then(Value::as_str).unwrap_or("");
            Ok(json!({"accept": true, "value": change}))
        }

        // --- Editing -------------------------------------------------------------
        "execute" => {
            let cmd: EngineCommand = arg(args, 1, m)?;
            to(b.execute(&arg::<String>(args, 0, m)?, &cmd)?)
        }
        "undo" => to(b.undo(&arg::<String>(args, 0, m)?)?),
        "redo" => to(b.redo(&arg::<String>(args, 0, m)?)?),
        "save" => {
            let o: SaveOptions = opt(args, 1, m)?.unwrap_or_default();
            to(b.save(&arg::<String>(args, 0, m)?, &o)?)
        }
        "disableOptimizeSuggestion" => {
            b.disable_optimize_suggestion();
            Ok(Value::Null)
        }

        // --- New-file operations ---------------------------------------------------
        "runFileOp" => Err(unavailable("Extract, merge and split")),

        // --- Recovery --------------------------------------------------------------
        "recoveryEntries" => to(b.recovery_entries()),
        "recover" => to(b.recover(&arg::<String>(args, 0, m)?, &arg::<String>(args, 1, m)?)?),
        "resolveUnfinished" => {
            let redo = arg::<String>(args, 1, m)? == "redo";
            b.resolve_unfinished_for(&arg::<String>(args, 0, m)?, redo)?;
            Ok(Value::Null)
        }

        // --- Privacy and updates ---------------------------------------------------
        "getPrivacy" => Ok(b.privacy()),
        "setUpdateCheck" => b.set_update_check(&arg::<String>(args, 0, m)?),
        "checkForUpdates" => {
            if b.privacy()["updateCheck"] != "on" {
                return Err(HostErr::new(
                    Code::Rejected,
                    "Turn the update check on first.",
                ));
            }
            Err(unavailable("The update check"))
        }
        "downloadUpdate" => Err(unavailable("Downloading updates")),

        // --- Printing ---------------------------------------------------------------
        "getPrintSetup" => Ok(json!({"supported": false, "printers": []})),
        "print" => Err(unavailable("Printing")),

        // --- Host extras (not in the contract) --------------------------------------
        "requestAction" => Ok(json!(b.request_action(
            &arg::<String>(args, 0, m)?,
            &arg::<String>(args, 1, m)?,
            &arg::<String>(args, 2, m)?
        )?)),
        "notifyFocus" => {
            b.check_external_all();
            Ok(Value::Null)
        }
        "takePendingOpens" => Ok(json!(
            b.take_pending_opens()
                .into_iter()
                .map(|p| json!({"kind": "path", "path": p.to_string_lossy()}))
                .collect::<Vec<_>>()
        )),
        "hostStats" => Ok(b.stats()),
        _ => Err(HostErr::internal(format!("unknown host method {method:?}"))),
    }
}
