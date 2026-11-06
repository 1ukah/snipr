#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Ponte fina entre a interface (HTML/CSS/JS sem framework) e o `Engine`.
//! Nenhum comando aqui devolve o valor de um segredo.

use std::{collections::HashMap, path::PathBuf, sync::Arc};

use snipr_core::{secrets, CliStatus, Engine, Provider, SearchEvent, SearchRequest, Settings, Site, Suggestion, Theme};
use tauri::{window::Color, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

struct AppState {
    engine: Arc<Engine>,
}

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Settings {
    state.engine.settings()
}

#[tauri::command]
async fn save_settings(state: State<'_, AppState>, settings: Settings) -> Res<()> {
    state.engine.save_settings(settings).await.map_err(err)
}

#[tauri::command]
fn list_sites(state: State<'_, AppState>) -> Vec<Site> {
    state.engine.sites()
}

#[tauri::command]
fn list_suggestions(state: State<'_, AppState>) -> Vec<Suggestion> {
    state.engine.suggestions()
}

#[tauri::command]
async fn add_site(state: State<'_, AppState>, url: String, name: Option<String>, manual: bool) -> Res<Site> {
    state.engine.add_site(&url, name, manual).await.map_err(err)
}

#[tauri::command]
async fn check_site_login(state: State<'_, AppState>, id: String) -> Res<Site> {
    state.engine.check_site_login(&id).await.map_err(err)
}

#[tauri::command]
fn remove_site(state: State<'_, AppState>, id: String) -> Res<()> {
    state.engine.remove_site(&id).map_err(err)
}

#[tauri::command]
fn secret_status(state: State<'_, AppState>, names: Vec<String>) -> HashMap<String, bool> {
    names
        .into_iter()
        .filter(|n| secrets::is_known(n))
        .map(|n| {
            let has = state.engine.has_secret(&n);
            (n, has)
        })
        .collect()
}

#[tauri::command]
fn set_secret(state: State<'_, AppState>, name: String, value: String) -> Res<()> {
    state.engine.set_secret(&name, &value).map_err(err)
}

#[tauri::command]
fn delete_secret(state: State<'_, AppState>, name: String) -> Res<()> {
    state.engine.delete_secret(&name).map_err(err)
}

#[tauri::command]
async fn ai_status(state: State<'_, AppState>) -> Res<Vec<CliStatus>> {
    Ok(state.engine.ai_status().await)
}

#[tauri::command]
fn open_ai_login(state: State<'_, AppState>, provider: Provider) -> Res<()> {
    state.engine.open_ai_login(provider).map_err(err)
}

#[tauri::command]
async fn list_models(state: State<'_, AppState>, provider: Provider, endpoint: String) -> Res<Vec<String>> {
    state.engine.list_models(provider, &endpoint).await.map_err(err)
}

#[tauri::command]
fn start_search(app: tauri::AppHandle, state: State<'_, AppState>, req: SearchRequest) -> Res<String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SearchEvent>();
    let job = state.engine.start_search(req, tx).map_err(err)?;
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app.emit("snipr-event", &ev);
        }
    });
    Ok(job)
}

#[tauri::command]
fn cancel_search(state: State<'_, AppState>, job: String) {
    state.engine.cancel(&job);
}

#[tauri::command]
async fn cancel_all(state: State<'_, AppState>) -> Res<()> {
    state.engine.cancel_all().await;
    Ok(())
}

#[tauri::command]
fn resolve_human(state: State<'_, AppState>, agent: String, ok: bool) -> bool {
    state.engine.resolve_human(&agent, ok)
}

#[tauri::command]
async fn open_login(state: State<'_, AppState>, site: String) -> Res<()> {
    state.engine.open_login(&site).await.map_err(err)
}

#[tauri::command]
async fn close_login(state: State<'_, AppState>, done: bool) -> Res<()> {
    state.engine.close_login(done).await;
    Ok(())
}

/// Pasta de dados local do Snipr (mesma usada pelo core).
fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("snipr")
}

/// No build release não há console: um erro na inicialização seria invisível.
/// Gravamos `crash.log` e mostramos uma caixa de mensagem nativa.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = format!("{info}");
        let log = data_dir().join("crash.log");
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(&log, format!("{msg}\n\n{}", std::backtrace::Backtrace::force_capture()));
        show_error(&format!(
            "O Snipr encontrou um erro e precisa fechar.\n\n{}\n\nDetalhes em: {}",
            msg.lines().take(4).collect::<Vec<_>>().join("\n"),
            log.display()
        ));
        default(info);
    }));
}

#[cfg(windows)]
fn show_error(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide("Snipr"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

#[cfg(not(windows))]
fn show_error(text: &str) {
    eprintln!("{text}");
}

/// Cada execução usa sua própria pasta do WebView2. Assim, processos órfãos de
/// uma execução anterior (ex.: após um crash) nunca bloqueiam a próxima.
/// A UI não guarda nada no WebView, então nada se perde. Pastas antigas são
/// apagadas quando já não estão em uso.
fn webview_data_dir() -> PathBuf {
    let base = data_dir().join("webview");
    if let Ok(entries) = std::fs::read_dir(&base) {
        for e in entries.flatten() {
            let _ = std::fs::remove_dir_all(e.path()); // falha em silêncio se estiver em uso
        }
    }
    base.join(format!("run-{}", std::process::id()))
}

fn main() {
    install_panic_hook();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "snipr=info,snipr_core=info,snipr_browser=info".into()),
        )
        .init();

    tauri::Builder::default()
        // Abrir o Snipr de novo só traz a janela existente para frente.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let engine = tauri::async_runtime::block_on(Engine::new())?;
            let background = match engine.settings().theme {
                Theme::Light => Color(244, 244, 243, 255),
                _ => Color(13, 13, 14, 255),
            };
            app.manage(AppState { engine });
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Snipr")
                .inner_size(1440.0, 900.0)
                .min_inner_size(960.0, 640.0)
                .background_color(background)
                .data_directory(webview_data_dir())
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            list_sites,
            list_suggestions,
            add_site,
            remove_site,
            check_site_login,
            secret_status,
            set_secret,
            delete_secret,
            ai_status,
            open_ai_login,
            list_models,
            start_search,
            cancel_search,
            cancel_all,
            resolve_human,
            open_login,
            close_login,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                let engine = window.state::<AppState>().engine.clone();
                tauri::async_runtime::block_on(engine.shutdown());
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o Snipr");
}
