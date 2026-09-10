#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedTemplate {
    pub text: String,
    pub cursor_offset_from_end: Option<usize>,
}

pub fn render_template(
    template: &str,
    date: Option<&str>,
    clipboard: Option<&str>,
) -> RenderedTemplate {
    let mut text = template.to_string();

    if let Some(date) = date {
        text = text.replace("{{date}}", date);
    }
    if let Some(clipboard) = clipboard {
        text = text.replace("{{clipboard}}", clipboard);
    }

    let cursor_offset_from_end = text.find("{{cursor}}").map(|index| {
        let after = &text[index + "{{cursor}}".len()..];
        after.chars().count()
    });
    text = text.replace("{{cursor}}", "");

    RenderedTemplate {
        text,
        cursor_offset_from_end,
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
}
