pub mod cleaner;
pub mod detectors;
pub mod safety;
pub mod scanner;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub app_name: String,
    pub version: String,
    pub engine_ready: bool,
    pub modules: Vec<String>,
}

pub fn build_app_status() -> AppStatus {
    AppStatus {
        app_name: String::from("NEXØRA"),
        version: String::from(env!("CARGO_PKG_VERSION")),
        engine_ready: true,
        modules: vec![
            String::from(scanner::MODULE_ID),
            String::from(detectors::MODULE_ID),
            String::from(cleaner::MODULE_ID),
        ],
    }
}

#[tauri::command]
fn get_app_status() -> AppStatus {
    build_app_status()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(scanner::commands::ScannerState::default())
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            scanner::commands::list_scan_targets,
            scanner::commands::start_scan,
            scanner::commands::cancel_scan,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_app_status_returns_expected_metadata_and_modules() {
        let status = build_app_status();
        assert_eq!(status.app_name, "NEXØRA");
        assert_eq!(status.version, env!("CARGO_PKG_VERSION"));
        assert!(status.engine_ready);
        assert_eq!(
            status.modules,
            vec![
                String::from("scanner"),
                String::from("detectors"),
                String::from("cleaner"),
            ]
        );
    }

    #[test]
    fn app_status_serializes_to_camel_case_json() {
        let json = serde_json::to_value(build_app_status()).expect("serialize AppStatus");
        assert_eq!(json["appName"], "NEXØRA");
        assert_eq!(json["engineReady"], true);
        assert_eq!(
            json["modules"],
            serde_json::json!(["scanner", "detectors", "cleaner"])
        );
    }
}
