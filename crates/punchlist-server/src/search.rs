//! `/api/search`: the workspace's issues whose id, title, body or comments contain a text
//! (ADR 0011).

use std::collections::HashMap;
use std::ops::Range;

use axum::Json;
use axum::extract::{Query, State};
use punchlist_api::{ErrorBody, SearchHit, SearchResults, Snippet, SnippetField};
use punchlist_core::display_name;
use serde::Deserialize;
use utoipa::IntoParams;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::actors::person_only;
use crate::auth::Actor;
use crate::{ApiError, AppState};

pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(search))
}

const MIN_CHARS: usize = 3;
const MAX_CHARS: usize = 200;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 100;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
struct SearchQuery {
    /// One literal phrase, 3 to 200 characters once trimmed and its whitespace runs made one
    /// space, matched as a substring in any case.
    q: String,
    /// The most hits to answer: 50 by default, at most 100.
    limit: Option<u32>,
}

/// Search the workspace's issues: their id, title, body and comments. A person's token.
#[utoipa::path(
    get,
    path = "/api/search",
    params(SearchQuery),
    responses(
        (status = 200, body = SearchResults),
        (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody, description = "Not a person's token."),
        (status = 422, body = ErrorBody, description = "A query under 3 or over 200 characters."),
    )
)]
async fn search(
    State(state): State<AppState>,
    actor: Actor,
    Query(query): Query<SearchQuery>,
) -> Result<Json<SearchResults>, ApiError> {
    person_only(&actor)?;
    let text = query.q.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = text.chars().count();
    if chars < MIN_CHARS {
        return Err(ApiError::Invalid(format!(
            "search needs at least {MIN_CHARS} characters"
        )));
    }
    if chars > MAX_CHARS {
        return Err(ApiError::Invalid(format!(
            "search takes at most {MAX_CHARS} characters"
        )));
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let pattern = format!("%{}%", escape_like(&text));
    // ponytail: substring matching on trigram indexes, not full-text search. No stemming
    // (`run` misses `running` and finds `trunk`), no ranking, and every issue the text
    // matches is read and sorted before the limit, so a common word costs more than a rare
    // one. Upgrade path: a generated tsvector column per table with a GIN index, when
    // ranking matters (ADR 0011).
    let mut issues = sqlx::query!(
        r#"WITH matched AS (
               SELECT id FROM issue
               WHERE workspace_id = $1
                 AND (id ILIKE $2 ESCAPE '\' OR title ILIKE $2 ESCAPE '\'
                      OR body ILIKE $2 ESCAPE '\')
               UNION
               SELECT c.issue_id FROM comment c JOIN issue i ON i.id = c.issue_id
               WHERE i.workspace_id = $1 AND c.body ILIKE $2 ESCAPE '\'
           )
           SELECT i.id, i.title, i.body, i.status,
                  (i.id ILIKE $2 ESCAPE '\' OR i.title ILIKE $2 ESCAPE '\') AS "in_title!",
                  i.body ILIKE $2 ESCAPE '\' AS "in_body!"
           FROM matched JOIN issue i USING (id)
           ORDER BY "in_title!" DESC, i.created_at DESC, i.number DESC
           LIMIT $3"#,
        actor.workspace_id,
        pattern,
        limit as i64 + 1,
    )
    .fetch_all(&state.pool)
    .await?;
    let more = issues.len() > limit;
    issues.truncate(limit);

    let without_body: Vec<String> = (issues.iter())
        .filter(|issue| !issue.in_body)
        .map(|issue| issue.id.clone())
        .collect();
    let comments: HashMap<String, (String, i64)> = sqlx::query!(
        r#"SELECT DISTINCT ON (c.issue_id) c.issue_id, c.body, e.seq
           FROM comment c JOIN event e ON e.issue_id = c.issue_id AND e.comment_id = c.id
           WHERE c.issue_id = ANY($1) AND c.body ILIKE $2 ESCAPE '\'
           ORDER BY c.issue_id, c.created_at DESC, e.seq DESC"#,
        &without_body,
        pattern,
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| (row.issue_id, (row.body, row.seq)))
    .collect();

    let hits = issues
        .into_iter()
        .map(|issue| {
            let snippet = if issue.in_body {
                Some(snippet(SnippetField::Body, &issue.body, &text, None))
            } else {
                (comments.get(&issue.id))
                    .map(|(body, seq)| snippet(SnippetField::Comment, body, &text, Some(*seq)))
            };
            SearchHit {
                status_name: display_name(&issue.status),
                issue_id: issue.id,
                title: issue.title,
                in_title: issue.in_title,
                snippet,
            }
        })
        .collect();
    Ok(Json(SearchResults { hits, more }))
}

/// `text` as an `ILIKE` pattern's body: `\`, `%` and `_` match themselves.
fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

const BEFORE: usize = 40;
const AFTER: usize = 80;
const FALLBACK: usize = 120;

/// The line around the first case-insensitive occurrence of `query` in `text`: up to 40
/// characters before it and 80 after, `…` included where cut. With no occurrence, which
/// happens when Postgres folds case differently, the text's first 120 characters.
fn snippet(field: SnippetField, text: &str, query: &str, comment_seq: Option<i64>) -> Snippet {
    let (before, hit, after) = match find_ignoring_case(text, query) {
        Some(range) => {
            let before = one_line(&text[..range.start]);
            let after = one_line(&text[range.end..]);
            let count = before.chars().count();
            let before = if count > BEFORE {
                let kept: String = before.chars().skip(count - (BEFORE - 1)).collect();
                format!("…{kept}")
            } else {
                before
            };
            let after = if after.chars().count() > AFTER {
                let kept: String = after.chars().take(AFTER - 1).collect();
                format!("{kept}…")
            } else {
                after
            };
            (before, one_line(&text[range]), after)
        }
        None => {
            let line = one_line(text);
            let cut = line.chars().count() > FALLBACK;
            let head = line.chars().take(FALLBACK).collect();
            (head, String::new(), if cut { "…" } else { "" }.to_string())
        }
    };
    Snippet {
        field,
        before,
        hit,
        after,
        comment_seq,
    }
}

/// The byte range of the first occurrence of `needle` in `text`, comparing characters by
/// their lowercase forms.
fn find_ignoring_case(text: &str, needle: &str) -> Option<Range<usize>> {
    text.char_indices().find_map(|(start, _)| {
        let mut rest = text[start..].chars();
        let mut end = start;
        for wanted in needle.chars() {
            let found = rest.next()?;
            if !found.to_lowercase().eq(wanted.to_lowercase()) {
                return None;
            }
            end += found.len_utf8();
        }
        Some(start..end)
    })
}

/// `text` with each run of whitespace and control characters, ANSI escapes' ESC included,
/// made one space, so a snippet is one line a terminal prints as text.
fn one_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            if !gap {
                line.push(' ');
            }
            gap = true;
        } else {
            line.push(c);
            gap = false;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(text: &str, query: &str) -> (String, String, String) {
        let s = snippet(SnippetField::Body, text, query, None);
        (s.before, s.hit, s.after)
    }

    #[test]
    fn a_long_body_is_cut_around_the_match() {
        let text = format!("{}Ci_Green{}", "x".repeat(3000), "y".repeat(1992));
        assert_eq!(text.chars().count(), 5000);
        let (before, hit, after) = parts(&text, "ci_green");
        assert_eq!(before, format!("…{}", "x".repeat(39)));
        assert_eq!(hit, "Ci_Green");
        assert_eq!(after, format!("{}…", "y".repeat(79)));
        assert_eq!(before.chars().count(), 40);
        assert_eq!(after.chars().count(), 80);
    }

    #[test]
    fn cuts_fall_on_character_boundaries() {
        let text = format!("{}NEEDLE{}", "café ü ".repeat(10), "ü café ".repeat(20));
        let (before, hit, after) = parts(&text, "needle");
        assert_eq!(before, format!("…é ü {}", "café ü ".repeat(5)));
        assert_eq!(hit, "NEEDLE");
        assert_eq!(after, format!("{}ü …", "ü café ".repeat(11)));
        assert_eq!(after.chars().count(), 80);
    }

    #[test]
    fn control_characters_and_whitespace_runs_become_one_space() {
        assert_eq!(one_line("a\u{1b}[31m b"), "a [31m b");
        assert_eq!(
            parts("x\n\n\t\u{1b}[31mred\u{1b}[0m  end", "red"),
            ("x [31m".into(), "red".into(), " [0m end".into())
        );
    }

    #[test]
    fn no_occurrence_answers_the_first_120_characters() {
        let text = "é".repeat(150);
        let (before, hit, after) = parts(&text, "zzz");
        assert_eq!(before, "é".repeat(120));
        assert_eq!(hit, "");
        assert_eq!(after, "…");
        assert_eq!(
            parts("short", "zzz"),
            ("short".into(), "".into(), "".into())
        );
    }

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like(r"100%_a\b"), r"100\%\_a\\b");
    }
}
