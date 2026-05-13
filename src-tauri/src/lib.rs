pub mod commands;
pub mod db;
pub mod scan;
pub mod volume;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let database = db::Database::open(app_data_dir.join("winclean.sqlite3"))?;
            database.migrate()?;
            app.manage(commands::AppState::new(database));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_volumes,
            commands::start_scan,
            commands::scan_status,
            commands::top_directories,
            commands::cancel_scan
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
