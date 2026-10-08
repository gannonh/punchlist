//! A Rust client for the Punchlist API, used by `pl` and later by the runner and the MCP
//! server.

use punchlist_api::{CreateIssue, ErrorBody, Event, Issue, IssueList, MoveIssue, Moved};
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
            http: reqwest::Client::new(),
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

    async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &[&str],
        body: Option<&B>,
    ) -> Result<T, ClientError> {
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
            return Ok(response.json().await?);
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
