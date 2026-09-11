//! Desktop integration that needs a native dialog or message box.

use std::path::PathBuf;

/// Shows a blocking error to the user. Used for failures before the window
/// exists, because the Windows build has no console to print to.
pub fn fatal(message: &str) {
    log::error!("fatal: {message}");
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Scriblet could not start")
            .set_description(message)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        eprintln!("Scriblet could not start: {message}");
    }
}

fn default_export_name() -> String {
    format!(
        "scriblet-export-{}.{}",
        chrono::Local::now().format("%Y-%m-%d"),
        textcompletion::transfer::FILE_EXTENSION
    )
}

/// Asks where to write an export. Returns `None` when the user cancels.
pub fn pick_export_path() -> Option<PathBuf> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Export Scriblet snippets")
            .add_filter(
                "Scriblet export",
                &[textcompletion::transfer::FILE_EXTENSION],
            )
            .set_file_name(default_export_name());
        if let Some(dir) =
            directories::UserDirs::new().and_then(|d| d.document_dir().map(PathBuf::from))
        {
            dialog = dialog.set_directory(dir);
        }
        dialog.save_file()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let dir = directories::UserDirs::new()
            .and_then(|d| d.document_dir().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        Some(dir.join(default_export_name()))
    }
}

/// Asks which export to import. Returns `None` when the user cancels or no
/// dialog is available on this platform.
pub fn pick_import_path() -> Option<PathBuf> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Import Scriblet snippets")
            .add_filter(
                "Scriblet export",
                &[textcompletion::transfer::FILE_EXTENSION],
            );
        if let Some(dir) =
            directories::UserDirs::new().and_then(|d| d.document_dir().map(PathBuf::from))
        {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_file()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        None
    }
}

pub fn tray_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}
