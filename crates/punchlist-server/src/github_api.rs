//! Reads GitHub as the GitHub App: files from a repository's default branch, to load
//! `.punchlist/` (ADR 0010), and the current state of pull requests and their review threads,
//! which webhooks alone cannot order.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::github::{Account, CheckRunData, PullRequestData, WorkflowJob};

/// How the server authenticates to GitHub.
#[derive(Clone)]
enum Auth {
    /// As the GitHub App: a JWT signed with its private key, exchanged per repository for
    /// an installation token.
    App {
        app_id: String,
        key: Arc<RsaKeyPair>,
    },
    /// A fixed token. Tests use it against a fake GitHub.
    Token(String),
}

#[derive(Clone)]
pub struct GithubClient {
    http: reqwest::Client,
    base_url: String,
    auth: Auth,
    /// The App's bot account, read once.
    bot: Arc<tokio::sync::OnceCell<Bot>>,
    /// Test-only: a run's token is reported to expire this long after it is minted, or
    /// sooner when GitHub's does, so a runner refreshes it before GitHub's hour is up.
    token_lifetime: Option<std::time::Duration>,
}

/// The account an installation token acts as.
#[derive(Clone)]
struct Bot {
    name: String,
    email: String,
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct AccessToken {
    token: String,
    #[serde(default)]
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    /// What the token may do, such as `"contents": "write"`.
    #[serde(default)]
    permissions: HashMap<String, String>,
}

#[derive(Deserialize)]
struct App {
    slug: String,
}

#[derive(Deserialize)]
struct User {
    id: u64,
}

/// What a run's token has to be able to do: push its branch, and open, comment on and merge
/// its pull request.
const RUN_PERMISSIONS: [&str; 2] = ["contents", "pull_requests"];

#[derive(Deserialize)]
struct RepositoryInfo {
    default_branch: String,
}

/// A review thread on a pull request, as GraphQL's `reviewThreads` reports it now.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GithubThread {
    /// The thread's node id: `thread.node_id` in a webhook.
    pub id: String,
    pub is_resolved: bool,
    /// The file the thread is on. A webhook's thread has it on its first comment.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    /// The line before the file changed under the thread, when `line` is gone.
    #[serde(default)]
    pub original_line: Option<i64>,
    #[serde(default)]
    pub comments: ThreadComments,
}

// ponytail: a thread's last `MAX_COMMENTS` comments are read, each cut to `MAX_BODY_CHARS`
// characters where it is parsed, and `MAX_THREAD_PAGES` pages of 100 threads, so no pull
// request can make a read, a row or an agent's context large. Page the comments and store
// them whole if a reviewer's earlier or longer text matters.
pub(crate) const MAX_COMMENTS: usize = 10;
pub(crate) const MAX_BODY_CHARS: usize = 2000;
const MAX_THREAD_PAGES: usize = 10;

/// A review thread's newest comments, oldest first, and how many it has.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadComments {
    pub total_count: i32,
    /// GraphQL lists null for a comment it cannot show.
    #[serde(deserialize_with = "present")]
    pub nodes: Vec<ThreadCommentNode>,
}

fn present<'de, D: serde::Deserializer<'de>>(nodes: D) -> Result<Vec<ThreadCommentNode>, D::Error> {
    let nodes = Vec::<Option<ThreadCommentNode>>::deserialize(nodes)?;
    Ok(nodes.into_iter().flatten().collect())
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct ThreadCommentNode {
    /// `None` when the account was deleted.
    #[serde(default)]
    pub author: Option<Account>,
    pub body: CommentBody,
    /// The comment on GitHub, as GitHub sent it.
    #[serde(default)]
    pub url: Option<String>,
}

/// A comment's text, at most `MAX_BODY_CHARS` characters of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommentBody {
    pub text: String,
    /// The comment was longer.
    pub cut: bool,
}

impl CommentBody {
    pub(crate) fn cut(body: &str) -> CommentBody {
        let end = body.char_indices().nth(MAX_BODY_CHARS).map(|(end, _)| end);
        CommentBody {
            // Postgres text cannot hold a NUL.
            text: body[..end.unwrap_or(body.len())].replace('\0', ""),
            cut: end.is_some(),
        }
    }
}

impl<'de> Deserialize<'de> for CommentBody {
    fn deserialize<D: serde::Deserializer<'de>>(body: D) -> Result<CommentBody, D::Error> {
        Ok(CommentBody::cut(&String::deserialize(body)?))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewThreadPage {
    nodes: Vec<GithubThread>,
    page_info: PageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

fn review_threads_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $number: Int!, $after: String) {{
  repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{
    reviewThreads(first: 100, after: $after) {{
      nodes {{
        id isResolved path line originalLine
        comments(last: {MAX_COMMENTS}) {{ totalCount nodes {{ author {{ login }} body url }} }}
      }}
      pageInfo {{ hasNextPage endCursor }}
    }}
  }} }}
}}"
    )
}

/// Every pull request from a branch of the repository, open or closed, with its base and its
/// base changes; a fork's has `isCrossRepository`. A page of each, which `other_bases` checks.
const BRANCH_PULL_REQUESTS_QUERY: &str =
    "query($owner: String!, $name: String!, $branch: String!) {
  repository(owner: $owner, name: $name) {
    defaultBranchRef { name }
    pullRequests(headRefName: $branch, first: 100, states: [OPEN, CLOSED, MERGED]) {
      pageInfo { hasNextPage }
      nodes {
        baseRefName isCrossRepository
        timelineItems(itemTypes: [BASE_REF_CHANGED_EVENT], first: 100) {
          pageInfo { hasNextPage }
          nodes { ... on BaseRefChangedEvent { previousRefName currentRefName } }
        }
      }
    }
  }
}";

/// A repository's default branch and the commit at its head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub branch: String,
    pub sha: String,
}

impl GithubClient {
    /// The GitHub App with `app_id` and its PEM private key. A key stored on one line, as
    /// 1Password Environments store it, gets its line breaks back.
    pub fn app(app_id: &str, private_key_pem: &str) -> anyhow::Result<GithubClient> {
        let (label, der) = decode_pem(private_key_pem)?;
        let key = match label.as_str() {
            "RSA PRIVATE KEY" => RsaKeyPair::from_der(&der),
            _ => RsaKeyPair::from_pkcs8(&der),
        }
        .map_err(|e| anyhow::anyhow!("the GitHub App private key is not an RSA key: {e}"))?;
        Ok(GithubClient::new(
            "https://api.github.com",
            Auth::App {
                app_id: app_id.to_string(),
                key: Arc::new(key),
            },
        ))
    }

    /// A client that sends `token` to `base_url`, such as a fake GitHub in tests.
    pub fn with_token(base_url: &str, token: &str) -> GithubClient {
        GithubClient::new(base_url, Auth::Token(token.to_string()))
    }

    fn new(base_url: &str, auth: Auth) -> GithubClient {
        GithubClient {
            http: reqwest::Client::builder()
                .user_agent(concat!("punchlist-server/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("a reqwest client builds"),
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            bot: Arc::default(),
            token_lifetime: None,
        }
    }

    /// Test-only: report a run's token to expire `lifetime` after it is minted.
    pub fn with_token_lifetime(mut self, lifetime: std::time::Duration) -> GithubClient {
        self.token_lifetime = Some(lifetime);
        self
    }

    /// A token for a run, limited to the one repository, that acts as the App's bot. An
    /// error when the App's installation cannot write the repository's contents and pull
    /// requests, since the run's push and pull request would then be refused.
    pub(crate) async fn run_token(
        &self,
        owner: &str,
        name: &str,
    ) -> anyhow::Result<punchlist_api::GithubToken> {
        // A fixed token stands in for the App's JWT against a fake GitHub.
        let app = match &self.auth {
            Auth::Token(token) => token.clone(),
            Auth::App { app_id, key } => app_jwt(app_id, key)?,
        };
        let installation: Installation = self
            .call(
                &app,
                reqwest::Method::GET,
                &format!("/repos/{owner}/{name}/installation"),
            )
            .await?
            .json()
            .await?;
        let minted: AccessToken = self
            .send(
                &app,
                reqwest::Method::POST,
                &format!("/app/installations/{}/access_tokens", installation.id),
                Some(&serde_json::json!({ "repositories": [name] })),
            )
            .await?
            .json()
            .await?;
        let short: Vec<String> = (RUN_PERMISSIONS.iter())
            .filter(|p| minted.permissions.get(**p).map(String::as_str) != Some("write"))
            .map(|p| {
                let has = minted.permissions.get(*p).map_or("none", String::as_str);
                format!("{p}: {has}")
            })
            .collect();
        if !short.is_empty() {
            anyhow::bail!(
                "the GitHub App's installation on {owner}/{name} grants {}; a run needs \
                 contents and pull_requests: write (GitHub App settings, Permissions, then \
                 accept them on the installation)",
                short.join(", ")
            );
        }
        let bot = self
            .bot
            .get_or_try_init(|| async {
                let app: App = (self.call(&app, reqwest::Method::GET, "/app").await?)
                    .json()
                    .await?;
                let name = format!("{}[bot]", app.slug);
                // `/users` takes an installation token, not the App's JWT.
                let user: User = self
                    .call(
                        &minted.token,
                        reqwest::Method::GET,
                        &format!("/users/{}%5Bbot%5D", app.slug),
                    )
                    .await?
                    .json()
                    .await?;
                let email = format!("{}+{name}@users.noreply.github.com", user.id);
                anyhow::Ok(Bot { name, email })
            })
            .await?;
        let now = chrono::Utc::now();
        // GitHub's tokens last an hour.
        let mut expires_at = minted
            .expires_at
            .unwrap_or(now + chrono::Duration::hours(1));
        if let Some(lifetime) = self.token_lifetime {
            expires_at = expires_at.min(now + chrono::Duration::from_std(lifetime)?);
        }
        Ok(punchlist_api::GithubToken {
            token: minted.token,
            expires_at,
            bot_name: bot.name.clone(),
            bot_email: bot.email.clone(),
        })
    }

    /// A token for calls on one repository.
    // ponytail: a new installation token per call, two requests each. When the App's rate
    // limit matters, cache the token until shortly before its `expires_at`.
    async fn token(&self, owner: &str, name: &str) -> anyhow::Result<String> {
        let (app_id, key) = match &self.auth {
            Auth::Token(token) => return Ok(token.clone()),
            Auth::App { app_id, key } => (app_id, key),
        };
        let jwt = app_jwt(app_id, key)?;
        let installation: Installation = self
            .call(
                &jwt,
                reqwest::Method::GET,
                &format!("/repos/{owner}/{name}/installation"),
            )
            .await?
            .json()
            .await?;
        let token: AccessToken = self
            .call(
                &jwt,
                reqwest::Method::POST,
                &format!("/app/installations/{}/access_tokens", installation.id),
            )
            .await?
            .json()
            .await?;
        Ok(token.token)
    }

    async fn call(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
    ) -> anyhow::Result<reqwest::Response> {
        self.send(token, method, path, None).await
    }

    /// `call`, with a JSON body when there is one.
    async fn send(
        &self,
        token: &str,
        method: reqwest::Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> anyhow::Result<reqwest::Response> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("GitHub {path}"))?;
        let status = response.status();
        // reqwest's error keeps the status, for `unless_gone`.
        response
            .error_for_status()
            .with_context(|| format!("GitHub {path} answered {status}"))
    }

    /// The head commit of the repository's default branch, whose files `Files` reads.
    pub async fn default_branch(&self, owner: &str, name: &str) -> anyhow::Result<Files> {
        let token = self.token(owner, name).await?;
        let repository: RepositoryInfo = self
            .call(
                &token,
                reqwest::Method::GET,
                &format!("/repos/{owner}/{name}"),
            )
            .await?
            .json()
            .await?;
        let commit: Object = self
            .call(
                &token,
                reqwest::Method::GET,
                &format!(
                    "/repos/{owner}/{name}/commits/{}",
                    repository.default_branch
                ),
            )
            .await?
            .json()
            .await?;
        Ok(Files {
            client: self.clone(),
            token,
            repository: format!("{owner}/{name}"),
            head: Head {
                branch: repository.default_branch,
                sha: commit.sha,
            },
        })
    }

    /// The current state of pull requests, read from GitHub now rather than from webhooks:
    /// every number in `known`, and every one of the repository's 30 most recently updated
    /// pull requests that `wanted` picks. The list omits `mergeable_state`, so each pull
    /// request is then read in full.
    // ponytail: a pull request that is not among the 30 most recently updated and not in
    // `known` is missed; page the list, or search by branch, if that bites.
    pub(crate) async fn pull_requests(
        &self,
        owner: &str,
        name: &str,
        known: &[i64],
        wanted: impl Fn(&PullRequestData) -> bool,
    ) -> anyhow::Result<Vec<PullRequestData>> {
        let token = self.token(owner, name).await?;
        let recent: Vec<PullRequestData> = self
            .call(
                &token,
                reqwest::Method::GET,
                &format!(
                    "/repos/{owner}/{name}/pulls?state=all&sort=updated&direction=desc&per_page=30"
                ),
            )
            .await?
            .json()
            .await?;
        let numbers: BTreeSet<i64> = known
            .iter()
            .copied()
            .chain(recent.iter().filter(|pr| wanted(pr)).map(|pr| pr.number))
            .collect();
        // One GitHub no longer has is left out; any other failure fails the read.
        let mut pull_requests = Vec::new();
        for number in numbers {
            pull_requests.extend(unless_gone(self.pull_request(owner, name, number).await)?);
        }
        Ok(pull_requests)
    }

    /// One pull request's current state.
    pub(crate) async fn pull_request(
        &self,
        owner: &str,
        name: &str,
        number: i64,
    ) -> anyhow::Result<PullRequestData> {
        let token = self.token(owner, name).await?;
        let path = format!("/repos/{owner}/{name}/pulls/{number}");
        Ok(self
            .call(&token, reqwest::Method::GET, &path)
            .await?
            .json()
            .await?)
    }

    /// Every check run on a commit, the superseded ones too (`pages`).
    pub(crate) async fn check_runs(
        &self,
        owner: &str,
        name: &str,
        sha: &str,
    ) -> anyhow::Result<Vec<CheckRunData>> {
        let token = self.token(owner, name).await?;
        let path = format!("/repos/{owner}/{name}/commits/{sha}/check-runs?filter=all");
        self.pages(&token, &path, "check_runs").await
    }

    /// The GitHub Actions jobs on a commit that a runner took, by check run id (a job's id is
    /// its check run's), each with its run's workflow file and that file's blob at the commit
    /// and on the default branch now. Only the workflow runs whose check suite is in `suites`
    /// are read. Needs the App's Actions: read permission.
    ///
    /// A check run that anything with `checks: write` created is listed as a job too, of the
    /// workflow run whose suite it joined, so being listed proves nothing. What its creator
    /// cannot set is a runner: such a job has `runner_id: null`, no labels and no steps. This
    /// is GitHub's behaviour as observed, not as documented; if GitHub ever lists such a job
    /// with a runner, a forged check counts again. Nor can a workflow rewrite a real job's
    /// check run: GitHub answers a `PATCH` of its `name` 200 and changes nothing, and one of
    /// its status or conclusion 403 (observed too). The caller still counts a job only for a
    /// check run of its name, conclusion and commit.
    ///
    /// The jobs are each run's latest attempt (GitHub's default filter), so while a rerun waits
    /// for a runner no job stands behind the commit's runs of that name, and none counts.
    ///
    /// Each job also has the other bases its run can have merged the commit into
    /// (`other_bases`), from its run's head branch: a run names no pull request of its own.
    // ponytail: one request for the runs, one per run in `suites` for its jobs, two per
    // workflow file and one per head branch, each list within `pages`' cap. Cache the default
    // branch's blobs per request if a refresh over many pull requests makes this cost.
    pub(crate) async fn workflow_jobs(
        &self,
        owner: &str,
        name: &str,
        sha: &str,
        suites: &HashSet<i64>,
    ) -> anyhow::Result<HashMap<i64, WorkflowJob>> {
        #[derive(Deserialize)]
        struct Run {
            id: i64,
            path: String,
            #[serde(default)]
            check_suite_id: Option<i64>,
            /// The branch of the run's event: for a pull request's run, its head branch.
            #[serde(default)]
            head_branch: Option<String>,
            /// Where that branch is: a fork's, for a pull request from one.
            #[serde(default)]
            head_repository: Option<RunRepository>,
        }
        #[derive(Deserialize)]
        struct RunRepository {
            full_name: String,
        }
        #[derive(Deserialize)]
        struct Job {
            id: i64,
            name: String,
            #[serde(default)]
            conclusion: Option<String>,
            head_sha: String,
            #[serde(default)]
            runner_id: Option<i64>,
        }
        let token = self.token(owner, name).await?;
        let repository = format!("{owner}/{name}");
        let runs = format!("/repos/{owner}/{name}/actions/runs?head_sha={sha}");
        let runs: Vec<Run> = self.pages(&token, &runs, "workflow_runs").await?;
        let mut jobs = HashMap::new();
        let mut blobs: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        let mut bases: HashMap<String, Option<Vec<String>>> = HashMap::new();
        let in_suites = |run: &Run| run.check_suite_id.is_some_and(|id| suites.contains(&id));
        for run in runs.into_iter().filter(in_suites) {
            let listed = format!(
                "/repos/{owner}/{name}/actions/runs/{}/jobs?filter=latest",
                run.id
            );
            let listed: Vec<Job> = self.pages(&token, &listed, "jobs").await?;
            // A hosted runner's id is a large number; 0 is not one.
            let ran = |job: &Job| job.runner_id.unwrap_or(0) != 0;
            // A branch of a fork, or none, has no pull request here that the run was for.
            let branch = (run.head_branch.as_ref()).filter(|_| {
                (run.head_repository.as_ref())
                    .is_some_and(|head| head.full_name.eq_ignore_ascii_case(&repository))
            });
            for job in listed.into_iter().filter(ran) {
                let blob = match blobs.get(&run.path) {
                    Some(blob) => blob.clone(),
                    None => {
                        let at = |r| self.blob(&token, &repository, &run.path, r);
                        let blob = (at(Some(sha)).await?, at(None).await?);
                        blobs.insert(run.path.clone(), blob.clone());
                        blob
                    }
                };
                let other_bases = match branch {
                    None => None,
                    Some(branch) => match bases.get(branch) {
                        Some(other) => other.clone(),
                        None => {
                            let other = self.other_bases(&token, owner, name, branch).await?;
                            bases.insert(branch.clone(), other.clone());
                            other
                        }
                    },
                };
                jobs.insert(
                    job.id,
                    WorkflowJob {
                        name: job.name,
                        conclusion: job.conclusion,
                        head_sha: job.head_sha,
                        path: run.path.clone(),
                        sha: blob.0,
                        default_sha: blob.1,
                        other_bases,
                    },
                );
            }
        }
        Ok(jobs)
    }

    /// The bases other than the default branch that the pull requests from `branch` of this
    /// repository have been into, open or closed, each one's earlier bases too (GraphQL's
    /// `BaseRefChangedEvent`), sorted; `None` when no pull request is from it. A pull request's
    /// run takes its workflow file from the head commit merged into its base, so these are every
    /// base a run from the branch can have had. More than a page of pull requests, or of one's
    /// base changes, is an error: a list cut short could leave a base out.
    async fn other_bases(
        &self,
        token: &str,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> anyhow::Result<Option<Vec<String>>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Repository {
            default_branch_ref: Option<Named>,
            pull_requests: Nodes<PullRequest>,
        }
        #[derive(Deserialize)]
        struct Named {
            name: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Nodes<T> {
            page_info: PageInfo,
            nodes: Vec<T>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct PullRequest {
            base_ref_name: String,
            is_cross_repository: bool,
            timeline_items: Nodes<BaseChange>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct BaseChange {
            previous_ref_name: String,
            current_ref_name: String,
        }
        let query = serde_json::json!({
            "query": BRANCH_PULL_REQUESTS_QUERY,
            "variables": {"owner": owner, "name": name, "branch": branch},
        });
        let mut answer: serde_json::Value = self
            .send(token, reqwest::Method::POST, "/graphql", Some(&query))
            .await?
            .json()
            .await?;
        let Some(repository) = answer
            .pointer_mut("/data/repository")
            .map(serde_json::Value::take)
        else {
            anyhow::bail!(
                "GitHub GraphQL has no pull requests from {branch:?}: {}",
                answer["errors"]
            );
        };
        let repository: Repository = serde_json::from_value(repository)?;
        let default = (repository.default_branch_ref)
            .context("GitHub GraphQL names no default branch")?
            .name;
        let pulls = repository.pull_requests;
        anyhow::ensure!(
            !pulls.page_info.has_next_page,
            "more pull requests are from {branch:?} than are read"
        );
        let mut other = BTreeSet::new();
        let mut any = false;
        for pr in pulls.nodes.into_iter().filter(|pr| !pr.is_cross_repository) {
            let changes = pr.timeline_items;
            anyhow::ensure!(
                !changes.page_info.has_next_page,
                "a pull request from {branch:?} changed its base more often than is read"
            );
            any = true;
            other.insert(pr.base_ref_name);
            for change in changes.nodes {
                other.extend([change.previous_ref_name, change.current_ref_name]);
            }
        }
        other.remove(&default);
        Ok(any.then(|| other.into_iter().collect()))
    }

    /// Every item of a REST list that answers `{"total_count": n, "<key>": [...]}`, a page of
    /// 100 at a time. The list ends on a short page; `total_count` is not trusted for that, as
    /// an item created between pages shifts the list. More than `PAGES` pages (unless
    /// `total_count` says the last was the end) is an error, so that the refresh, which has no
    /// deadline of its own, always ends, and no list is cut short: what is left out, the gates
    /// would judge as absent.
    async fn pages<T: serde::de::DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        key: &str,
    ) -> anyhow::Result<Vec<T>> {
        const PER_PAGE: usize = 100;
        const PAGES: usize = 10;
        let mut items: Vec<serde_json::Value> = Vec::new();
        let mut total = None;
        for page in 1..=PAGES {
            let paged = format!("{path}&per_page={PER_PAGE}&page={page}");
            let mut answer: serde_json::Value = self
                .call(token, reqwest::Method::GET, &paged)
                .await?
                .json()
                .await?;
            let listed: Option<Vec<serde_json::Value>> =
                serde_json::from_value(answer[key].take()).ok();
            let (Some(count), Some(listed)) = (answer["total_count"].as_u64(), listed) else {
                anyhow::bail!("GitHub {path:?} has no `total_count` and list `{key}`");
            };
            // An item created between two pages shifts the list: a later page repeats the
            // boundary item, or the count moves. The read is then not one list, and fails
            // closed; the agent asks again.
            anyhow::ensure!(
                *total.get_or_insert(count) == count
                    && (listed.iter()).all(|new| items.iter().all(|seen| seen["id"] != new["id"])),
                "GitHub's `{key}` at {path:?} changed between pages"
            );
            let short = listed.len() < PER_PAGE;
            items.extend(listed);
            // Exactly the cap fills the last page: GitHub's count says nothing is left.
            if short || (page == PAGES && count as usize <= items.len()) {
                return Ok((items.into_iter())
                    .map(serde_json::from_value)
                    .collect::<Result<_, _>>()?);
            }
        }
        anyhow::bail!(
            "GitHub lists more than {} `{key}` at {path:?}",
            PER_PAGE * PAGES
        )
    }

    /// The blob (or, for a directory, tree) at `path` in a commit, or on the default branch
    /// with no `git_ref`; `None` when there is nothing at the path. `path` is a workflow
    /// run's, so its author's text: it is logged quoted.
    async fn blob(
        &self,
        token: &str,
        repository: &str,
        path: &str,
        git_ref: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let url = contents_url(&self.base_url, repository, path, git_ref)?;
        let response = self
            .http
            .get(url)
            .bearer_auth(token)
            // An object for a directory too, where the plain JSON is a list.
            .header("accept", "application/vnd.github.object+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("GitHub contents of {path:?}"))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(response.json::<Object>().await?.sha)),
            status => anyhow::bail!("GitHub contents of {path:?} answered {status}"),
        }
    }

    /// Every review thread on a pull request, resolved or not. REST has no review threads, so
    /// this is GraphQL, a page of 100 at a time. GraphQL answers 200 with `errors` and no
    /// data when it refuses, so a missing list is an error. So is a pull request with more
    /// than `MAX_THREAD_PAGES` pages: a list cut short would forget its other threads, and
    /// `no_open_threads` would pass over them.
    pub(crate) async fn review_threads(
        &self,
        owner: &str,
        name: &str,
        number: i64,
    ) -> anyhow::Result<Vec<GithubThread>> {
        let token = &self.token(owner, name).await?;
        let mut threads = Vec::new();
        let mut after = None;
        let text = review_threads_query();
        for _ in 0..MAX_THREAD_PAGES {
            let query = serde_json::json!({
                "query": text,
                "variables": {"owner": owner, "name": name, "number": number, "after": after},
            });
            let mut answer: serde_json::Value = self
                .send(token, reqwest::Method::POST, "/graphql", Some(&query))
                .await?
                .json()
                .await?;
            let page = answer
                .pointer_mut("/data/repository/pullRequest/reviewThreads")
                .map(serde_json::Value::take);
            let Some(page) = page else {
                anyhow::bail!(
                    "GitHub GraphQL has no review threads for {owner}/{name}#{number}: {}",
                    answer["errors"]
                );
            };
            let page: ReviewThreadPage = serde_json::from_value(page)?;
            threads.extend(page.nodes);
            if !page.page_info.has_next_page {
                return Ok(threads);
            }
            let cursor = page.page_info.end_cursor;
            let cursor = cursor.context("GitHub GraphQL named no cursor for the next page")?;
            // The gated-request refresh has no deadline of its own, so a page that names the
            // cursor it was asked for must not loop.
            anyhow::ensure!(
                after.as_deref() != Some(cursor.as_str()),
                "GitHub GraphQL repeated the cursor {cursor}"
            );
            after = Some(cursor);
        }
        anyhow::bail!(
            "{owner}/{name}#{number} has more than {} review threads, which is more than is read",
            MAX_THREAD_PAGES * 100
        )
    }
}

/// `None` when GitHub answered 404 or 410: what was asked for is gone, and no retry finds it.
/// Any other failure is still the error.
pub(crate) fn unless_gone<T>(read: anyhow::Result<T>) -> anyhow::Result<Option<T>> {
    let status = |error: &anyhow::Error| error.downcast_ref::<reqwest::Error>()?.status();
    match read {
        Err(error)
            if matches!(
                status(&error),
                Some(StatusCode::NOT_FOUND | StatusCode::GONE)
            ) =>
        {
            Ok(None)
        }
        read => read.map(Some),
    }
}

/// A commit, blob or tree, as far as its sha.
#[derive(Deserialize)]
struct Object {
    sha: String,
}

/// The files of one commit, read on demand.
pub struct Files {
    client: GithubClient,
    token: String,
    repository: String,
    pub head: Head,
}

impl Files {
    /// The text of `.punchlist/<path>`, or `None` when the commit has no such file.
    pub async fn punchlist_file(&self, path: &str) -> anyhow::Result<Option<String>> {
        let url = contents_url(
            &self.client.base_url,
            &self.repository,
            &format!(".punchlist/{path}"),
            Some(&self.head.sha),
        )?;
        let response = self
            .client
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("accept", "application/vnd.github.raw+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("GitHub .punchlist/{path}"))?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(None),
            status if status.is_success() => Ok(Some(response.text().await?)),
            status => anyhow::bail!("GitHub .punchlist/{path} answered {status}"),
        }
    }
}

/// The URL of the file at `path`, at `sha` or on the default branch without one. Each path
/// segment and the query are escaped, so a prompt named `a?b.md` is that file, not a query
/// that swallows `ref`.
fn contents_url(
    base_url: &str,
    repository: &str,
    path: &str,
    sha: Option<&str>,
) -> anyhow::Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(base_url)?;
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("{base_url} cannot be a base URL"))?
        .pop_if_empty()
        .extend(["repos"])
        .extend(repository.split('/'))
        .extend(["contents"])
        .extend(path.split('/'));
    if let Some(sha) = sha {
        url.query_pairs_mut().append_pair("ref", sha);
    }
    Ok(url)
}

/// A JWT that authenticates as the App for ten minutes, backdated a minute for clock drift.
fn app_jwt(app_id: &str, key: &RsaKeyPair) -> anyhow::Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = URL_SAFE_NO_PAD.encode(format!(
        r#"{{"iat":{},"exp":{},"iss":"{app_id}"}}"#,
        now - 60,
        now + 540
    ));
    let message = format!("{header}.{claims}");
    let mut signature = vec![0; key.public_modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        message.as_bytes(),
        &mut signature,
    )
    .map_err(|_| anyhow::anyhow!("cannot sign the GitHub App JWT"))?;
    Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
}

/// The label and DER bytes of a PEM block. Whitespace inside the body is ignored, so a key
/// whose line breaks were turned into spaces still decodes.
fn decode_pem(pem: &str) -> anyhow::Result<(String, Vec<u8>)> {
    let rest = pem
        .trim()
        .strip_prefix("-----BEGIN ")
        .context("the GitHub App private key is not PEM")?;
    let (label, rest) = rest
        .split_once("-----")
        .context("the GitHub App private key is not PEM")?;
    let end = format!("-----END {label}-----");
    let body = rest
        .strip_suffix(end.as_str())
        .context("the GitHub App private key's PEM has no matching END line")?;
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    Ok((label.to_string(), STANDARD.decode(body)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prompt_path_cannot_leave_its_place_in_the_url() {
        assert_eq!(
            contents_url(
                "http://x",
                "o/n",
                ".punchlist/prompts/a?b#c d%.md",
                Some("abc")
            )
            .unwrap()
            .as_str(),
            "http://x/repos/o/n/contents/.punchlist/prompts/a%3Fb%23c%20d%25.md?ref=abc"
        );
        assert_eq!(
            contents_url("http://x", "o/n", ".github/workflows/ci.yml", None)
                .unwrap()
                .as_str(),
            "http://x/repos/o/n/contents/.github/workflows/ci.yml"
        );
    }

    #[test]
    fn pem_on_one_line_decodes() {
        let (label, der) =
            decode_pem("-----BEGIN RSA PRIVATE KEY----- AAEC Aw== -----END RSA PRIVATE KEY-----")
                .unwrap();
        assert_eq!((label.as_str(), der), ("RSA PRIVATE KEY", vec![0, 1, 2, 3]));
        assert!(decode_pem("not a key").is_err());
    }
}
