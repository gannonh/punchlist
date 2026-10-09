//! Reads files from a repository's default branch through GitHub's REST API, as the
//! GitHub App. The server uses it to load `.punchlist/` (ADR 0010).

use std::sync::Arc;

use anyhow::Context;
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use reqwest::StatusCode;
use serde::Deserialize;

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
}

#[derive(Deserialize)]
struct Installation {
    id: u64,
}

#[derive(Deserialize)]
struct AccessToken {
    token: String,
}

#[derive(Deserialize)]
struct RepositoryInfo {
    default_branch: String,
}

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
        }
    }

    /// A token for calls on one repository.
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
        let response = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(token)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .with_context(|| format!("GitHub {path}"))?;
        let status = response.status();
        anyhow::ensure!(status.is_success(), "GitHub {path} answered {status}");
        Ok(response)
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
        let commit: Commit = self
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
}

#[derive(Deserialize)]
struct Commit {
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
        let url = format!(
            "{}/repos/{}/contents/.punchlist/{path}?ref={}",
            self.client.base_url, self.repository, self.head.sha
        );
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
    fn pem_on_one_line_decodes() {
        let (label, der) =
            decode_pem("-----BEGIN RSA PRIVATE KEY----- AAEC Aw== -----END RSA PRIVATE KEY-----")
                .unwrap();
        assert_eq!((label.as_str(), der), ("RSA PRIVATE KEY", vec![0, 1, 2, 3]));
        assert!(decode_pem("not a key").is_err());
    }
}
