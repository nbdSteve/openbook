//! Input validation. Every function is pure.

use openbook_core::ids;

use crate::error::ApiError;

pub const MAX_COURSE_NAME_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 500;
pub const MAX_FILENAME_CHARS: usize = 200;
pub const MAX_MESSAGE_CHARS: usize = 4000;
pub const MAX_USER_ID_CHARS: usize = 128;

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::InvalidRequest(message.into())
}

/// A course name: 1..80 characters after trimming.
pub fn course_name(input: &str) -> Result<String, ApiError> {
    let name = input.trim();
    match name.chars().count() {
        0 => Err(invalid("name is required")),
        n if n > MAX_COURSE_NAME_CHARS => Err(invalid(format!(
            "name must be at most {MAX_COURSE_NAME_CHARS} characters"
        ))),
        _ => Ok(name.to_owned()),
    }
}

/// A course description: 0..500 characters after trimming.
pub fn description(input: &str) -> Result<String, ApiError> {
    let text = input.trim();
    if text.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(invalid(format!(
            "description must be at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    Ok(text.to_owned())
}

/// A chat message: 1..4000 characters after trimming.
pub fn chat_message(input: &str) -> Result<String, ApiError> {
    let text = input.trim();
    match text.chars().count() {
        0 => Err(invalid("message is required")),
        n if n > MAX_MESSAGE_CHARS => Err(invalid(format!(
            "message must be at most {MAX_MESSAGE_CHARS} characters"
        ))),
        _ => Ok(text.to_owned()),
    }
}

/// A demo question: 1..300 characters after trimming.
pub fn demo_message(input: &str) -> Result<String, ApiError> {
    let max = openbook_core::demo::MAX_MESSAGE_CHARS;
    let text = input.trim();
    match text.chars().count() {
        0 => Err(invalid("message is required")),
        n if n > max => Err(invalid(format!("message must be at most {max} characters"))),
        _ => Ok(text.to_owned()),
    }
}

/// A PDF file name: 1..200 characters, ends in `.pdf` (any case), no control characters.
pub fn pdf_filename(input: &str) -> Result<String, ApiError> {
    let name = input.trim();
    let chars = name.chars().count();
    if chars == 0 {
        return Err(invalid("filename is required"));
    }
    if chars > MAX_FILENAME_CHARS {
        return Err(invalid(format!(
            "filename must be at most {MAX_FILENAME_CHARS} characters"
        )));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("filename has invalid characters"));
    }
    if !name.to_lowercase().ends_with(".pdf") {
        return Err(invalid("filename must end in .pdf"));
    }
    Ok(name.to_owned())
}

/// The only accepted upload type.
pub fn pdf_content_type(input: &str) -> Result<(), ApiError> {
    if input.trim().eq_ignore_ascii_case("application/pdf") {
        Ok(())
    } else {
        Err(invalid("contentType must be application/pdf"))
    }
}

/// A file size in bytes: more than 0, at most `max` (`413` otherwise).
pub fn upload_size(size: u64, max: u64) -> Result<u64, ApiError> {
    if size == 0 {
        return Err(invalid("sizeBytes must be more than 0"));
    }
    if size > max {
        return Err(ApiError::TooLarge(format!(
            "The file is larger than {} MiB",
            max / (1024 * 1024)
        )));
    }
    Ok(size)
}

/// An ID in a path. Anything that is not a ULID names no resource, so it is a `404`.
pub fn ulid_path(input: &str, what: &str) -> Result<String, ApiError> {
    if ids::is_ulid(input) {
        Ok(input.to_owned())
    } else {
        Err(ApiError::not_found(what))
    }
}

/// A user ID in a path (a Cognito `sub`). Only a sanity check on length and characters.
pub fn user_id_path(input: &str) -> Result<String, ApiError> {
    let ok = !input.is_empty()
        && input.chars().count() <= MAX_USER_ID_CHARS
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(input.to_owned())
    } else {
        Err(ApiError::not_found("Member"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_invalid<T: std::fmt::Debug>(r: Result<T, ApiError>) -> bool {
        matches!(r, Err(ApiError::InvalidRequest(_)))
    }

    #[test]
    fn course_names() {
        assert_eq!(course_name("  Algebra I ").unwrap(), "Algebra I");
        assert!(is_invalid(course_name("   ")));
        assert!(is_invalid(course_name("")));
        assert!(course_name(&"é".repeat(80)).is_ok());
        assert!(is_invalid(course_name(&"é".repeat(81))));
    }

    #[test]
    fn descriptions() {
        assert_eq!(description("  ").unwrap(), "");
        assert!(description(&"x".repeat(500)).is_ok());
        assert!(is_invalid(description(&"x".repeat(501))));
    }

    #[test]
    fn chat_messages() {
        assert_eq!(chat_message(" hi ").unwrap(), "hi");
        assert!(is_invalid(chat_message("  \n ")));
        assert!(chat_message(&"x".repeat(4000)).is_ok());
        assert!(is_invalid(chat_message(&"x".repeat(4001))));
        // Counted in characters, not bytes.
        assert!(chat_message(&"é".repeat(4000)).is_ok());
    }

    #[test]
    fn demo_messages() {
        assert_eq!(
            demo_message("  What is a function? ").unwrap(),
            "What is a function?"
        );
        assert!(is_invalid(demo_message("")));
        assert!(is_invalid(demo_message(" \n\t ")));
        assert!(demo_message(&"x".repeat(300)).is_ok());
        assert!(is_invalid(demo_message(&"x".repeat(301))));
        // Padding does not count, and characters are not bytes.
        assert!(demo_message(&format!("  {}  ", "é".repeat(300))).is_ok());
        assert!(is_invalid(demo_message(&"é".repeat(301))));
    }

    #[test]
    fn filenames() {
        assert_eq!(pdf_filename(" Notes.PDF ").unwrap(), "Notes.PDF");
        assert!(is_invalid(pdf_filename("")));
        assert!(is_invalid(pdf_filename("notes.docx")));
        assert!(is_invalid(pdf_filename("notes.pdf.exe")));
        assert!(is_invalid(pdf_filename("a\nb.pdf")));
        assert!(pdf_filename(&format!("{}.pdf", "a".repeat(196))).is_ok());
        assert!(is_invalid(pdf_filename(&format!(
            "{}.pdf",
            "a".repeat(197)
        ))));
    }

    #[test]
    fn content_types() {
        assert!(pdf_content_type("application/pdf").is_ok());
        assert!(is_invalid(pdf_content_type("text/plain")));
        assert!(is_invalid(pdf_content_type("")));
    }

    #[test]
    fn upload_sizes() {
        assert_eq!(upload_size(10, 100).unwrap(), 10);
        assert_eq!(upload_size(100, 100).unwrap(), 100);
        assert!(is_invalid(upload_size(0, 100)));
        assert!(matches!(upload_size(101, 100), Err(ApiError::TooLarge(_))));
    }

    #[test]
    fn path_ids() {
        let id = ids::new_id();
        assert_eq!(ulid_path(&id, "Course").unwrap(), id);
        assert_eq!(
            ulid_path("nope", "Course"),
            Err(ApiError::NotFound("Course not found".into()))
        );
        assert!(user_id_path("1b2c3d4e-0000-4000-8000-123456789abc").is_ok());
        assert!(user_id_path("").is_err());
        assert!(user_id_path("a#b").is_err());
        assert!(user_id_path(&"a".repeat(129)).is_err());
    }
}
