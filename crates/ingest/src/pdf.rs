//! PDF access through pdfium: page count, text layer, and page rendering.
//!
//! `libpdfium` comes from `OPENBOOK_PDFIUM_PATH`, or from the directory of the executable.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::DynamicImage;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use openbook_search::quality::needs_transcription;
use pdfium_render::prelude::*;
use tokio::sync::mpsc::Sender;

use crate::transcribe::{MAX_IMAGE_BYTES, MAX_RENDER_SIDE};

/// JPEG qualities to try, best first.
const JPEG_QUALITIES: [u8; 4] = [85, 70, 55, 40];

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
    /// The text layer looks poor, so the page needs transcription.
    pub needs_transcription: bool,
    /// The rendered page (grayscale JPEG under `MAX_IMAGE_BYTES`). Only for pages that need
    /// transcription, and `None` when rendering failed.
    pub image: Option<Vec<u8>>,
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
pub fn pdfium(dir: &Path) -> Result<&'static Pdfium, PdfError> {
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
        let needs = needs_transcription(&text, has_images);
        let image = if needs { render_image(&page) } else { None };
        if tx
            .blocking_send(PageData {
                number,
                text,
                has_images,
                needs_transcription: needs,
                image,
            })
            .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

fn render_image(page: &PdfPage<'_>) -> Option<Vec<u8>> {
    let side = MAX_RENDER_SIDE as i32;
    let config = PdfRenderConfig::new()
        .set_maximum_width(side)
        .set_maximum_height(side);
    let bitmap = page.render_with_config(&config).ok()?;
    let image = bitmap.as_image().ok()?;
    encode_for_model(&image)
}

/// Turns a rendered page into a grayscale JPEG with the longest side at most `MAX_RENDER_SIDE`
/// and under `MAX_IMAGE_BYTES` (Bedrock accepts at most 3.75 MB per image). It lowers the JPEG
/// quality first, then the size, until the image fits. Returns `None` when nothing fits.
pub fn encode_for_model(image: &DynamicImage) -> Option<Vec<u8>> {
    let gray = DynamicImage::ImageLuma8(image.to_luma8());
    let longest = gray.width().max(gray.height());
    let base = if longest > MAX_RENDER_SIDE {
        gray.resize(MAX_RENDER_SIDE, MAX_RENDER_SIDE, FilterType::Lanczos3)
    } else {
        gray
    };
    for scale in [1.0_f32, 0.85, 0.7, 0.55, 0.4] {
        let candidate = if scale < 1.0 {
            let w = ((base.width() as f32) * scale).max(1.0) as u32;
            let h = ((base.height() as f32) * scale).max(1.0) as u32;
            base.resize_exact(w, h, FilterType::Triangle)
        } else {
            base.clone()
        };
        for quality in JPEG_QUALITIES {
            let mut bytes = Vec::new();
            let encoder = JpegEncoder::new_with_quality(&mut bytes, quality);
            if candidate.write_with_encoder(encoder).is_ok() && bytes.len() <= MAX_IMAGE_BYTES {
                return Some(bytes);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pseudo-random noise: it compresses badly, like a scanned page with grain.
    fn noisy_image(width: u32, height: u32) -> DynamicImage {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut image = image::RgbImage::new(width, height);
        for pixel in image.pixels_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *pixel = image::Rgb([(state >> 8) as u8, (state >> 24) as u8, (state >> 40) as u8]);
        }
        DynamicImage::ImageRgb8(image)
    }

    #[test]
    fn a_noisy_full_page_image_ends_up_under_the_limit_with_the_right_size() {
        // Far bigger than a model needs, and incompressible.
        let image = noisy_image(3200, 4200);
        let bytes = encode_for_model(&image).expect("an image that fits");
        assert!(bytes.len() <= MAX_IMAGE_BYTES, "{} bytes", bytes.len());
        const { assert!(MAX_IMAGE_BYTES < 3_750_000) }; // Bedrock accepts at most 3.75 MB
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.width().max(decoded.height()) <= MAX_RENDER_SIDE);
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG");
        // Grayscale: one component.
        assert_eq!(decoded.color().channel_count(), 1);
    }

    #[test]
    fn noise_at_the_render_size_is_squeezed_by_quality_and_scale() {
        let image = noisy_image(MAX_RENDER_SIDE, MAX_RENDER_SIDE);
        let bytes = encode_for_model(&image).unwrap();
        assert!(bytes.len() <= MAX_IMAGE_BYTES);
    }

    #[test]
    fn a_plain_page_keeps_its_size_and_is_small() {
        let page = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            1000,
            1400,
            image::Rgb([255, 255, 255]),
        ));
        let bytes = encode_for_model(&page).unwrap();
        assert!(bytes.len() < 100_000);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (1000, 1400),
            "small images are not scaled up or down"
        );
    }

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
