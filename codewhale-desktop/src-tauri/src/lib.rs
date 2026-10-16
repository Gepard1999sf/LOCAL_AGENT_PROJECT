mod api;
mod comfy;
pub mod comfy_mcp;
mod drive;
pub mod drive_mcp;
pub mod mcp_core;
mod models;
mod ollama;
mod runtime;
mod settings;
mod system;
mod winproc;

use tauri::{Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(runtime::Runtime::new())
        .manage(api::Subscriptions::default())
        .manage(models::PullRegistry::default())
        .manage(drive::DriveProcs::default())
        .manage(comfy::ComfyProcs::default())
        .setup(|app| {
            let settings = settings::Settings::load(app.handle());
            app.manage(settings);
            // Start CodeWhale in the background; the UI shows progress via `runtime-status`.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let rt = handle.state::<runtime::Runtime>();
                let _ = rt.ensure_started(&handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            settings::settings_get,
            settings::settings_set,
            runtime::runtime_start,
            runtime::runtime_restart,
            runtime::runtime_status,
            runtime::runtime_logs,
            api::cw_request,
            api::cw_subscribe,
            api::cw_unsubscribe,
            ollama::ollama_status,
            ollama::ollama_models,
            ollama::ollama_unload,
            ollama::ollama_create_variant,
            models::models_pull,
            models::models_pull_cancel,
            models::models_pulls_active,
            models::models_delete,
            models::models_copy,
            models::models_show,
            models::models_running,
            models::models_estimate_vram,
            drive::drive_status,
            drive::drive_register_mcp,
            drive::drive_unregister_mcp,
            drive::drive_set_ollama_models,
            drive::drive_restart_ollama,
            drive::drive_index_build,
            drive::drive_kiwix_start,
            drive::drive_kiwix_stop,
            drive::drive_install_skills,
            drive::drive_install_helpers,
            drive::drive_golden_models,
            comfy::comfy_status,
            comfy::comfy_start,
            comfy::comfy_stop,
            comfy::comfy_register_mcp,
            comfy::comfy_unregister_mcp,
            comfy::comfy_install_templates,
            comfy::comfy_free,
            comfy::comfy_generate,
            system::gpu_stats,
            system::git_branch,
            system::path_info,
            system::codewhale_home_path,
            system::open_in_editor,
            system::write_export,
            system::read_image_b64,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            let h = handle.clone();
            tauri::async_runtime::block_on(async move {
                h.state::<runtime::Runtime>().stop(&h).await;
            });
        }
    });
}
