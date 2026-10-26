pub mod cleanup;
pub mod optimizer;
pub mod commands;
pub mod db;
pub mod db_optimizer;
pub mod db_storage;
pub mod duplicates;
pub mod offload;
pub mod pathutil;
pub mod recommendations;
pub mod reports;
pub mod scan;
pub mod storage_ai;
pub mod storage_policy;
pub mod volume;

use tauri::Manager;

/// Initialize app storage, finalize interrupted jobs, and run Tauri with registered commands.
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

            // WinClean 2.0 optimizer: managed state, crash recovery, and the
            // event-driven background runtime (all off the UI thread).
            let optimizer_state = optimizer::state::OptimizerState::system();
            let telemetry: optimizer::telemetry::SharedTelemetry = std::sync::Arc::new(optimizer::telemetry::TelemetryService::system());
            let recovery = std::sync::Arc::new(optimizer::commands::RecoveryHolder::default());
            {
                let (db, backends, holder) = (database.clone(), optimizer_state.backends.clone(), recovery.clone());
                std::thread::spawn(move || {
                    let ctx = optimizer::orchestrator::Ctx { db: &db, backends: &backends };
                    let report = optimizer::recovery::recover_at_startup(&ctx);
                    *holder.0.lock().unwrap_or_else(|p| p.into_inner()) = report;
                });
            }
            optimizer::runtime::start(app.handle().clone(), database.clone(), optimizer_state.backends.clone(), telemetry.clone());
            let native = match app.get_webview_window("main") {
                Some(window) => optimizer::commands::apply_native_effects(&window),
                None => optimizer::commands::NativeState { mica: false },
            };
            app.manage(native);
            app.manage(optimizer_state);
            app.manage(telemetry);
            app.manage(recovery);
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
            commands::cancel_scan,
            commands::reveal_in_explorer,
            optimizer::commands::optimizer_list_services,
            optimizer::commands::optimizer_service_recommendations,
            optimizer::commands::optimizer_prepare_service_plan,
            optimizer::commands::optimizer_apply_service_plan,
            optimizer::commands::optimizer_set_service_start_type,
            optimizer::commands::optimizer_helper_status,
            optimizer::commands::optimizer_helper_connect,
            optimizer::commands::optimizer_helper_disconnect,
            optimizer::commands::optimizer_list_processes,
            optimizer::commands::optimizer_set_process_ecoqos,
            optimizer::commands::optimizer_workload,
            optimizer::commands::optimizer_battery_status,
            optimizer::commands::optimizer_battery_telemetry,
            optimizer::commands::optimizer_telemetry_history,
            optimizer::commands::optimizer_power_history,
            optimizer::commands::optimizer_telemetry_lease,
            optimizer::commands::optimizer_list_profiles,
            optimizer::commands::optimizer_apply_power_profile,
            optimizer::commands::optimizer_save_custom_profile,
            optimizer::commands::optimizer_delete_custom_profile,
            optimizer::commands::optimizer_set_auto_switch,
            optimizer::commands::optimizer_measurement_start,
            optimizer::commands::optimizer_measurement_switch,
            optimizer::commands::optimizer_measurement_finish,
            optimizer::commands::optimizer_measurement_list,
            optimizer::commands::optimizer_hardware_info,
            optimizer::commands::optimizer_capabilities,
            optimizer::commands::optimizer_mcp_status,
            optimizer::commands::optimizer_mcp_verify,
            optimizer::commands::optimizer_mcp_provision_agent_user,
            optimizer::commands::optimizer_mcp_cancel,
            optimizer::commands::optimizer_mcp_search_roots,
            optimizer::commands::optimizer_mcp_add_search_root,
            optimizer::commands::optimizer_mcp_remove_search_root,
            optimizer::commands::optimizer_mcp_verify_permission,
            optimizer::commands::optimizer_mcp_search,
            optimizer::commands::optimizer_mcp_file_details,
            optimizer::commands::optimizer_history,
            optimizer::commands::optimizer_rollback,
            optimizer::commands::optimizer_recovery_report,
            optimizer::commands::native_info
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
