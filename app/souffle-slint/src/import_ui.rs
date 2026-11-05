//! Meeting Notes AI: "Import audio…" on the home page (`AudioImportRow`).
//!
//! Shows the native open panel, then imports the chosen files one after
//! another on a worker thread (`souffle_lib::import`), reporting progress
//! to the home page and refreshing the history after every imported file.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use souffle_lib::import::{ImportProgress, ImportStage};
use souffle_lib::state::AppState;

use slint::ComponentHandle;

use crate::MainWindow;

fn stage_index(stage: ImportStage) -> i32 {
    match stage {
        ImportStage::Decoding => 0,
        ImportStage::LoadingModel => 1,
        ImportStage::Transcribing => 2,
        ImportStage::Saving => 3,
    }
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn on_ui(weak: &slint::Weak<MainWindow>, f: impl FnOnce(&MainWindow) + Send + 'static) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            f(&window);
        }
    });
}

/// Import `paths` one after another; blocking, runs on its own thread.
fn run_imports(
    weak: slint::Weak<MainWindow>,
    state: Arc<AppState>,
    paths: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
) {
    let count = paths.len();
    let mut imported = 0i32;
    let mut errors: Vec<String> = Vec::new();
    for (position, path) in paths.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = file_label(&path);
        {
            let name = name.clone();
            on_ui(&weak, move |w| {
                w.set_import_file_name(name.into());
                w.set_import_file_index(i32::try_from(position + 1).unwrap_or(i32::MAX));
                w.set_import_file_count(i32::try_from(count).unwrap_or(i32::MAX));
                w.set_import_stage(0);
                w.set_import_percent(0);
            });
        }
        let progress_weak = weak.clone();
        let should_cancel = || cancel.load(Ordering::Relaxed);
        let on_progress = move |progress: ImportProgress| {
            let percent = (progress.fraction.clamp(0.0, 1.0) * 100.0).round() as i32;
            on_ui(&progress_weak, move |w| {
                w.set_import_stage(stage_index(progress.stage));
                w.set_import_percent(percent);
            });
        };
        match souffle_lib::import::import_audio_file(&state, &path, &should_cancel, &on_progress) {
            Ok(_meeting) => {
                imported += 1;
                let state = state.clone();
                on_ui(&weak, move |w| crate::refresh_timeline(w, &state));
            }
            Err(error) if error == souffle_lib::import::CANCELED => break,
            Err(error) => {
                eprintln!("Import of {name} failed: {error}");
                errors.push(if count > 1 {
                    format!("{name}: {error}")
                } else {
                    error
                });
            }
        }
    }
    let error = errors.join("\n");
    on_ui(&weak, move |w| {
        w.set_import_busy(false);
        w.set_import_imported_count(imported);
        w.set_import_error(error.into());
    });
}

pub(crate) fn wire(window: &MainWindow, state: Arc<AppState>) {
    let cancel = Arc::new(AtomicBool::new(false));

    let weak = window.as_weak();
    let cancel_start = cancel.clone();
    window.on_import_requested(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if window.get_import_busy() {
            return;
        }
        window.set_import_error("".into());
        window.set_import_imported_count(0);
        let weak = weak.clone();
        let state = state.clone();
        let cancel = cancel_start.clone();
        // The open panel runs modally; like the export's save panel it is
        // shown from a fresh event loop turn, not inside this callback.
        let spawned = slint::spawn_local(async move {
            let picked = crate::run_on_main_thread(|| {
                souffle_lib::native::dialog::pick_audio_files(
                    souffle_lib::import::SUPPORTED_EXTENSIONS,
                )
            })
            .await;
            let paths = match picked {
                Ok(paths) => paths,
                Err(error) => {
                    if let Some(w) = weak.upgrade() {
                        w.set_import_error(error.into());
                    }
                    return;
                }
            };
            if paths.is_empty() {
                return;
            }
            let Some(w) = weak.upgrade() else {
                return;
            };
            cancel.store(false, Ordering::Relaxed);
            w.set_import_busy(true);
            w.set_import_stage(0);
            w.set_import_percent(0);
            let worker_weak = weak.clone();
            let spawn = std::thread::Builder::new()
                .name("audio-import".into())
                .spawn(move || run_imports(worker_weak, state, paths, cancel));
            if let Err(error) = spawn {
                w.set_import_busy(false);
                w.set_import_error(format!("Could not start the import: {error}").into());
            }
        });
        if let Err(error) = spawned {
            eprintln!("Could not show the import panel: {error}");
        }
    });

    let cancel_stop = cancel.clone();
    window.on_import_cancel_requested(move || {
        cancel_stop.store(true, Ordering::Relaxed);
    });

    let weak = window.as_weak();
    window.on_import_dismissed(move || {
        if let Some(window) = weak.upgrade() {
            window.set_import_error("".into());
            window.set_import_imported_count(0);
        }
    });
}
