//! Request and response types for the Punchlist API (ADR 0008).
//!
//! The server and the Rust client share these types, and utoipa derives their schemas for
//! the OpenAPI document. Core types that cross the API, such as `Role`, are re-exported
//! with their schemas from `punchlist-core`'s `openapi` feature.

use chrono::{DateTime, Utc};
pub use punchlist_core::Role;
use punchlist_core::display_name;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// `POST /api/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateIssue {
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Issue {
    /// The identifier people use, such as `PL-1`.
    pub id: String,
    pub title: String,
    /// Markdown.
    pub body: String,
    /// The workflow's status key, such as `in_progress`.
    pub status: String,
    /// The status as people read it, such as `In Progress`.
    pub status_name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `GET /api/issues`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IssueList {
    pub issues: Vec<Issue>,
    /// The sequence number of the workspace's latest event, read in the same snapshot as
    /// the list (ADR 0006).
    pub last_event_seq: i64,
}

/// `POST /api/issues/{id}/transitions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MoveIssue {
    /// The status key to move to, such as `todo`.
    pub to: String,
}

/// A transition that the server accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Moved {
    pub issue: Issue,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Actor {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
}

/// One entry in an issue's timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Event {
    /// Increases per workspace (ADR 0006).
    pub seq: i64,
    pub issue_id: String,
    pub actor: Actor,
    pub created_at: DateTime<Utc>,
    pub detail: EventDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventDetail {
    Transition {
        from: String,
        /// `from` as people read it, such as `In Progress`.
        from_name: String,
        to: String,
        /// `to` as people read it.
        to_name: String,
        /// The content hash of the workflow the transition was checked against.
        workflow_version: String,
    },
}

impl EventDetail {
    /// A transition between two status keys, with their display names filled in.
    pub fn transition(from: String, to: String, workflow_version: String) -> EventDetail {
        EventDetail::Transition {
            from_name: display_name(&from),
            to_name: display_name(&to),
            from,
            to,
            workflow_version,
        }
    }
}

/// The body of every error response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    /// A stable code, such as `role_not_allowed` or `not_found`.
    pub code: String,
    /// What went wrong. For a refused transition, the rule that refused it.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_detail_is_tagged_by_kind() {
        let detail = EventDetail::transition(
            "in_progress".into(),
            "agent_review".into(),
            "sha256:ab".into(),
        );
        assert_eq!(
            serde_json::to_string(&detail).unwrap(),
            r#"{"kind":"transition","from":"in_progress","from_name":"In Progress","to":"agent_review","to_name":"Agent Review","workflow_version":"sha256:ab"}"#
        );
    }

    #[test]
    fn create_issue_body_defaults_to_empty() {
        let request: CreateIssue = serde_json::from_str(r#"{"title":"First"}"#).unwrap();
        assert_eq!(
            request,
            CreateIssue {
                title: "First".into(),
                body: String::new()
            }
        );
    }
}
