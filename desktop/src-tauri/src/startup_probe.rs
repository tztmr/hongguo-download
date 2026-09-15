//! Installer verification uses the normal native setup with fresh, isolated data.
use crate::{ensure_api_ready, AppState};
use std::{fs, path::PathBuf, thread};
use tauri::{AppHandle, Manager};

pub(crate) fn directory() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--startup-probe")) {
        return None;
    }
    let path = PathBuf::from(
        args.next()
            .expect("startup probe requires a new absolute directory"),
    );
    assert!(
        path.is_absolute() && args.next().is_none(),
        "invalid startup probe arguments"
    );
    // Refuse existing directories so verification cannot load or alter user data.
    fs::create_dir(&path).expect("startup probe directory must not already exist");
    Some(path)
}

pub(crate) fn start(app: AppHandle, directory: PathBuf) {
    thread::spawn(move || {
        let state = app.state::<AppState>();
        let result = ensure_api_ready(&state.client, &state.api_base, &state.api_ready);
        let report = match &result {
            Ok(()) => {
                serde_json::json!({"status": "ok", "version": app.package_info().version.to_string()})
            }
            Err(error) => serde_json::json!({"status": "error", "error": error}),
        };
        let saved = fs::write(directory.join("result.json"), report.to_string()).is_ok();
        // The probe runner stops the application tree after reading the result.
        // Keep the parent alive so Windows can also terminate PyInstaller children.
        if !saved {
            app.exit(1);
        }
    });
}
