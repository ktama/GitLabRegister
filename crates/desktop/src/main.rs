// 窓のない起動にする。コンソールが一緒に開くのを防ぐ。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! GitLabRegister の Tauri 外殻 (設計書 4.2 UI 層)。
//!
//! 画面は WebView2 上の Web UI で、計算と入出力はコマンド境界へ委ねる。
//! この外殻が担うのは、窓の起動と WebView2 の封じ込めである (設計書 8.3)。

mod clock;
mod commands;
mod state;

use gitlabregister_infra::paths::AppPaths;

fn main() {
    // WebView2 のユーザーデータフォルダを設定フォルダ配下へ固定する (NFR-05、NFR-09)。
    //
    // 既定では実行ファイルの隣か OS の既定の場所に作られる。
    // 環境変数による指定は実装側の指定を上書きするため、ここで最初に設定する。
    // WebView を初期化した後に設定しても効かない。
    let paths = match AppPaths::from_environment() {
        Ok(paths) => paths,
        Err(error) => {
            // 保存先を決められない状態では続けられない。
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = paths.ensure_created() {
        eprintln!("設定フォルダを作成できません: {error}");
        std::process::exit(1);
    }
    // SAFETY: WebView の初期化より前、他の走行列が動く前に 1 回だけ設定する。
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", paths.webview_data_dir());
    }

    let app_state = state::AppState::new(paths);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::load_settings,
            commands::save_settings,
            commands::connection_state,
            commands::connect,
            commands::disconnect,
            commands::list_schedules,
            commands::create_schedule,
            commands::update_schedule,
            commands::duplicate_schedule,
            commands::set_schedule_enabled,
            commands::delete_schedule_confirmation,
            commands::delete_schedule,
            commands::pattern_change_confirmation,
            commands::expand,
            commands::resolve_preset,
            commands::register_batch,
            commands::retry_failed,
            commands::register_single,
            commands::cancel_registration,
            commands::is_registering,
            commands::list_projects,
            commands::add_project,
            commands::project_delete_confirmation,
            commands::delete_project,
            commands::list_holidays,
            commands::add_holiday,
            commands::delete_holiday,
            commands::import_holidays,
            commands::export_holidays,
            commands::export_schedules,
            commands::import_schedules,
            commands::list_history,
            commands::history_delete_confirmation,
            commands::delete_history,
            commands::candidates,
            commands::refetch_candidates,
            commands::open_issue,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri の起動に失敗しました");
}
