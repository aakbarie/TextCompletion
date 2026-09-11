use textcompletion::expansion::{ExpansionMatcher, SharedSnippetIndex};
use textcompletion::template::render_template;

#[derive(Clone, Copy)]
enum Role {
    Md,
    Rn,
}

struct SyntheticCase {
    id: usize,
    role: Role,
    trigger: String,
    replacement: String,
    delimiter: char,
    date: Option<&'static str>,
    clipboard: Option<&'static str>,
}

fn generate_cases() -> Vec<SyntheticCase> {
    let mut cases = Vec::with_capacity(200);

    for i in 0..100 {
        let trigger = format!(";md{:03}", i + 1);
        let replacement = match i % 5 {
            0 => "I reviewed the available clinical documentation and agree with the determination.",
            1 => "Peer-to-peer review completed on {{date}}. Clinical rationale documented in the medical record.",
            2 => "Additional clinical documentation is required before medical necessity can be determined.",
            3 => "The request meets applicable medical necessity criteria. {{cursor}}",
            _ => "Reviewed member information: {{clipboard}}",
        };
        cases.push(SyntheticCase {
            id: i + 1,
            role: Role::Md,
            trigger,
            replacement: replacement.to_string(),
            delimiter: if i % 3 == 0 {
                ' '
            } else if i % 3 == 1 {
                '\t'
            } else {
                '\n'
            },
            date: Some("09/10/2026"),
            clipboard: Some("Synthetic Member"),
        });
    }

    for i in 0..100 {
        let trigger = format!(";rn{:03}", i + 1);
        let replacement = match i % 5 {
            0 => "RN review completed. Member outreach attempted and documented.",
            1 => "Care coordination follow-up scheduled for {{date}}.",
            2 => "Member education provided regarding plan benefits and next steps.",
            3 => "Escalated to medical director for review. {{cursor}}",
            _ => "RN reviewed: {{clipboard}}",
        };
        cases.push(SyntheticCase {
            id: 101 + i,
            role: Role::Rn,
            trigger,
            replacement: replacement.to_string(),
            delimiter: if i % 3 == 0 {
                ' '
            } else if i % 3 == 1 {
                '\t'
            } else {
                '\n'
            },
            date: Some("09/10/2026"),
            clipboard: Some("Synthetic Member"),
        });
    }

    cases
}

#[test]
fn synthetic_uat_200_md_and_rn_cases() {
    let cases = generate_cases();
    assert_eq!(cases.len(), 200);

    let index = SharedSnippetIndex::default();
    {
        let mut guard = index.write();
        for case in &cases {
            guard.insert(case.trigger.clone(), case.replacement.clone());
        }
    }

    for case in cases {
        let mut matcher = ExpansionMatcher::new(index.clone());
        for ch in case.trigger.chars() {
            assert!(
                matcher.feed_char(ch).is_none(),
                "case {} prematurely expanded",
                case.id
            );
        }

        let expansion = matcher
            .feed_char(case.delimiter)
            .unwrap_or_else(|| panic!("case {} failed to match", case.id));

        assert_eq!(
            expansion.backspaces,
            case.trigger.chars().count(),
            "case {} backspace count",
            case.id
        );
        assert_eq!(
            expansion.trailing, case.delimiter,
            "case {} delimiter",
            case.id
        );

        let rendered = render_template(&expansion.replacement, case.date, case.clipboard);
        assert!(
            !rendered.text.contains("{{date}}"),
            "case {} date variable unresolved",
            case.id
        );
        assert!(
            !rendered.text.contains("{{clipboard}}"),
            "case {} clipboard variable unresolved",
            case.id
        );
        assert!(
            !rendered.text.contains("{{cursor}}"),
            "case {} cursor marker unresolved",
            case.id
        );
        assert!(!rendered.text.is_empty(), "case {} rendered empty", case.id);

        match case.role {
            Role::Md => assert!(case.trigger.starts_with(";md")),
            Role::Rn => assert!(case.trigger.starts_with(";rn")),
        }
    }
}
