//! Parsing Linear's GraphQL answers for `pl import linear`, over responses in Linear's shape
//! with real KAT content (`fixtures/linear/`).

use punchlist_api::{LinearComment, LinearState, LinearUser};
use punchlist_cli::{parse_comments, parse_issues, parse_labels, parse_project};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/linear/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn gannon() -> Option<LinearUser> {
    Some(LinearUser {
        id: "240f1aae-bb2d-4208-8e7a-229638b979c7".into(),
        name: "Gannon Hall".into(),
    })
}

#[test]
fn the_project_and_its_milestones_parse() {
    let project = parse_project(&fixture("project"), "KAT", "Punchlist").unwrap();
    assert_eq!(project.id, "a203d5bc-c8c0-4ce1-8366-cc64538b6bfa");
    assert_eq!(project.name, "Punchlist");
    assert_eq!(
        project.url,
        "https://linear.app/kata-sh/project/punchlist-7cf20c10006d"
    );
    let milestones: Vec<(&str, &str)> = (project.milestones.iter())
        .map(|m| (m.name.as_str(), &m.description.as_deref().unwrap()[..40]))
        .collect();
    assert_eq!(
        milestones,
        [
            (
                "Gate 0: Spec, stack and prototype",
                "PASS when the PRD and ADRs 0001 to 0004 "
            ),
            (
                "Gate 1: Groundwork ships from Punchlist",
                "PASS when Groundwork's and Punchlist's o"
            ),
        ]
    );
}

#[test]
fn a_missing_project_or_a_refusal_is_an_error_naming_it() {
    assert_eq!(
        parse_project(&fixture("no-project"), "KAT", "Punchlist")
            .unwrap_err()
            .to_string(),
        "Linear team KAT has no project named Punchlist"
    );
    assert_eq!(
        parse_project(r#"{"data":{"teams":{"nodes":[]}}}"#, "NOPE", "Punchlist")
            .unwrap_err()
            .to_string(),
        "Linear has no team with the key NOPE"
    );
    assert_eq!(
        parse_project(&fixture("account-disabled"), "KAT", "Punchlist")
            .unwrap_err()
            .to_string(),
        "Linear refused the query: Account disabled"
    );
}

#[test]
fn issues_pages_and_a_comment_page_parse() {
    let first = parse_issues(&fixture("issues-1")).unwrap();
    assert_eq!(
        first.next.as_deref(),
        Some("fec846ce-e3c7-4e33-90dd-b193b6ffd996")
    );
    assert_eq!(
        first.more_comments,
        [(
            "b9e82236-1717-4739-ae8a-253cd409d1d3".to_string(),
            "a31e9dc6-3696-4b18-a1b3-708bb1a3d31e".to_string()
        )]
    );
    assert_eq!(
        first.more_labels,
        [(
            "b9e82236-1717-4739-ae8a-253cd409d1d3".to_string(),
            "8f0c5e0a-label-cursor".to_string()
        )]
    );
    let shown: Vec<String> = (first.issues.iter())
        .map(|i| {
            format!(
                "{} {:?} {:?} {:?} parent {:?} milestone {:?} comments {} at {} / {}",
                i.identifier,
                i.title,
                i.state,
                i.labels,
                i.parent_id,
                i.milestone_id,
                i.comments.len(),
                i.created_at.to_rfc3339(),
                i.updated_at.to_rfc3339()
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            "KAT-3743 \"Slice 10: the web app shows the issue page from the approved prototype\" LinearState { name: \"Done\", kind: \"completed\" } [\"feature\"] parent None milestone Some(\"c6e977ba-a96a-4d2e-9f0d-c7e47c60b790\") comments 1 at 2026-10-08T18:35:04.284+00:00 / 2026-10-18T21:36:45.464+00:00",
            "KAT-3744 \"Slice 11: import a Linear project into a workspace\" LinearState { name: \"In Progress\", kind: \"started\" } [\"feature\"] parent None milestone Some(\"c6e977ba-a96a-4d2e-9f0d-c7e47c60b790\") comments 0 at 2026-10-08T18:35:21.630+00:00 / 2026-10-19T17:30:44.709+00:00",
        ]
    );
    assert_eq!(
        first.issues[0].url,
        "https://linear.app/kata-sh/issue/KAT-3743/slice-10-the-web-app-shows-the-issue-page-from-the-approved-prototype"
    );
    assert_eq!(first.issues[0].creator, gannon());

    let second = parse_issues(&fixture("issues-2")).unwrap();
    assert_eq!(second.next, None);
    assert!(second.more_comments.is_empty());
    assert_eq!(second.issues[0].identifier, "KAT-3782");
    assert_eq!(second.issues[0].description, None);
    assert_eq!(second.issues[0].milestone_id, None);
    assert_eq!(
        second.issues[0].state,
        LinearState {
            name: "Duplicate".into(),
            kind: "duplicate".into()
        }
    );

    assert_eq!(
        parse_labels(&fixture("labels")).unwrap(),
        (vec!["web".to_string()], None)
    );

    let (comments, next) = parse_comments(&fixture("comments")).unwrap();
    assert_eq!(next, None);
    assert_eq!(
        comments,
        [LinearComment {
            id: "d4a131d6-3141-4db4-bdfc-aef466c82ad4".into(),
            body: "PR #48 is ready to merge: it is marked ready, merges cleanly, CI is green on c41f9d6, and no review threads are open.\n\nCodeRabbit's re-review found nothing to act on. Moved to Human Review.\n\n(agent)".into(),
            user: gannon(),
            created_at: "2026-10-18T21:08:35.128Z".parse().unwrap(),
        }]
    );
}
