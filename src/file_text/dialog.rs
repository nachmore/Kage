//! The native file dialog for `pick_file_text`
//! (docs/design/EXTENSION_FILE_TEXT.md §4.2, §5.3).
//!
//! The title is Kage's own string with the extension's manifest name; no
//! extension text reaches the OS dialog. One filter lists the requested
//! formats' extensions, with no "All files" entry. Kage gives no starting
//! folder, so the OS opens where it last was.
//!
//! From the launcher and inline assist (always-on-top overlays that hide
//! on blur) the JS host hides the window first, and the dialog opens with
//! no parent after Kage is made the active app. From every other window
//! the dialog is attached to it: a sheet on macOS, an owned modal on
//! Windows, a transient dialog on Linux.

use super::Code;
use crate::error::AppError;
use crate::window_labels;
use kage_core::file_text_wire::Format;
use std::path::PathBuf;
use tauri_plugin_dialog::DialogExt;

/// What the dialog shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogRequest {
    pub title: String,
    pub filter_name: String,
    /// Every extension of the requested formats, without dots.
    pub extensions: Vec<&'static str>,
}

impl DialogRequest {
    pub fn new(display_name: &str, formats: &[Format]) -> Self {
        Self {
            title: crate::t!("file_text.dialog.title", "extension" => display_name),
            filter_name: crate::t!("file_text.dialog.filter"),
            extensions: formats
                .iter()
                .flat_map(|f| f.extensions().iter().copied())
                .collect(),
        }
    }
}

/// Where the picked path comes from: the native dialog in the app, a
/// fixed path in tests.
pub trait DialogSource: Send + Sync {
    /// Shows the dialog and waits. `Ok(None)` when the user picked nothing.
    fn pick(&self, request: &DialogRequest) -> Result<Option<PathBuf>, AppError>;
}

/// Whether a dialog from the window `label` is attached to it. The
/// launcher and inline assist get a free dialog instead.
pub fn attaches_to(label: &str) -> bool {
    label != window_labels::FLOATING && label != window_labels::INLINE_ASSIST
}

/// The OS dialog, through `tauri-plugin-dialog`.
pub struct NativeDialog<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    window: tauri::WebviewWindow<R>,
}

impl<R: tauri::Runtime> NativeDialog<R> {
    pub fn new(app: tauri::AppHandle<R>, window: tauri::WebviewWindow<R>) -> Self {
        Self { app, window }
    }
}

impl<R: tauri::Runtime> DialogSource for NativeDialog<R> {
    /// Blocks; call it on a blocking thread.
    fn pick(&self, request: &DialogRequest) -> Result<Option<PathBuf>, AppError> {
        let mut dialog = self
            .app
            .dialog()
            .file()
            .set_title(&request.title)
            .add_filter(&request.filter_name, &request.extensions);
        if attaches_to(self.window.label()) {
            dialog = dialog.set_parent(&self.window);
        } else {
            // macOS: the launcher is a non-activating panel, so Kage may
            // not be the active app, and a dialog from an inactive app
            // comes up without keyboard focus (as for `pick_folder`).
            crate::setup::activate_app(&self.app);
        }
        match dialog.blocking_pick_file() {
            None => Ok(None),
            Some(picked) => picked
                .into_path()
                .map(Some)
                .map_err(|_| Code::DialogFailed.error(&[])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlays_get_a_free_dialog_and_other_windows_a_parented_one() {
        assert!(!attaches_to(window_labels::FLOATING));
        assert!(!attaches_to(window_labels::INLINE_ASSIST));
        assert!(attaches_to(window_labels::MAIN));
        assert!(attaches_to(window_labels::SETTINGS));
        assert!(attaches_to("chat-1234"));
        assert!(attaches_to("ext-view:mindmap:explorer"));
    }

    #[test]
    fn the_request_names_the_extension_and_lists_only_requested_extensions() {
        crate::i18n::init(Some("en"));
        let d = DialogRequest::new("Mind Map", &[Format::Markdown, Format::Pdf]);
        assert_eq!(d.title, "Mind Map: choose a file to read");
        assert_eq!(d.filter_name, "Supported files");
        assert_eq!(d.extensions, vec!["md", "markdown", "mdown", "mkd", "pdf"]);
    }
}
