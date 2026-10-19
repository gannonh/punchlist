//! Reads one Linear project for `pl import linear` through Linear's GraphQL API, with the
//! personal API key in `LINEAR_API_KEY`. The key stays in this process: the server gets only
//! what was read. Parsing a response is pure, and tested over recorded responses.

use anyhow::Context;
use chrono::{DateTime, Utc};
use punchlist_api::{
    LinearComment, LinearImport, LinearIssue, LinearMilestone, LinearProject, LinearState,
    LinearUser,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;

const API_URL: &str = "https://api.linear.app/graphql";

/// The team by its key, and its project by name, with the project's milestones. Every
/// connection takes `first`: Linear counts one without it as 50 toward its complexity limit of
/// 10,000, and `teams` unbounded made this query 13,185.
pub const PROJECT_QUERY: &str = "query Project($team: String!, $project: String!) {
  teams(filter: {key: {eq: $team}}, first: 1) {
    nodes {
      key
      projects(filter: {name: {eq: $project}}, first: 2) {
        nodes { id name url projectMilestones(first: 100) { nodes { id name description } } }
      }
    }
  }
}";

/// One page of the project's issues. Pages and comment pages are kept small for Linear's
/// query complexity limit: 50 issues, each with 50 comments and 20 labels.
pub const ISSUES_QUERY: &str = "query Issues($team: String!, $project: ID!, $after: String) {
  issues(filter: {team: {key: {eq: $team}}, project: {id: {eq: $project}}}, first: 50, after: $after) {
    nodes {
      id identifier url title description createdAt updatedAt
      state { name type }
      labels(first: 20) { nodes { name } }
      parent { id }
      projectMilestone { id }
      creator { id name }
      comments(first: 50) {
        nodes { id body createdAt user { id name } }
        pageInfo { hasNextPage endCursor }
      }
    }
    pageInfo { hasNextPage endCursor }
  }
}";

/// The next page of one issue's comments.
pub const COMMENTS_QUERY: &str = "query Comments($issue: String!, $after: String) {
  issue(id: $issue) {
    comments(first: 100, after: $after) {
      nodes { id body createdAt user { id name } }
      pageInfo { hasNextPage endCursor }
    }
  }
}";

#[derive(Deserialize)]
struct Response<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    nodes: Vec<T>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

impl PageInfo {
    fn next(self) -> Option<String> {
        self.end_cursor.filter(|_| self.has_next_page)
    }
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Teams {
    teams: Nodes<Team>,
}

#[derive(Deserialize)]
struct Team {
    projects: Nodes<Project>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Project {
    id: String,
    name: String,
    url: String,
    project_milestones: Nodes<Milestone>,
}

#[derive(Deserialize)]
struct Milestone {
    id: String,
    name: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Issues {
    issues: Page<Issue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Issue {
    id: String,
    identifier: String,
    url: String,
    title: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    state: LinearState,
    labels: Nodes<Label>,
    parent: Option<Id>,
    project_milestone: Option<Id>,
    creator: Option<LinearUser>,
    comments: Page<Comment>,
}

#[derive(Deserialize)]
struct Label {
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Comment {
    id: String,
    body: String,
    created_at: DateTime<Utc>,
    user: Option<LinearUser>,
}

impl From<Comment> for LinearComment {
    fn from(c: Comment) -> LinearComment {
        LinearComment {
            id: c.id,
            body: c.body,
            user: c.user,
            created_at: c.created_at,
        }
    }
}

#[derive(Deserialize)]
struct IssueComments {
    issue: Option<CommentsOf>,
}

#[derive(Deserialize)]
struct CommentsOf {
    comments: Page<Comment>,
}

/// A response's data, or Linear's errors.
fn data<T: DeserializeOwned>(json: &str) -> anyhow::Result<T> {
    let response: Response<T> = serde_json::from_str(json)
        .context("Linear answered with something that is not a GraphQL response")?;
    if !response.errors.is_empty() {
        let messages: Vec<&str> = response.errors.iter().map(|e| e.message.as_str()).collect();
        anyhow::bail!("Linear refused the query: {}", messages.join("; "));
    }
    response.data.context("Linear answered with no data")
}

/// `PROJECT_QUERY`'s answer: the project named `project` in the team keyed `team`.
pub fn parse_project(json: &str, team: &str, project: &str) -> anyhow::Result<LinearProject> {
    let teams: Teams = data(json)?;
    let found = teams
        .teams
        .nodes
        .into_iter()
        .next()
        .with_context(|| format!("Linear has no team with the key {team}"))?;
    let mut projects = found.projects.nodes.into_iter();
    let found = projects
        .next()
        .with_context(|| format!("Linear team {team} has no project named {project}"))?;
    anyhow::ensure!(
        projects.next().is_none(),
        "Linear team {team} has more than one project named {project}"
    );
    Ok(LinearProject {
        id: found.id,
        name: found.name,
        url: found.url,
        milestones: (found.project_milestones.nodes.into_iter())
            .map(|m| LinearMilestone {
                id: m.id,
                name: m.name,
                description: m.description,
            })
            .collect(),
    })
}

/// One page of `ISSUES_QUERY`'s answer.
#[derive(Debug)]
pub struct IssuesPage {
    pub issues: Vec<LinearIssue>,
    /// Each issue with more comments than the page holds, and the cursor after the last.
    pub more_comments: Vec<(String, String)>,
    /// The cursor of the next page of issues, when there is one.
    pub next: Option<String>,
}

pub fn parse_issues(json: &str) -> anyhow::Result<IssuesPage> {
    let page: Issues = data(json)?;
    let mut more_comments = Vec::new();
    let issues = (page.issues.nodes.into_iter())
        .map(|issue| {
            if let Some(cursor) = issue.comments.page_info.next() {
                more_comments.push((issue.id.clone(), cursor));
            }
            LinearIssue {
                id: issue.id,
                identifier: issue.identifier,
                url: issue.url,
                title: issue.title,
                description: issue.description,
                state: issue.state,
                labels: issue.labels.nodes.into_iter().map(|l| l.name).collect(),
                parent_id: issue.parent.map(|p| p.id),
                milestone_id: issue.project_milestone.map(|m| m.id),
                creator: issue.creator,
                created_at: issue.created_at,
                updated_at: issue.updated_at,
                comments: issue.comments.nodes.into_iter().map(Into::into).collect(),
            }
        })
        .collect();
    Ok(IssuesPage {
        issues,
        more_comments,
        next: page.issues.page_info.next(),
    })
}

/// `COMMENTS_QUERY`'s answer: the comments, and the cursor of the next page when there is one.
pub fn parse_comments(json: &str) -> anyhow::Result<(Vec<LinearComment>, Option<String>)> {
    let found: IssueComments = data(json)?;
    let page = found
        .issue
        .context("Linear no longer has the issue")?
        .comments;
    Ok((
        page.nodes.into_iter().map(Into::into).collect(),
        page.page_info.next(),
    ))
}

pub struct Linear {
    http: reqwest::Client,
    url: String,
    key: String,
}

impl Linear {
    /// Reads the key from `LINEAR_API_KEY`. `LINEAR_API_URL` replaces Linear's endpoint, for
    /// tests.
    pub fn from_env() -> anyhow::Result<Linear> {
        let key = std::env::var("LINEAR_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .context(
                "LINEAR_API_KEY is not set; pl reads Linear with your personal API key from the \
                 environment and never stores it, such as `with-env pl import linear …`",
            )?;
        Ok(Linear {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()?,
            url: std::env::var("LINEAR_API_URL").unwrap_or_else(|_| API_URL.to_string()),
            key: key.trim().to_string(),
        })
    }

    async fn query(&self, query: &str, variables: serde_json::Value) -> anyhow::Result<String> {
        // A personal API key goes in the header as it is, with no `Bearer`.
        let response = (self.http.post(&self.url))
            .header("authorization", &self.key)
            .json(&json!({"query": query, "variables": variables}))
            .send()
            .await
            .context("cannot reach Linear")?;
        Ok(response.text().await?)
    }

    /// The project named `project` in the team keyed `team`, with every issue in it and every
    /// comment on them.
    pub async fn read(&self, team: &str, project: &str) -> anyhow::Result<LinearImport> {
        let vars = json!({"team": team, "project": project});
        let found = parse_project(&self.query(PROJECT_QUERY, vars).await?, team, project)?;
        let mut issues = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let vars = json!({"team": team, "project": found.id, "after": after});
            let mut page = parse_issues(&self.query(ISSUES_QUERY, vars).await?)?;
            for (id, cursor) in page.more_comments {
                let issue = (page.issues.iter_mut())
                    .find(|i| i.id == id)
                    .expect("a page's comment cursor is for one of its issues");
                let mut after = Some(cursor);
                while let Some(cursor) = after {
                    let vars = json!({"issue": id, "after": cursor});
                    let (comments, next) =
                        parse_comments(&self.query(COMMENTS_QUERY, vars).await?)?;
                    issue.comments.extend(comments);
                    after = next;
                }
            }
            issues.extend(page.issues);
            match page.next {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }
        Ok(LinearImport {
            team: team.to_string(),
            project: found,
            issues,
        })
    }
}
