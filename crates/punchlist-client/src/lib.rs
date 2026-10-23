//! A Rust client for the Punchlist API, used by `pl` and later by the runner and the MCP
//! server.

use punchlist_api::{
    ActorList, AppendLog, Claim, ClaimRequest, CreateAgent, CreateComment, CreateIssue,
    CreateMilestone, CreateProject, CreatedAgent, ErrorBody, Event, FinishRun, GateChecklist,
    GithubToken, Heartbeat, HeartbeatReply, ImportRun, ImportStatus, Issue, IssueList,
    IssueWritten, Lease, LinearImport, Milestone, MoveIssue, Moved, OpenPullRequest, PROOF_TYPES,
    Project, ProjectList, Proof, ProofKind, PullRequest, PullRequestsNow, RefreshGithubToken,
    RegisterRunner, RegisteredRunner, ResumeRun, Run, RunLog, RunnerList, Session, UpdateIssue,
    WaitForPerson, WorkflowInfo, proof_type_by_extension,
};
use reqwest::{Method, RequestBuilder, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server refused: a transition, a claim or a stale attempt. The message names the
    /// rule; a failed gate also carries the gate, its reason and every gate's result.
    #[error("refused: {}", .0.message)]
    Refused(Box<ErrorBody>),
    /// Any other error response.
    #[error("{message} ({status})")]
    Api {
        status: StatusCode,
        code: String,
        message: String,
    },
    #[error("server URL `{0}` is not a valid http(s) URL")]
    BadUrl(String),
    #[error("cannot reach the Punchlist server: {0}")]
    Http(#[from] reqwest::Error),
    /// A file to upload could not be read or is not a kind the server accepts. Nothing was
    /// sent.
    #[error("{0}")]
    File(String),
}

#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    token: String,
}

impl Client {
    pub fn new(base_url: &str, token: &str) -> Result<Client, ClientError> {
        let base_url =
            Url::parse(base_url).map_err(|_| ClientError::BadUrl(base_url.to_string()))?;
        Ok(Client {
            http: reqwest::Client::builder()
                .user_agent(concat!("punchlist-client/", env!("CARGO_PKG_VERSION")))
                // Above the longest claim long-poll (30 s), so a hung request fails instead
                // of blocking a caller, such as the runner's log flusher, forever.
                .timeout(std::time::Duration::from_secs(90))
                .build()?,
            base_url,
            token: token.to_string(),
        })
    }

    pub async fn create_issue(&self, request: &CreateIssue) -> Result<IssueWritten, ClientError> {
        self.send(Method::POST, &["issues"], Some(request)).await
    }

    pub async fn list_issues(&self) -> Result<IssueList, ClientError> {
        self.send::<(), _>(Method::GET, &["issues"], None).await
    }

    pub async fn get_issue(&self, id: &str) -> Result<Issue, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id], None).await
    }

    /// Edits an issue's fields; an absent field is left as it is.
    pub async fn update_issue(
        &self,
        id: &str,
        request: &UpdateIssue,
    ) -> Result<IssueWritten, ClientError> {
        self.send(Method::PATCH, &["issues", id], Some(request))
            .await
    }

    /// Who the token belongs to, and its workspace.
    pub async fn session(&self) -> Result<Session, ClientError> {
        self.send::<(), _>(Method::GET, &["session"], None).await
    }

    /// Queues an import of a Linear project into the workspace.
    pub async fn import_linear(
        &self,
        workspace_id: &str,
        snapshot: &LinearImport,
    ) -> Result<ImportRun, ClientError> {
        let path = ["workspaces", workspace_id, "imports", "linear"];
        self.send(Method::POST, &path, Some(snapshot)).await
    }

    pub async fn import_run(&self, workspace_id: &str, run: i64) -> Result<ImportRun, ClientError> {
        let run = run.to_string();
        let path = ["workspaces", workspace_id, "imports", &run];
        self.send::<(), _>(Method::GET, &path, None).await
    }

    pub async fn import_status(&self, workspace_id: &str) -> Result<ImportStatus, ClientError> {
        let path = ["workspaces", workspace_id, "imports"];
        self.send::<(), _>(Method::GET, &path, None).await
    }

    pub async fn list_projects(&self) -> Result<ProjectList, ClientError> {
        self.send::<(), _>(Method::GET, &["projects"], None).await
    }

    pub async fn create_project(&self, request: &CreateProject) -> Result<Project, ClientError> {
        self.send(Method::POST, &["projects"], Some(request)).await
    }

    pub async fn create_milestone(
        &self,
        project_id: &str,
        request: &CreateMilestone,
    ) -> Result<Milestone, ClientError> {
        let path = ["projects", project_id, "milestones"];
        self.send(Method::POST, &path, Some(request)).await
    }

    pub async fn move_issue(&self, id: &str, to: &str) -> Result<Moved, ClientError> {
        let request = MoveIssue { to: to.to_string() };
        self.send(Method::POST, &["issues", id, "transitions"], Some(&request))
            .await
    }

    /// Comments on an issue. An agent's comment ends with `(agent)`.
    pub async fn comment(&self, id: &str, body: &str) -> Result<Event, ClientError> {
        let request = CreateComment {
            body: body.to_string(),
        };
        self.send(Method::POST, &["issues", id, "comments"], Some(&request))
            .await
    }

    /// Uploads a file as proof for the issue. Its content type comes from its extension;
    /// `kind`, when given, must be that type's kind.
    pub async fn attach_proof(
        &self,
        id: &str,
        path: &std::path::Path,
        title: &str,
        kind: Option<ProofKind>,
    ) -> Result<Proof, ClientError> {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let (content_type, _) = proof_type_by_extension(extension).ok_or_else(|| {
            let accepted: Vec<&str> = PROOF_TYPES.iter().map(|(ext, _, _)| *ext).collect();
            ClientError::File(format!(
                "{} is not a file proof can be; use one of .{}",
                path.display(),
                accepted.join(", .")
            ))
        })?;
        // ponytail: the whole file is read into memory, up to the server's size limit.
        // Upgrade path: stream it.
        let bytes = std::fs::read(path)
            .map_err(|error| ClientError::File(format!("read {}: {error}", path.display())))?;
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let file = reqwest::multipart::Part::bytes(bytes)
            .file_name(name)
            .mime_str(content_type)?;
        let mut form = reqwest::multipart::Form::new()
            .text("title", title.to_string())
            .part("file", file);
        if let Some(kind) = kind {
            form = form.text("kind", kind.as_str());
        }
        let request = self
            .builder(Method::POST, &["issues", id, "proof"])?
            .multipart(form);
        Ok(Self::respond(request).await?.json().await?)
    }

    /// An issue's proof, oldest first.
    pub async fn list_proof(&self, id: &str) -> Result<Vec<Proof>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "proof"], None)
            .await
    }

    /// The workspace's active workflow.
    pub async fn workflow(&self) -> Result<WorkflowInfo, ClientError> {
        self.send::<(), _>(Method::GET, &["workflow"], None).await
    }

    pub async fn issue_events(&self, id: &str) -> Result<Vec<Event>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "events"], None)
            .await
    }

    /// The gates of the issue's next gated transition, checked against the recorded
    /// evidence. `None` when the issue's status has no gated transition.
    pub async fn issue_gates(&self, id: &str) -> Result<Option<GateChecklist>, ClientError> {
        self.send_optional::<(), _>(Method::GET, &["issues", id, "gates"], None)
            .await
    }

    /// Runs for an issue, newest first, each with its last 20 log lines.
    pub async fn issue_runs(&self, id: &str) -> Result<Vec<Run>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "runs"], None)
            .await
    }

    /// An issue's pull requests, newest number first.
    pub async fn issue_pull_requests(&self, id: &str) -> Result<Vec<PullRequest>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "pull-requests"], None)
            .await
    }

    /// An issue's pull requests and their unresolved review threads, after the server has
    /// read both from GitHub.
    pub async fn refresh_issue_pull_requests(
        &self,
        id: &str,
    ) -> Result<PullRequestsNow, ClientError> {
        let path = ["issues", id, "pull-requests", "refresh"];
        self.send::<(), _>(Method::POST, &path, None).await
    }

    /// The open and draft pull requests of the workspace's repository, newest number first.
    /// A person's token.
    pub async fn pull_requests(&self) -> Result<Vec<OpenPullRequest>, ClientError> {
        self.send::<(), _>(Method::GET, &["pull-requests"], None)
            .await
    }

    /// The pull requests that name no issue, newest number first.
    pub async fn unlinked_pull_requests(&self) -> Result<Vec<PullRequest>, ClientError> {
        self.send::<(), _>(Method::GET, &["pull-requests", "unlinked"], None)
            .await
    }

    /// Registers a runner, or re-adopts the one this person registered under the same name.
    /// Call with a person's token; the response holds the runner's own token for every later
    /// runner call, and an earlier token of a re-adopted runner stops working.
    pub async fn register_runner(
        &self,
        request: &RegisterRunner,
    ) -> Result<RegisteredRunner, ClientError> {
        self.send(Method::POST, &["runners"], Some(request)).await
    }

    pub async fn list_runners(&self) -> Result<RunnerList, ClientError> {
        self.send::<(), _>(Method::GET, &["runners"], None).await
    }

    /// Mints an agent actor with its own token. Person token; the token is in the response
    /// only.
    pub async fn create_agent(&self, request: &CreateAgent) -> Result<CreatedAgent, ClientError> {
        self.send(Method::POST, &["actors"], Some(request)).await
    }

    pub async fn list_actors(&self) -> Result<ActorList, ClientError> {
        self.send::<(), _>(Method::GET, &["actors"], None).await
    }

    /// Records a heartbeat and renews the leases of the attempts in `heartbeat.held`. The
    /// reply lists the ones that are stale. Runner token.
    pub async fn heartbeat(
        &self,
        runner_id: &str,
        heartbeat: &Heartbeat,
    ) -> Result<HeartbeatReply, ClientError> {
        self.send(
            Method::POST,
            &["runners", runner_id, "heartbeat"],
            Some(heartbeat),
        )
        .await
    }

    /// The running attempts the runner holds whose leases have not lapsed. Runner token.
    pub async fn leases(&self, runner_id: &str) -> Result<Vec<Lease>, ClientError> {
        self.send::<(), _>(Method::GET, &["runners", runner_id, "leases"], None)
            .await
    }

    /// Resumes an attempt the runner holds: renews its lease and returns its claim again. A
    /// lapsed lease is `ClientError::Refused` with code `stale_attempt`. Runner token.
    pub async fn resume_run(&self, run_id: &str, attempt: i32) -> Result<Claim, ClientError> {
        let request = ResumeRun { attempt };
        self.send(Method::POST, &["runs", run_id, "resume"], Some(&request))
            .await
    }

    /// A fresh GitHub token for the runner's running attempt. Runner token; a lapsed lease is
    /// `ClientError::Refused` with code `stale_attempt`.
    pub async fn refresh_github_token(
        &self,
        run_id: &str,
        attempt: i32,
    ) -> Result<GithubToken, ClientError> {
        let request = RefreshGithubToken { attempt };
        self.send(
            Method::POST,
            &["runs", run_id, "github-token"],
            Some(&request),
        )
        .await
    }

    /// Claims one issue, in Start or waiting in a status with a dispatch rule, waiting up to
    /// `wait_seconds` for one. `None` when there was nothing to claim. A lost race is `ClientError::Refused` with code `claim_taken`.
    pub async fn claim(&self, wait_seconds: u32) -> Result<Option<Claim>, ClientError> {
        let request = ClaimRequest { wait_seconds };
        self.send_optional(Method::POST, &["runs", "claim"], Some(&request))
            .await
    }

    pub async fn append_log(&self, run_id: &str, request: &AppendLog) -> Result<(), ClientError> {
        self.send_optional::<_, serde_json::Value>(
            Method::POST,
            &["runs", run_id, "log"],
            Some(request),
        )
        .await
        .map(|_| ())
    }

    pub async fn finish_run(&self, run_id: &str, request: &FinishRun) -> Result<Run, ClientError> {
        self.send(Method::POST, &["runs", run_id, "finish"], Some(request))
            .await
    }

    /// Says the run cannot go on without a person, and what the person must do. The run's
    /// agent token only.
    pub async fn wait_for_person(&self, run_id: &str, reason: &str) -> Result<(), ClientError> {
        let request = WaitForPerson {
            reason: reason.to_string(),
        };
        self.send_optional::<_, serde_json::Value>(
            Method::POST,
            &["runs", run_id, "waiting"],
            Some(&request),
        )
        .await
        .map(|_| ())
    }

    pub async fn run_log(&self, run_id: &str) -> Result<RunLog, ClientError> {
        self.send::<(), _>(Method::GET, &["runs", run_id, "log"], None)
            .await
    }

    async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let response = self.request(method, path, body).await?;
        Ok(response.json().await?)
    }

    /// Like `send`, but 204 No Content is `None`.
    async fn send_optional<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        body: Option<&B>,
    ) -> Result<Option<T>, ClientError> {
        let response = self.request(method, path, body).await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        Ok(Some(response.json().await?))
    }

    async fn request<B: Serialize>(
        &self,
        method: Method,
        path: &[&str],
        body: Option<&B>,
    ) -> Result<reqwest::Response, ClientError> {
        let mut request = self.builder(method, path)?;
        if let Some(body) = body {
            request = request.json(body);
        }
        Self::respond(request).await
    }

    /// A request to `/api/<path>` with the token.
    fn builder(&self, method: Method, path: &[&str]) -> Result<RequestBuilder, ClientError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| ClientError::BadUrl(self.base_url.to_string()))?
            .pop_if_empty()
            .push("api")
            .extend(path);
        Ok(self.http.request(method, url).bearer_auth(&self.token))
    }

    /// Sends the request; an error status is a `ClientError`.
    async fn respond(request: RequestBuilder) -> Result<reqwest::Response, ClientError> {
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await?;
        let error = serde_json::from_str::<ErrorBody>(&text).unwrap_or_else(|_| {
            ErrorBody::new(
                "unknown",
                if text.is_empty() {
                    status.to_string()
                } else {
                    text
                },
            )
        });
        Err(if status == StatusCode::CONFLICT {
            ClientError::Refused(Box::new(error))
        } else {
            ClientError::Api {
                status,
                code: error.code,
                message: error.message,
            }
        })
    }
}
