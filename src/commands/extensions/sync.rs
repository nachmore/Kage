//! Tauri commands an extension's `context.sync` calls
//! (docs/design/SYNC.md §12.6, §19.5). Capability `sync`; the sandbox host
//! force-injects `extensionId`.
//!
//! They check the call's size and the id on the calling thread, then ask
//! the engine thread (10-second timeout) from a blocking thread, never a
//! Tokio worker. The engine answers between the steps of a tick, so a push
//! waits for at most one shard merge. With sync off or not built, every
//! one answers `sync_unavailable`.

use super::*;
use crate::sync::sources::extensions::{MAX_PULL, MAX_PUSH_BYTES, MAX_PUSH_RECORDS};
use crate::sync::{
    unavailable, EngineReply, EngineRequest, ExtReply, ExtRequest, ExtSyncStatus, PullResult,
    PushResult,
};

/// How long a call waits for the engine thread.
const EXT_SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

fn invalid(key: &str) -> AppError {
    AppError::keyed(ErrorKind::InvalidInput, key, &[])
}

fn check_id(extension_id: &str) -> Result<(), AppError> {
    extensions::validate_extension_id(extension_id)
        .map_err(|_| invalid("errors.sync.ext_bad_request"))
}

/// The size limits of one push (§12.7): 500 records and ids, 4 MiB.
pub(crate) fn check_push(
    collection: &str,
    records: &[serde_json::Value],
    deleted: &[String],
) -> Result<(), AppError> {
    if collection.len() > 64 || records.len() + deleted.len() > MAX_PUSH_RECORDS {
        return Err(invalid("errors.sync.ext_push_too_large"));
    }
    let bytes: usize = records
        .iter()
        .map(|r| serde_json::to_vec(r).map_or(usize::MAX, |b| b.len()))
        .fold(0usize, usize::saturating_add)
        .saturating_add(deleted.iter().map(String::len).sum::<usize>());
    if bytes > MAX_PUSH_BYTES {
        return Err(invalid("errors.sync.ext_push_too_large"));
    }
    Ok(())
}

/// Ask the engine thread, from a blocking thread.
async fn ask(req: ExtRequest) -> Result<ExtReply, AppError> {
    let Some(handle) = crate::sync::scheduler::global_handle() else {
        return Err(unavailable());
    };
    let reply = tauri::async_runtime::spawn_blocking(move || {
        handle.request(EngineRequest::Ext(req), EXT_SYNC_TIMEOUT)
    })
    .await
    .map_err(|e| crate::sync::engine::internal(anyhow::anyhow!("{e}")))??;
    match reply {
        EngineReply::Ext(r) => Ok(r),
        _ => Err(unavailable()),
    }
}

/// §12.6 `status()`.
#[tauri::command]
pub async fn ext_sync_status(extension_id: String) -> Result<ExtSyncStatus, AppError> {
    check_id(&extension_id)?;
    match ask(ExtRequest::Status { ext: extension_id }).await? {
        ExtReply::Status(s) => Ok(s),
        _ => Err(unavailable()),
    }
}

/// Push records of `collection` after the extension's own write, and the
/// ids it deleted. `initial`: the first push on this computer.
#[tauri::command]
pub async fn ext_sync_push(
    extension_id: String,
    collection: String,
    records: Vec<serde_json::Value>,
    deleted: Vec<String>,
    initial: bool,
) -> Result<PushResult, AppError> {
    check_id(&extension_id)?;
    check_push(&collection, &records, &deleted)?;
    match ask(ExtRequest::Push {
        ext: extension_id,
        collection,
        records,
        deleted,
        initial,
    })
    .await?
    {
        ExtReply::Push(r) => Ok(r),
        _ => Err(unavailable()),
    }
}

/// Up to `max` changes since the extension's last ack.
#[tauri::command]
pub async fn ext_sync_pull(extension_id: String, max: usize) -> Result<PullResult, AppError> {
    check_id(&extension_id)?;
    match ask(ExtRequest::Pull {
        ext: extension_id,
        max: max.clamp(1, MAX_PULL),
    })
    .await?
    {
        ExtReply::Pull(r) => Ok(r),
        _ => Err(unavailable()),
    }
}

/// The extension applied everything up to `cursor`.
#[tauri::command]
pub async fn ext_sync_ack(extension_id: String, cursor: String) -> Result<(), AppError> {
    check_id(&extension_id)?;
    if cursor.is_empty() || cursor.len() > 20 || !cursor.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("errors.sync.bad_cursor"));
    }
    match ask(ExtRequest::Ack {
        ext: extension_id,
        cursor,
    })
    .await?
    {
        ExtReply::Ack => Ok(()),
        _ => Err(unavailable()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn a_push_over_the_call_limits_is_refused_before_the_engine() {
        let one = vec![serde_json::json!({ "id": "a" })];
        assert!(check_push("items", &one, &[]).is_ok());
        let many: Vec<serde_json::Value> = (0..MAX_PUSH_RECORDS + 1)
            .map(|i| serde_json::json!({ "id": format!("r{i}") }))
            .collect();
        let e = check_push("items", &many, &[]).unwrap_err();
        assert_eq!(e.key, "errors.sync.ext_push_too_large");
        let ids: Vec<String> = (0..MAX_PUSH_RECORDS).map(|i| format!("r{i}")).collect();
        assert!(
            check_push("items", &one, &ids).is_err(),
            "records and deletes count together"
        );
        let big = vec![serde_json::json!({ "id": "a", "text": "x".repeat(MAX_PUSH_BYTES) })];
        assert!(check_push("items", &big, &[]).is_err());
    }

    #[test]
    fn calls_answer_unavailable_without_an_engine_and_check_their_arguments() {
        let kind = |e: AppError| serde_json::to_value(&e).unwrap()["kind"].clone();
        assert_eq!(
            kind(block_on(ext_sync_status("Bad Id".into())).unwrap_err()),
            "invalid_input"
        );
        assert_eq!(
            kind(block_on(ext_sync_ack("mindmap".into(), "12a".into())).unwrap_err()),
            "invalid_input"
        );
        // No engine handle is installed in unit tests.
        if crate::sync::scheduler::global_handle().is_none() {
            assert_eq!(
                kind(block_on(ext_sync_status("mindmap".into())).unwrap_err()),
                "sync_unavailable"
            );
            assert_eq!(
                kind(block_on(ext_sync_pull("mindmap".into(), 10)).unwrap_err()),
                "sync_unavailable"
            );
        }
    }
}
