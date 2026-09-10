use textcompletion::template::render_template;

#[test]
fn renders_date_variable() {
    let rendered = render_template("Reviewed on {{date}}.", Some("09/10/2026"), None);
    assert_eq!(rendered.text, "Reviewed on 09/10/2026.");
    assert_eq!(rendered.cursor_offset_from_end, None);
}

#[test]
fn renders_clipboard_variable() {
    let rendered = render_template("Member: {{clipboard}}", None, Some("Jane Doe"));
    assert_eq!(rendered.text, "Member: Jane Doe");
}

#[test]
fn removes_cursor_marker_and_reports_position() {
    let rendered = render_template("Decision: {{cursor}} because criteria are met.", None, None);
    assert_eq!(rendered.text, "Decision:  because criteria are met.");
    assert_eq!(rendered.cursor_offset_from_end, Some(25));
}

#[test]
fn unknown_variables_are_left_intact() {
    let rendered = render_template("{{service}} requested", None, None);
    assert_eq!(rendered.text, "{{service}} requested");
}
