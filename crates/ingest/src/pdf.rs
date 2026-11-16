//! PDF access through pdfium: page count, text layer, and page rendering.
//!
//! `libpdfium` comes from `OPENBOOK_PDFIUM_PATH`, or from the directory of the executable.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::ImageFormat;
use openbook_search::quality::needs_transcription;
use pdfium_render::prelude::*;
use tokio::sync::mpsc::Sender;

use crate::transcribe::MAX_RENDER_SIDE;

/// Why a PDF cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PdfError {
    #[error("the PDF could not be opened")]
    Unreadable,
    #[error("the PDF is password protected")]
    Password,
    #[error("the PDF has {pages} pages, and the limit is {limit}")]
    TooManyPages { pages: u32, limit: u32 },
    /// The pdfium library is missing or broken. This is a deployment problem, not the owner's.
    #[error("pdfium is not available: {0}")]
    Library(String),
}

/// One page, as the producer reads it.
#[derive(Debug, Clone)]
pub struct PageData {
    /// 1-based.
    pub number: u32,
    /// The text layer, cleaned.
    pub text: String,
    pub has_images: bool,
    /// The rendered page (PNG). Only present when the text layer looks poor.
    pub png: Option<Vec<u8>>,
}

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// The directory with the pdfium library: the setting, or the directory of the executable.
pub fn library_dir(configured: Option<&Path>) -> PathBuf {
    configured.map(Path::to_path_buf).unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

/// pdfium starts once per process.
fn pdfium(dir: &Path) -> Result<&'static Pdfium, PdfError> {
    let loaded = PDFIUM.get_or_init(|| {
        let library = Pdfium::pdfium_platform_library_name_at_path(dir);
        Pdfium::bind_to_library(&library)
            .map(Pdfium::new)
            .map_err(|e| format!("cannot load {}: {e}", library.display()))
    });
    loaded.as_ref().map_err(|e| PdfError::Library(e.clone()))
}

fn open<'a>(pdfium: &'a Pdfium, path: &Path) -> Result<PdfDocument<'a>, PdfError> {
    pdfium.load_pdf_from_file(path, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            PdfError::Password
        }
        _ => PdfError::Unreadable,
    })
}

/// Opens the PDF and returns its page count. Refuses more than `max_pages`.
pub fn page_count(dir: &Path, path: &Path, max_pages: u32) -> Result<u32, PdfError> {
    let pdfium = pdfium(dir)?;
    let document = open(pdfium, path)?;
    let pages = u32::try_from(document.pages().len()).unwrap_or(u32::MAX);
    if pages > max_pages {
        return Err(PdfError::TooManyPages {
            pages,
            limit: max_pages,
        });
    }
    Ok(pages)
}

/// Cleans a text layer: line breaks inside a paragraph become spaces, and blank lines stay.
pub fn clean_text(raw: &str) -> String {
    let normalized = raw
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "");
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in normalized.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs.join("\n\n")
}

/// Reads the pages that are not in `skip` and sends them to `tx` in page order.
/// A page whose text layer looks poor is rendered to PNG as well.
/// It stops quietly when the receiver is gone. Call it on a blocking thread.
pub fn extract_pages(
    dir: &Path,
    path: &Path,
    skip: &HashSet<u32>,
    tx: &Sender<PageData>,
) -> Result<(), PdfError> {
    let pdfium = pdfium(dir)?;
    let document = open(pdfium, path)?;
    for (index, page) in document.pages().iter().enumerate() {
        let number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if skip.contains(&number) {
            continue;
        }
        let text = page
            .text()
            .map(|t| clean_text(&t.all()))
            .unwrap_or_default();
        let has_images = page
            .objects()
            .iter()
            .any(|object| object.object_type() == PdfPageObjectType::Image);
        let png = if needs_transcription(&text, has_images) {
            render_png(&page)
        } else {
            None
        };
        if tx
            .blocking_send(PageData {
                number,
                text,
                has_images,
                png,
            })
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

fn render_png(page: &PdfPage<'_>) -> Option<Vec<u8>> {
    let config = PdfRenderConfig::new()
        .set_maximum_width(MAX_RENDER_SIDE as i32)
        .set_maximum_height(MAX_RENDER_SIDE as i32);
    let bitmap = page.render_with_config(&config).ok()?;
    let image = bitmap.as_image().ok()?;
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).ok()?;
    Some(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_joins_lines_and_keeps_paragraphs() {
        let raw = "First line\r\nsecond line\r\n\r\n\r\nNew paragraph\nhere  \n";
        assert_eq!(
            clean_text(raw),
            "First line second line\n\nNew paragraph here"
        );
        assert_eq!(clean_text(""), "");
        assert_eq!(clean_text("a\0b"), "ab");
    }

    #[test]
    fn the_library_directory_defaults_to_the_executable_directory() {
        let configured = library_dir(Some(Path::new("/opt/pdfium")));
        assert_eq!(configured, PathBuf::from("/opt/pdfium"));
        let default = library_dir(None);
        assert!(default.is_dir());
    }
}
