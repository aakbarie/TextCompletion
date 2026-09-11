//! Snippet template variables.
//!
//! Supported placeholders:
//! - `{{date}}`: today's date in the local time zone, formatted MM/DD/YYYY
//! - `{{clipboard}}`: the current text clipboard contents
//! - `{{cursor}}`: where the caret should land after expansion
//!
//! The template is tokenized once. Substituted values are inserted literally,
//! so clipboard text that happens to contain `{{cursor}}` is typed as-is and
//! never treated as an instruction. Unknown placeholders are left untouched.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Segment<'a> {
    Literal(&'a str),
    Date,
    Clipboard,
    Cursor,
}

/// Splits a template into literal text and placeholders in one pass.
fn tokenize(template: &str) -> Vec<Segment<'_>> {
    const PLACEHOLDERS: [(&str, Segment<'static>); 3] = [
        (DATE_PLACEHOLDER, Segment::Date),
        (CLIPBOARD_PLACEHOLDER, Segment::Clipboard),
        (CURSOR_PLACEHOLDER, Segment::Cursor),
    ];

    let mut segments = Vec::new();
    let mut literal_start = 0;
    let mut index = 0;

    while index < template.len() {
        let rest = &template[index..];
        if let Some((token, segment)) = PLACEHOLDERS
            .iter()
            .find(|(token, _)| rest.starts_with(token))
        {
            if literal_start < index {
                segments.push(Segment::Literal(&template[literal_start..index]));
            }
            segments.push(*segment);
            index += token.len();
            literal_start = index;
        } else {
            // Advance one character; placeholders are ASCII so this cannot split one.
            index += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    if literal_start < template.len() {
        segments.push(Segment::Literal(&template[literal_start..]));
    }
    segments
}

/// Renders `template` with the given values. A placeholder whose value is
/// `None` is kept literally so nothing is silently dropped. Only the first
/// `{{cursor}}` positions the caret; any others are removed.
pub fn render_template(
    template: &str,
    date: Option<&str>,
    clipboard: Option<&str>,
) -> RenderedTemplate {
    struct Output {
        text: String,
        chars: usize,
    }

    impl Output {
        fn push(&mut self, piece: &str) {
            self.text.push_str(piece);
            self.chars += piece.chars().count();
        }
    }

    let mut out = Output {
        text: String::with_capacity(template.len()),
        chars: 0,
    };
    let mut chars_before_cursor: Option<usize> = None;

    for segment in tokenize(template) {
        match segment {
            Segment::Literal(piece) => out.push(piece),
            Segment::Date => out.push(date.unwrap_or(DATE_PLACEHOLDER)),
            Segment::Clipboard => out.push(clipboard.unwrap_or(CLIPBOARD_PLACEHOLDER)),
            Segment::Cursor => {
                if chars_before_cursor.is_none() {
                    chars_before_cursor = Some(out.chars);
                }
            }
        }
    }

    RenderedTemplate {
        cursor_offset_from_end: chars_before_cursor.map(|before| out.chars - before),
        text: out.text,
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
    fn substitutes_date_and_clipboard() {
        let rendered = render_template(
            "On {{date}}: {{clipboard}}!",
            Some("09/11/2026"),
            Some("Jane"),
        );
        assert_eq!(rendered.text, "On 09/11/2026: Jane!");
        assert_eq!(rendered.cursor_offset_from_end, None);
    }

    #[test]
    fn missing_values_keep_placeholders_literal() {
        let rendered = render_template("{{date}} {{clipboard}} {{service}}", None, None);
        assert_eq!(rendered.text, "{{date}} {{clipboard}} {{service}}");
    }

    #[test]
    fn clipboard_containing_markers_is_typed_literally() {
        let rendered = render_template(
            "Note: {{clipboard}} end{{cursor}}",
            Some("D"),
            Some("x {{cursor}} {{date}} y"),
        );
        assert_eq!(rendered.text, "Note: x {{cursor}} {{date}} y end");
        assert_eq!(rendered.cursor_offset_from_end, Some(0));
    }

    #[test]
    fn only_the_first_cursor_marker_positions_the_caret() {
        let rendered = render_template("ab{{cursor}}cd{{cursor}}ef", None, None);
        assert_eq!(rendered.text, "abcdef");
        assert_eq!(rendered.cursor_offset_from_end, Some(4));
    }

    #[test]
    fn adjacent_and_leading_markers() {
        let rendered = render_template("{{cursor}}{{date}}{{clipboard}}", Some("1"), Some("2"));
        assert_eq!(rendered.text, "12");
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
