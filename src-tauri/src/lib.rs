use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cdj_core::convert::{run_jobs, JobEvent, Summary};
use cdj_core::ffmpeg::Ffmpeg;
use cdj_core::plan::{plan_jobs, ConvertSettings, Job};
use cdj_core::scan::{analyze_all, scan};
use cdj_core::{builtin_profiles, find_profile, CdjProfile, FileReport};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

#[derive(Default)]
struct AppState {
    ffmpeg: Mutex<Option<Ffmpeg>>,
    cancel: Arc<AtomicBool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FfmpegInfo {
    path: String,
    version: String,
    has_soxr: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeResult {
    reports: Vec<FileReport>,
    rekordbox_roots: Vec<PathBuf>,
}

#[derive(Clone, Serialize)]
struct Progress {
    done: usize,
    total: usize,
}

fn profile(id: &str) -> Result<CdjProfile, String> {
    find_profile(id).ok_or_else(|| format!("Modelo de CDJ desconocido: {id}"))
}

/// FFmpeg que viene dentro de la app (Tauri lo deja junto al ejecutable).
fn bundled_ffmpeg_candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| vec![d.join(exe)]))
        .unwrap_or_default()
}

fn ffmpeg(state: &AppState) -> Result<Ffmpeg, String> {
    let mut guard = state.ffmpeg.lock().unwrap();
    if guard.is_none() {
        *guard = Ffmpeg::locate(&bundled_ffmpeg_candidates());
    }
    guard
        .clone()
        .ok_or_else(|| "No se encontró FFmpeg. Reinstala la aplicación.".to_string())
}

#[tauri::command]
fn get_profiles() -> Vec<CdjProfile> {
    builtin_profiles()
}

#[tauri::command]
fn ffmpeg_info(state: State<'_, AppState>) -> Option<FfmpegInfo> {
    ffmpeg(&state).ok().map(|f| FfmpegInfo {
        path: f.path.display().to_string(),
        version: f.version,
        has_soxr: f.has_soxr,
    })
}

/// Carpeta de salida sugerida: Música/CDJ Converter/<nombre de lo que se agregó> (<modelo>).
#[tauri::command]
fn default_output_dir(
    app: AppHandle,
    paths: Vec<PathBuf>,
    profile_id: String,
) -> Result<PathBuf, String> {
    let base = app
        .path()
        .audio_dir()
        .or_else(|_| app.path().home_dir())
        .map_err(|e| e.to_string())?;
    let first = paths.first().ok_or("Sin archivos")?;
    let name = if first.is_dir() {
        first.file_name()
    } else {
        first.parent().and_then(|p| p.file_name())
    }
    .map(|n| n.to_string_lossy().into_owned())
    .unwrap_or_else(|| "USB".into());
    let model = profile(&profile_id)?.name;
    Ok(base.join("CDJ Converter").join(format!("{name} ({model})")))
}

#[tauri::command]
async fn analyze(
    app: AppHandle,
    paths: Vec<PathBuf>,
    profile_id: String,
    exclude: Option<PathBuf>,
) -> Result<AnalyzeResult, String> {
    let profile = profile(&profile_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let scanned = scan(&paths, exclude.as_deref());
        let reports = analyze_all(&scanned.entries, &profile, |done, total| {
            if done == total || done % 25 == 0 {
                let _ = app.emit("analyze-progress", Progress { done, total });
            }
        });
        AnalyzeResult {
            reports,
            rekordbox_roots: scanned.rekordbox_roots,
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn preview_plan(
    reports: Vec<FileReport>,
    profile_id: String,
    settings: ConvertSettings,
) -> Result<Vec<Job>, String> {
    plan_jobs(&reports, &profile(&profile_id)?, &settings).map_err(|e| e.to_string())
}

#[tauri::command]
async fn convert(
    app: AppHandle,
    state: State<'_, AppState>,
    reports: Vec<FileReport>,
    profile_id: String,
    settings: ConvertSettings,
) -> Result<Summary, String> {
    let profile = profile(&profile_id)?;
    let ffmpeg = ffmpeg(&state)?;
    let jobs = plan_jobs(&reports, &profile, &settings).map_err(|e| e.to_string())?;
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1))
        .unwrap_or(2)
        .clamp(1, 6);
    tauri::async_runtime::spawn_blocking(move || {
        run_jobs(&jobs, &ffmpeg, threads, &cancel, |event: JobEvent| {
            let _ = app.emit("convert-progress", event);
        })
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_convert(state: State<'_, AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_profiles,
            ffmpeg_info,
            default_output_dir,
            analyze,
            preview_plan,
            convert,
            cancel_convert
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar la aplicación");
}
