use textcompletion::binding::{build_expansion_plan, Delimiter};

#[test]
fn matched_binding_erases_trigger_and_reinserts_space() {
    let plan = build_expansion_plan(";p2p", "Peer-to-peer review completed.", Delimiter::Space);
    assert_eq!(plan.backspaces, 4);
    assert_eq!(plan.replacement, "Peer-to-peer review completed.");
    assert_eq!(plan.delimiter, Delimiter::Space);
}

#[test]
fn multiline_unicode_replacement_is_preserved() {
    let replacement = "Reviewed:\n• Criteria met\n• Médico notified";
    let plan = build_expansion_plan(";review", replacement, Delimiter::Enter);
    assert_eq!(plan.backspaces, 7);
    assert_eq!(plan.replacement, replacement);
    assert_eq!(plan.delimiter, Delimiter::Enter);
}

#[test]
fn backspace_count_is_unicode_character_count_not_bytes() {
    let plan = build_expansion_plan(";sí", "Approved", Delimiter::Tab);
    assert_eq!(plan.backspaces, 3);
}
