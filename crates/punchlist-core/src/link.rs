//! Which issue a pull request belongs to, read from its branch name and title.

/// The issue id a pull request names, such as `PL-7`, for a workspace whose ids start with
/// `prefix`. The branch `feature/<id>-…` (or `feature/<id>`) names it first; otherwise
/// `(<id>)` in the title does. Both are matched without regard to case, because
/// `branch_name` lowercases the id. When the branch and the title name different issues,
/// the branch wins: an agent's branch is cut from its issue, and a title is easy to edit.
pub fn linked_issue_id(prefix: &str, branch: &str, title: &str) -> Option<String> {
    branch_issue_id(prefix, branch).or_else(|| title_issue_id(prefix, title))
}

/// The issue id the branch `feature/<id>-…` names, uppercase.
pub(crate) fn branch_issue_id(prefix: &str, branch: &str) -> Option<String> {
    branch_issue_number(prefix, branch).map(|number| issue_id(prefix, number))
}

/// The issue id `(<id>)` in the title names, uppercase: the first, when it names several.
pub(crate) fn title_issue_id(prefix: &str, title: &str) -> Option<String> {
    title_issue_ids(prefix, title).next()
}

/// Every issue id `(<id>)` in the title, uppercase, in order.
pub(crate) fn title_issue_ids<'a>(
    prefix: &'a str,
    title: &'a str,
) -> impl Iterator<Item = String> + 'a {
    title
        .split('(')
        .skip(1)
        .filter_map(move |rest| {
            let inside = &rest[..rest.find(')')?];
            strip_prefix_ignore_case(inside, prefix)?
                .strip_prefix('-')
                .and_then(parse_number)
        })
        .map(move |number| issue_id(prefix, number))
}

fn issue_id(prefix: &str, number: u64) -> String {
    format!("{}-{number}", prefix.to_ascii_uppercase())
}

fn branch_issue_number(prefix: &str, branch: &str) -> Option<u64> {
    let rest = strip_prefix_ignore_case(branch, "feature/")?;
    let rest = strip_prefix_ignore_case(rest, prefix)?;
    let rest = rest.strip_prefix('-')?;
    let digits_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (digits, tail) = rest.split_at(digits_end);
    if !(tail.is_empty() || tail.starts_with('-')) {
        return None;
    }
    parse_number(digits)
}

/// Digits only, no leading zero, so `PL-07` names no issue rather than PL-7.
fn parse_number(digits: &str) -> Option<u64> {
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(branch: &str, title: &str) -> Option<String> {
        linked_issue_id("PL", branch, title)
    }

    #[test]
    fn branch_names_the_issue() {
        assert_eq!(
            link("feature/pl-7-fix-the-thing", "Fix"),
            Some("PL-7".into())
        );
        assert_eq!(link("feature/PL-7-fix", "Fix"), Some("PL-7".into()));
        assert_eq!(link("feature/pl-7", "Fix"), Some("PL-7".into()));
        assert_eq!(link("feature/pl-123-x", "Fix"), Some("PL-123".into()));
    }

    #[test]
    fn title_names_the_issue() {
        assert_eq!(
            link("main-fix", "Fix the thing (PL-7)"),
            Some("PL-7".into())
        );
        assert_eq!(link("main-fix", "(pl-7) Fix"), Some("PL-7".into()));
        assert_eq!(
            link("main-fix", "Fix (see notes) the thing (PL-12)"),
            Some("PL-12".into())
        );
    }

    #[test]
    fn branch_wins_when_both_name_an_issue() {
        assert_eq!(link("feature/pl-7-fix", "Fix (PL-7)"), Some("PL-7".into()));
        assert_eq!(link("feature/pl-7-fix", "Fix (PL-9)"), Some("PL-7".into()));
    }

    #[test]
    fn neither_names_an_issue() {
        assert_eq!(link("fix-the-thing", "Fix the thing"), None);
        assert_eq!(link("feature/fix-pl-7", "Fix PL-7"), None);
        assert_eq!(link("feature/pl-7x-fix", "Fix [PL-7]"), None);
        assert_eq!(link("feature/pl-07-fix", "Fix (PL-07)"), None);
        assert_eq!(link("feature/plx-7-fix", "Fix (PLX-7)"), None);
        assert_eq!(link("bugfix/pl-7-fix", "Fix (PL-7"), None);
        assert_eq!(link("feature/pl-", "Fix (PL-)"), None);
    }

    #[test]
    fn other_prefixes_are_not_this_workspace() {
        assert_eq!(
            linked_issue_id("KAT", "feature/kat-3736-slice-3", ""),
            Some("KAT-3736".into())
        );
        assert_eq!(
            linked_issue_id("KAT", "feature/pl-7-fix", "Fix (PL-7)"),
            None
        );
    }
}
