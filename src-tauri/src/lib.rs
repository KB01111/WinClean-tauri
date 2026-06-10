pub mod cleanup;
pub mod commands;
pub mod db;
pub mod offload;
pub mod recommendations;
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
            database.finalize_interrupted_offload_jobs()?;
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
            commands::open_recycle_bin,
            commands::delete_scan_run,
            commands::clear_scan_history,
            commands::recommendations,
            commands::google_drive_status,
            commands::begin_google_drive_auth,
            commands::disconnect_google_drive,
            commands::start_offload,
            commands::offload_status,
            commands::cancel_offload,
            commands::list_offload_jobs,
            commands::active_offload_job,
            commands::restore_offload_item,
            commands::largest_stale_files,
            commands::open_url,
            commands::ai_key_status,
            commands::set_ai_api_key,
            commands::clear_ai_api_key,
            commands::get_ai_api_key,
            commands::export_report,
            commands::export_diagnostics,
            commands::app_health,
            commands::cancel_scan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
