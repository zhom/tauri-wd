#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_cross_platform_webdriver::init())
        .run(tauri::generate_context!())
        .expect("failed to run WebDriver fixture");
}
