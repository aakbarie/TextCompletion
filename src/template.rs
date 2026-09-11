//! Snippet template variables.
//!
//! Supported placeholders:
//! - `{{date}}`: today's date in the local time zone, formatted MM/DD/YYYY
//! - `{{clipboard}}`: the current text clipboard contents
//! - `{{cursor}}`: where the caret should land after expansion
//!
//! Unknown placeholders are left untouched so nothing is silently dropped.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTemplate {
    pub text: String,
    /// Number of characters between the caret marker and the end of `text`.
    pub cursor_offset_from_end: Option<usize>,
}

pub const DATE_PLACEHOLDER: &str = "{{date}}";
pub const CLIPBOARD_PLACEHOLDER: &str = "{{clipboard}}";
pub const CURSOR_PLACEHOLDER: &str = "{{cursor}}";

/// Date format used for `{{date}}`.
pub const DATE_FORMAT: &str = "%m/%d/%Y";

pub fn render_template(
    template: &str,
    date: Option<&str>,
    clipboard: Option<&str>,
) -> RenderedTemplate {
    let mut text = template.to_string();

    if let Some(date) = date {
        text = text.replace(DATE_PLACEHOLDER, date);
    }
    if let Some(clipboard) = clipboard {
        text = text.replace(CLIPBOARD_PLACEHOLDER, clipboard);
    }

    let cursor_offset_from_end = text.find(CURSOR_PLACEHOLDER).map(|index| {
        let after = &text[index + CURSOR_PLACEHOLDER.len()..];
        after.chars().count()
    });
    text = text.replace(CURSOR_PLACEHOLDER, "");

    RenderedTemplate {
        text,
        cursor_offset_from_end,
    }
}

/// Renders a template against the live environment: today's date and the
/// current clipboard. The clipboard is only read when the template asks for it.
pub fn render_now(template: &str) -> RenderedTemplate {
    let date = if template.contains(DATE_PLACEHOLDER) {
        Some(chrono::Local::now().format(DATE_FORMAT).to_string())
    } else {
        None
    };

    let clipboard = if template.contains(CLIPBOARD_PLACEHOLDER) {
        match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
            Ok(text) => Some(text),
            Err(error) => {
                log::warn!("clipboard unavailable for {{{{clipboard}}}}: {error}");
                Some(String::new())
            }
        }
    } else {
        None
    };

    render_template(template, date.as_deref(), clipboard.as_deref())
}

/// How many Left-arrow presses move the caret from the end of the typed
/// output back to the `{{cursor}}` position. The trailing delimiter the user
/// typed is re-inserted after the text, so it is counted too.
pub fn cursor_left_presses(rendered: &RenderedTemplate, trailing_inserted: bool) -> usize {
    match rendered.cursor_offset_from_end {
        Some(offset) => offset + usize::from(trailing_inserted),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_offset_counts_unicode_characters() {
        let rendered = render_template("A {{cursor}}βγ", None, None);
        assert_eq!(rendered.text, "A βγ");
        assert_eq!(rendered.cursor_offset_from_end, Some(2));
    }

    #[test]
    fn left_presses_include_trailing_delimiter() {
        let rendered = render_template("Decision: {{cursor}} because", None, None);
        assert_eq!(cursor_left_presses(&rendered, true), " because".len() + 1);
        assert_eq!(cursor_left_presses(&rendered, false), " because".len());
    }

    #[test]
    fn no_cursor_marker_means_no_movement() {
        let rendered = render_template("plain", None, None);
        assert_eq!(cursor_left_presses(&rendered, true), 0);
    }

    #[test]
    fn render_now_resolves_date_without_touching_clipboard() {
        let rendered = render_now("Today is {{date}}.");
        assert!(!rendered.text.contains(DATE_PLACEHOLDER));
        assert!(rendered.text.starts_with("Today is "));
        assert_eq!(rendered.text.len(), "Today is MM/DD/YYYY.".len());
    }
}
