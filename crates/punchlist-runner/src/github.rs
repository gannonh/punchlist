//! A run's GitHub credential: the installation token the server mints for the run, as the
//! workspace's GitHub App. The agent's `gh` and `git`, and the runner's own git for the run,
//! read it from files that a refresh rewrites in place, since a running agent's environment
//! cannot change. They get no other GitHub credential of the person's.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use punchlist_api::GithubToken;

/// Hands git the token in the file `PUNCHLIST_GITHUB_TOKEN_FILE` names.
const CREDENTIAL_HELPER: &str = r#"!f() { test "$1" = get && printf 'username=x-access-token\npassword=%s\n' "$(cat "$PUNCHLIST_GITHUB_TOKEN_FILE")"; }; f"#;

/// Variables that carry a GitHub credential, or a way to one, into the agent.
const REMOVED: [&str; 7] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "SSH_AUTH_SOCK",
    "GIT_ASKPASS",
    "GIT_CONFIG_PARAMETERS",
];

/// `<root>/github/<run id>/`: `token`, read by git's credential helper, and `gh/hosts.yml`,
/// `gh`'s login. Removed when dropped.
pub(crate) struct GithubAccess {
    dir: PathBuf,
    bot_name: String,
    bot_email: String,
}

impl GithubAccess {
    pub(crate) fn create(
        root: &Path,
        run_id: &str,
        token: &GithubToken,
    ) -> std::io::Result<GithubAccess> {
        // The bot's name goes into YAML: GitHub's bot logins are letters, digits, `-` and
        // `[bot]`.
        let plain = |b: u8| b.is_ascii_alphanumeric() || b"-[]".contains(&b);
        if token.bot_name.is_empty() || !token.bot_name.bytes().all(plain) {
            return Err(std::io::Error::other(
                "the GitHub bot's name is not a plain login",
            ));
        }
        let dir = std::path::absolute(root.join("github").join(run_id))?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir.join("gh"))?;
        let access = GithubAccess {
            dir,
            bot_name: token.bot_name.clone(),
            bot_email: token.bot_email.clone(),
        };
        access.write(&token.token)?;
        Ok(access)
    }

    /// Replaces the token where `gh` and git read it. Each file is replaced whole, so a
    /// reader sees the old token or the new one.
    pub(crate) fn write(&self, token: &str) -> std::io::Result<()> {
        // It goes into YAML below; GitHub's tokens are letters, digits, `_`, `-` and `.`.
        let plain = |b: u8| b.is_ascii_alphanumeric() || b"_-.".contains(&b);
        if token.is_empty() || !token.bytes().all(plain) {
            return Err(std::io::Error::other(
                "the GitHub token is not a plain token",
            ));
        }
        replace(&self.dir.join("token"), token)?;
        // gh's multi-account layout, `version: "1"`. In the older one gh migrates the file
        // first, asking `GET /user` for the login, which an installation token may not read.
        replace(&self.dir.join("gh").join("config.yml"), "version: \"1\"\n")?;
        let user = &self.bot_name;
        let hosts = format!(
            "github.com:\n    users:\n        \"{user}\":\n            oauth_token: {token}\n    \
             git_protocol: https\n    user: \"{user}\"\n    oauth_token: {token}\n"
        );
        replace(&self.dir.join("gh").join("hosts.yml"), &hosts)
    }

    /// The agent's environment: `gh` logged in with the token alone, git handed it for
    /// `https://github.com` and writing commits as the App's bot, and none of the person's
    /// git configuration, credential variables or SSH agent. `None` removes a variable.
    pub(crate) fn env(&self) -> Vec<(&'static str, Option<OsString>)> {
        let mut env: Vec<(&'static str, Option<OsString>)> =
            REMOVED.iter().map(|name| (*name, None)).collect();
        let set = |value: &str| Some(OsString::from(value));
        env.extend([
            ("GH_CONFIG_DIR", Some(self.dir.join("gh").into_os_string())),
            (
                "PUNCHLIST_GITHUB_TOKEN_FILE",
                Some(self.dir.join("token").into_os_string()),
            ),
            // The person's global and system git config may name credential helpers.
            ("GIT_CONFIG_GLOBAL", set("/dev/null")),
            ("GIT_CONFIG_NOSYSTEM", set("1")),
            ("GIT_CONFIG_COUNT", set("1")),
            (
                "GIT_CONFIG_KEY_0",
                set("credential.https://github.com.helper"),
            ),
            ("GIT_CONFIG_VALUE_0", set(CREDENTIAL_HELPER)),
            ("GIT_TERMINAL_PROMPT", set("0")),
            ("GIT_AUTHOR_NAME", set(&self.bot_name)),
            ("GIT_AUTHOR_EMAIL", set(&self.bot_email)),
            ("GIT_COMMITTER_NAME", set(&self.bot_name)),
            ("GIT_COMMITTER_EMAIL", set(&self.bot_email)),
        ]);
        env
    }
}

impl Drop for GithubAccess {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Writes `text` to a new file readable by its owner alone, then renames it over `path`.
fn replace(path: &Path, text: &str) -> std::io::Result<()> {
    let new = path.with_extension("new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&new, path)
}

/// When to ask for the next token: ten minutes before `expires_at`, or halfway there for a
/// token that lasts less than twenty minutes, so a short test token is not refreshed in a
/// loop.
pub(crate) fn refresh_at(
    received_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    let lead = chrono::Duration::minutes(10).min((expires_at - received_at) / 2);
    expires_at - lead.max(chrono::Duration::zero())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn token(token: &str) -> GithubToken {
        GithubToken {
            token: token.into(),
            expires_at: chrono::Utc::now(),
            bot_name: "punchlist-sartre[bot]".into(),
            bot_email: "42+punchlist-sartre[bot]@users.noreply.github.com".into(),
        }
    }

    /// `command` run with the access's environment over the runner's own, which has a
    /// person's GitHub token and SSH agent.
    fn run(access: &GithubAccess, dir: &Path, command: &str) -> String {
        let mut shell = Command::new("sh");
        shell
            .args(["-c", command])
            .current_dir(dir)
            .env("GH_TOKEN", "gho_person")
            .env("GITHUB_TOKEN", "gho_person")
            .env("SSH_AUTH_SOCK", "/tmp/agent.sock");
        for (name, value) in access.env() {
            match value {
                Some(value) => shell.env(name, value),
                None => shell.env_remove(name),
            };
        }
        let output = shell.output().unwrap();
        assert!(output.status.success(), "{command}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn git_and_gh_get_the_runs_token_and_nothing_of_the_persons() {
        let temp = tempfile::tempdir().unwrap();
        let access = GithubAccess::create(temp.path(), "run-1", &token("ghs_first")).unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run(&access, &repo, "git init -q");

        let credential = "printf 'protocol=https\\nhost=github.com\\n\\n' | git credential fill";
        assert_eq!(
            run(&access, &repo, credential),
            "protocol=https\nhost=github.com\nusername=x-access-token\npassword=ghs_first\n"
        );
        // Unrelated remotes must never receive the installation token.
        for (protocol, host) in [
            ("https", "attacker.example"),
            ("https", "github.com.attacker.example"),
            ("http", "github.com"),
        ] {
            assert_eq!(
                run(
                    &access,
                    &repo,
                    &format!(
                        "if printf 'protocol={protocol}\\nhost={host}\\n\\n' | \
                         git -c core.askPass=false credential fill 2>/dev/null; \
                         then exit 1; fi"
                    )
                ),
                ""
            );
        }
        assert_eq!(
            run(
                &access,
                &repo,
                "echo \"${GH_TOKEN-unset} ${GITHUB_TOKEN-unset} ${SSH_AUTH_SOCK-unset}\""
            ),
            "unset unset unset\n"
        );
        assert_eq!(
            run(&access, &repo, "cat \"$GH_CONFIG_DIR/hosts.yml\""),
            "github.com:\n    users:\n        \"punchlist-sartre[bot]\":\n            \
             oauth_token: ghs_first\n    git_protocol: https\n    \
             user: \"punchlist-sartre[bot]\"\n    oauth_token: ghs_first\n"
        );
        assert_eq!(
            run(&access, &repo, "cat \"$GH_CONFIG_DIR/config.yml\""),
            "version: \"1\"\n"
        );
        run(&access, &repo, "git commit -q --allow-empty -m one");
        assert_eq!(
            run(
                &access,
                &repo,
                "git log -1 --format='%an <%ae> / %cn <%ce>'"
            ),
            "punchlist-sartre[bot] <42+punchlist-sartre[bot]@users.noreply.github.com> / \
             punchlist-sartre[bot] <42+punchlist-sartre[bot]@users.noreply.github.com>\n"
        );

        // A refresh reaches a git already configured with the old token.
        // GitHub's newer tokens are longer, with `-` and `.`.
        access.write("ghs_second.a-b").unwrap();
        assert!(run(&access, &repo, credential).ends_with("password=ghs_second.a-b\n"));
        assert!(
            run(&access, &repo, "cat \"$GH_CONFIG_DIR/hosts.yml\"")
                .contains("oauth_token: ghs_second.a-b\n")
        );

        assert!(access.write("ghs_x\n    user: someone").is_err());
        let mut forged = token("ghs_first");
        forged.bot_name = "bot\"\n    oauth_token: x".into();
        assert!(GithubAccess::create(temp.path(), "run-2", &forged).is_err());
        let dir = access.dir.clone();
        drop(access);
        assert!(!dir.exists());
    }

    #[test]
    fn a_token_is_refreshed_ten_minutes_before_it_expires_or_halfway_for_a_short_one() {
        let t0 = chrono::DateTime::<chrono::Utc>::from_timestamp(1_000_000, 0).unwrap();
        let minutes = chrono::Duration::minutes;
        assert_eq!(refresh_at(t0, t0 + minutes(60)), t0 + minutes(50));
        assert_eq!(refresh_at(t0, t0 + minutes(2)), t0 + minutes(1));
        assert_eq!(refresh_at(t0, t0 - minutes(1)), t0 - minutes(1));
    }
}
