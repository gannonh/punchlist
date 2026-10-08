//! A Rust client for the Punchlist API, used by `pl` and later by the runner and the MCP
//! server.

use punchlist_api::{
    AppendLog, Claim, ClaimRequest, CreateIssue, ErrorBody, Event, FinishRun, Issue, IssueList,
    MoveIssue, Moved, RegisterRunner, RegisteredRunner, Run, RunLog, Runner, RunnerList,
};
use reqwest::{Method, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server refused a transition. The message names the rule.
    #[error("refused: {message}")]
    Refused { code: String, message: String },
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

    pub async fn create_issue(&self, request: &CreateIssue) -> Result<Issue, ClientError> {
        self.send(Method::POST, &["issues"], Some(request)).await
    }

    pub async fn list_issues(&self) -> Result<IssueList, ClientError> {
        self.send::<(), _>(Method::GET, &["issues"], None).await
    }

    pub async fn get_issue(&self, id: &str) -> Result<Issue, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id], None).await
    }

    pub async fn move_issue(&self, id: &str, to: &str) -> Result<Moved, ClientError> {
        let request = MoveIssue { to: to.to_string() };
        self.send(Method::POST, &["issues", id, "transitions"], Some(&request))
            .await
    }

    pub async fn issue_events(&self, id: &str) -> Result<Vec<Event>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "events"], None)
            .await
    }

    /// Runs for an issue, newest first, each with its last 20 log lines.
    pub async fn issue_runs(&self, id: &str) -> Result<Vec<Run>, ClientError> {
        self.send::<(), _>(Method::GET, &["issues", id, "runs"], None)
            .await
    }

    /// Registers a runner. Call with a person's token; the response holds the runner's own
    /// token for every later runner call.
    pub async fn register_runner(
        &self,
        request: &RegisterRunner,
    ) -> Result<RegisteredRunner, ClientError> {
        self.send(Method::POST, &["runners"], Some(request)).await
    }

    pub async fn list_runners(&self) -> Result<RunnerList, ClientError> {
        self.send::<(), _>(Method::GET, &["runners"], None).await
    }

    /// Records a heartbeat and renews the runner's leases. Runner token.
    pub async fn heartbeat(&self, runner_id: &str) -> Result<Runner, ClientError> {
        self.send::<(), _>(Method::POST, &["runners", runner_id, "heartbeat"], None)
            .await
    }

    /// Claims one issue in Start, waiting up to `wait_seconds` for one. `None` when there
    /// was nothing to claim. A lost race is `ClientError::Refused` with code `claim_taken`.
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
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map_err(|()| ClientError::BadUrl(self.base_url.to_string()))?
            .pop_if_empty()
            .push("api")
            .extend(path);
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let text = response.text().await?;
        let error = serde_json::from_str::<ErrorBody>(&text).unwrap_or(ErrorBody {
            code: "unknown".into(),
            message: if text.is_empty() {
                status.to_string()
            } else {
                text
            },
        });
        Err(if status == StatusCode::CONFLICT {
            ClientError::Refused {
                code: error.code,
                message: error.message,
            }
        } else {
            ClientError::Api {
                status,
                code: error.code,
                message: error.message,
            }
        })
    }
}
