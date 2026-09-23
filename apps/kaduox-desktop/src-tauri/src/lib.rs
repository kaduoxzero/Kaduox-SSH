mod ai_store;
mod command_classify;
mod commands;
mod credentials;
mod history;
mod models;
mod state;
mod util;

use commands::ai::{
    ai_chat, ai_classify_command, ai_conv_append, ai_conv_create, ai_conv_delete, ai_conv_list,
    ai_conv_messages, ai_conv_rename, ai_execute_command, ai_key_status, ai_models,
    delete_ai_api_key, save_ai_api_key,
};
use commands::connection::{
    clear_history, connect_host, delete_stored_password, disconnect_host, execute_command,
    list_command_history, list_history, list_sessions, record_terminal_command,
};
use commands::files::{
    create_remote_directory, create_remote_file, delete_remote_path, download_file,
    download_remote_directory, list_remote_files, read_remote_file, remote_directory_size,
    remote_home_directory,
    rename_remote_path, upload_file, write_remote_file,
};
use commands::forward::{list_forwards, start_forward, stop_forward};
use commands::help::open_help_link;
use commands::hosts::{
    delete_chain, delete_folder, delete_host, list_chains, list_folders, list_hosts, save_chain,
    save_folder, save_host,
};
use commands::info::{
    get_local_basic_info, get_local_system_metrics, query_basic_info, query_system_metrics,
};
use commands::terminal::{close_terminal, resize_terminal, start_terminal, terminal_write};
use state::DesktopState;

/// The window layout differs by platform: Windows/Linux keep the frameless
/// custom-drawn titlebar, while macOS uses the native traffic-light buttons
/// over an overlay titlebar so the window follows platform conventions.
fn build_main_window(app: &tauri::App) -> tauri::Result<()> {
    let builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::default())
        .title("Kaduox SSH")
        .inner_size(1440.0, 900.0)
        .min_inner_size(1024.0, 680.0)
        .center()
        .resizable(true)
        .fullscreen(false)
        .visible(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false).shadow(true);

    builder.build()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            build_main_window(app)?;
            Ok(())
        })
        .manage(DesktopState::default())
        .invoke_handler(tauri::generate_handler![
            ai_models,
            open_help_link,
            ai_chat,
            ai_key_status,
            save_ai_api_key,
            delete_ai_api_key,
            ai_classify_command,
            ai_execute_command,
            ai_conv_create,
            ai_conv_list,
            ai_conv_messages,
            ai_conv_append,
            ai_conv_rename,
            ai_conv_delete,
            list_hosts,
            list_folders,
            save_folder,
            delete_folder,
            list_chains,
            save_host,
            save_chain,
            delete_chain,
            delete_host,
            connect_host,
            disconnect_host,
            list_sessions,
            delete_stored_password,
            start_terminal,
            terminal_write,
            resize_terminal,
            close_terminal,
            execute_command,
            list_history,
            list_command_history,
            record_terminal_command,
            clear_history,
            remote_home_directory,
            list_remote_files,
            remote_directory_size,
            upload_file,
            download_file,
            download_remote_directory,
            create_remote_directory,
            create_remote_file,
            read_remote_file,
            write_remote_file,
            rename_remote_path,
            delete_remote_path,
            start_forward,
            list_forwards,
            stop_forward,
            get_local_basic_info,
            query_basic_info,
            get_local_system_metrics,
            query_system_metrics,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Kaduox SSH desktop application");
}
