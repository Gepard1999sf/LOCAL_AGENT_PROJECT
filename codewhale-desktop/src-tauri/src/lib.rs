mod api;
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
