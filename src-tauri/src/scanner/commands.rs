use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, State};

use crate::scanner::engine::{scan_path, ProgressSink};
use crate::scanner::types::{ScanProgressEvent, ScanRequest, ScanResult, ScanTargets};
use crate::scanner::win_fs::{list_drives, list_user_folders};

pub const SCAN_PROGRESS_EVENT: &str = "scanner://progress";

#[derive(Default)]
pub struct ScannerState {
    active_scans: Mutex<HashMap<String, Arc<AtomicBool>>>,
    next_id: AtomicU64,
}

impl ScannerState {
    pub fn register(
        &self,
        requested_id: Option<String>,
    ) -> Result<(String, Arc<AtomicBool>), String> {
        let custom_id = requested_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let mut guard = self
            .active_scans
            .lock()
            .map_err(|_| String::from("No se pudo acceder al estado de escaneos activos."))?;

        let scan_id = match custom_id {
            Some(id) => {
                if guard.contains_key(&id) {
                    return Err(format!("Ya existe un escaneo activo con id '{id}'."));
                }
                id
            }
            None => loop {
                let seq = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                let candidate = format!("scan-{seq}");
                if !guard.contains_key(&candidate) {
                    break candidate;
                }
            },
        };

        let cancel_token = Arc::new(AtomicBool::new(false));
        guard.insert(scan_id.clone(), Arc::clone(&cancel_token));
        Ok((scan_id, cancel_token))
    }

    pub fn cancel(&self, scan_id: Option<&str>) -> bool {
        let Ok(guard) = self.active_scans.lock() else {
            return false;
        };

        if let Some(id) = scan_id {
            if let Some(token) = guard.get(id) {
                token.store(true, Ordering::SeqCst);
                return true;
            }
            return false;
        }

        if guard.is_empty() {
            return false;
        }
        for token in guard.values() {
            token.store(true, Ordering::SeqCst);
        }
        true
    }

    pub fn unregister(&self, scan_id: &str, cancel_token: &Arc<AtomicBool>) {
        if let Ok(mut guard) = self.active_scans.lock() {
            if let Some(existing) = guard.get(scan_id) {
                if Arc::ptr_eq(existing, cancel_token) {
                    guard.remove(scan_id);
                }
            }
        }
    }
}

struct TauriProgressSink {
    app: AppHandle,
    cancel_token: Arc<AtomicBool>,
}

impl ProgressSink for TauriProgressSink {
    fn on_progress(&self, event: &ScanProgressEvent) {
        if self.app.emit(SCAN_PROGRESS_EVENT, event.clone()).is_err() {
            self.cancel_token.store(true, Ordering::Relaxed);
        }
    }
}

#[tauri::command]
pub fn list_scan_targets() -> Result<ScanTargets, String> {
    Ok(ScanTargets {
        drives: list_drives(),
        user_folders: list_user_folders(),
    })
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, ScannerState>,
    request: ScanRequest,
) -> Result<ScanResult, String> {
    let trimmed_path = request.path.trim();
    if trimmed_path.is_empty() {
        return Err(String::from("La ruta de escaneo no puede estar vacía."));
    }

    let (scan_id, cancel_token) = state.register(request.scan_id)?;
    let options = request.options.unwrap_or_default();
    let target_path = PathBuf::from(trimmed_path);

    let sink = TauriProgressSink {
        app,
        cancel_token: Arc::clone(&cancel_token),
    };

    let scan_id_for_task = scan_id.clone();
    let token_for_task = Arc::clone(&cancel_token);
    let join_result = tauri::async_runtime::spawn_blocking(move || {
        scan_path(
            scan_id_for_task,
            &target_path,
            options,
            token_for_task,
            &sink,
        )
    })
    .await;

    state.unregister(&scan_id, &cancel_token);

    match join_result {
        Ok(res) => res,
        Err(err) => Err(format!("Error interno ejecutando el escaneo: {err}")),
    }
}

#[tauri::command]
pub fn cancel_scan(
    state: State<'_, ScannerState>,
    scan_id: Option<String>,
) -> Result<bool, String> {
    Ok(state.cancel(scan_id.as_deref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_state_registers_cancels_and_unregisters_tokens() {
        let state = ScannerState::default();
        let (id1, tok1) = state.register(None).expect("register auto id");
        let (id2, tok2) = state
            .register(Some(String::from("custom-scan")))
            .expect("register custom id");

        assert_ne!(id1, id2);
        assert_eq!(id2, "custom-scan");
        assert!(!tok1.load(Ordering::Relaxed));
        assert!(!tok2.load(Ordering::Relaxed));

        assert!(state.cancel(Some("custom-scan")));
        assert!(tok2.load(Ordering::Relaxed));
        assert!(!tok1.load(Ordering::Relaxed));

        assert!(!state.cancel(Some("non-existent")));

        assert!(state.cancel(None));
        assert!(tok1.load(Ordering::Relaxed));

        state.unregister(&id1, &tok1);
        state.unregister(&id2, &tok2);
        assert!(!state.cancel(None));
    }

    #[test]
    fn scanner_state_rejects_duplicate_active_id_and_guards_unregister_by_ptr() {
        let state = ScannerState::default();
        let (id, tok1) = state
            .register(Some(String::from("dup-scan")))
            .expect("initial registration succeeds");

        let dup_err = state
            .register(Some(String::from("dup-scan")))
            .expect_err("duplicate active scan_id must be rejected");
        assert!(dup_err.contains("dup-scan"));
        assert!(!tok1.load(Ordering::Relaxed));

        // Stale or mismatched token cannot unregister the active scan
        let stale_token = Arc::new(AtomicBool::new(false));
        state.unregister(&id, &stale_token);
        assert!(state.cancel(Some("dup-scan")));
        assert!(tok1.load(Ordering::Relaxed));

        // Matching token unregisters cleanly, allowing subsequent reuse of the ID
        state.unregister(&id, &tok1);
        assert!(!state.cancel(Some("dup-scan")));
        let (_id_reused, tok2) = state
            .register(Some(String::from("dup-scan")))
            .expect("id should be reusable after unregister");
        state.unregister("dup-scan", &tok2);
    }
}
