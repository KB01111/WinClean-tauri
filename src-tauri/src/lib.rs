pub mod cleanup;
pub mod commands;
pub mod db;
pub mod reports;
pub mod scan;
pub mod volume;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let database = db::Database::open(app_data_dir.join("winclean.sqlite3"))?;
            database.migrate()?;
            database.finalize_interrupted_scan_runs()?;
            app.manage(commands::AppState::new(database, app_data_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_volumes,
            commands::start_scan,
            commands::scan_status,
            commands::scan_runs,
            commands::top_directories,
            commands::directory_children,
            commands::file_search,
            commands::extension_summary,
            commands::category_summary,
            commands::cleanup_plan,
            commands::execute_cleanup,
            commands::cleanup_history,
            commands::restore_cleanup_action,
            commands::export_report,
            commands::export_diagnostics,
            commands::app_health,
            commands::cancel_scan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
