//! Worktree preparation (ADR 0005): a shared clone per repository and one git worktree per
//! issue, on the issue's branch. New code; it uses the copied `path_safety` and `workspace`.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use punchlist_api::Repository;

use crate::path_safety::{canonicalize, sanitize_identifier};
use crate::workspace::{git_output_error, validate_workspace_path};

/// The longest one git command may run before it is killed, such as a clone from a remote
/// that stopped answering.
const GIT_TIMEOUT: Duration = Duration::from_secs(600);

/// Set when the `prepare` future is dropped, so the blocking git work stops with it.
#[derive(Clone, Default)]
struct Cancel(Arc<AtomicBool>);

impl Cancel {
    fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

struct CancelOnDrop(Cancel);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.0.store(true, Ordering::Relaxed);
    }
}

#[derive(Debug)]
pub struct Worktrees {
    root: PathBuf,
    /// Remote URLs are `<remote_base>/<owner>/<name>.git`.
    remote_base: String,
    /// One lock per repository so concurrent claims do not race on the clone.
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Worktrees {
    pub fn new(root: PathBuf) -> Worktrees {
        Worktrees {
            root,
            remote_base: "https://github.com".to_string(),
            locks: Default::default(),
        }
    }

    /// Test-only: clone from `<remote_base>/<owner>/<name>.git`, such as a local directory.
    #[cfg(test)]
    pub fn with_remote_base(root: PathBuf, remote_base: String) -> Worktrees {
        Worktrees {
            remote_base,
            ..Worktrees::new(root)
        }
    }

    /// Makes the issue's worktree and returns its path. Reuses it when it already exists
    /// on `branch`. Dropping the future kills the git command it is running.
    pub async fn prepare(
        &self,
        repository: &Repository,
        branch: &str,
        issue_id: &str,
    ) -> Result<PathBuf, String> {
        let key = format!("{}/{}", repository.owner, repository.name);
        let lock = self
            .locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .clone();
        let _guard = lock.lock().await;
        let root = self.root.clone();
        let remote_base = self.remote_base.clone();
        let repository = repository.clone();
        let branch = branch.to_string();
        let issue_id = issue_id.to_string();
        let cancel = Cancel::default();
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        tokio::task::spawn_blocking(move || {
            let git = Git {
                cancel,
                timeout: GIT_TIMEOUT,
            };
            prepare_blocking(&git, &root, &remote_base, &repository, &branch, &issue_id)
        })
        .await
        .map_err(|e| format!("worktree task failed: {e}"))?
    }
}

fn safe_segment(value: &str, what: &str) -> Result<String, String> {
    let sanitized = sanitize_identifier(value);
    if sanitized != value || sanitized == "." || sanitized == ".." {
        return Err(format!("unsafe {what} `{value}`"));
    }
    Ok(sanitized)
}

/// Runs git commands that end when they finish, time out, or are cancelled.
struct Git {
    cancel: Cancel,
    timeout: Duration,
}

impl Git {
    fn output(&self, dir: Option<&Path>, args: &[&str], context: &str) -> Result<Output, String> {
        let mut command = Command::new("git");
        if let Some(dir) = dir {
            command.arg("-C").arg(dir);
        }
        let mut child = command
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so a kill also reaches helpers such as git-remote-https.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("{context}: cannot run git: {e}"))?;
        // Read both pipes on their own threads so a chatty command cannot fill one and block.
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let mut stderr = child.stderr.take().expect("stderr is piped");
        let out = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stdout.read_to_end(&mut buf);
            buf
        });
        let err = std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| format!("{context}: cannot wait for git: {e}"))?
            {
                break status;
            }
            let stop = if self.cancel.is_set() {
                Some(format!("{context}: cancelled"))
            } else if Instant::now() >= deadline {
                Some(format!(
                    "{context}: timed out after {}s",
                    self.timeout.as_secs()
                ))
            } else {
                None
            };
            if let Some(reason) = stop {
                crate::agent::kill_group(Some(child.id()));
                let _ = child.kill();
                let _ = child.wait();
                return Err(reason);
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        Ok(Output {
            status,
            stdout: out.join().unwrap_or_default(),
            stderr: err.join().unwrap_or_default(),
        })
    }

    fn run(&self, dir: Option<&Path>, args: &[&str], context: &str) -> Result<String, String> {
        let output = self.output(dir, args, context)?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
        }
        git_output_error::<String>(output, context).map_err(|e| e.to_string())
    }

    /// Whether the command succeeded. A cancel or timeout is an error, not `false`.
    fn ok(&self, dir: &Path, args: &[&str], context: &str) -> Result<bool, String> {
        Ok(self.output(Some(dir), args, context)?.status.success())
    }
}

fn prepare_blocking(
    git: &Git,
    root: &Path,
    remote_base: &str,
    repository: &Repository,
    branch: &str,
    issue_id: &str,
) -> Result<PathBuf, String> {
    let owner = safe_segment(&repository.owner, "repository owner")?;
    let name = safe_segment(&repository.name, "repository name")?;
    let issue_dir = sanitize_identifier(issue_id);
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    let root = canonicalize(root).map_err(|e| e.to_string())?;

    let clone_dir = root.join("repos").join(&owner).join(&name);
    validate_workspace_path(&clone_dir, &root).map_err(|e| e.to_string())?;
    let default_branch = repository.default_branch.as_str();
    if clone_dir.join(".git").exists() {
        git.run(
            Some(&clone_dir),
            &["fetch", "origin", default_branch],
            "git fetch",
        )?;
    } else {
        std::fs::create_dir_all(clone_dir.parent().expect("clone dir has a parent"))
            .map_err(|e| e.to_string())?;
        let url = format!("{remote_base}/{owner}/{name}.git");
        git.run(
            None,
            &["clone", &url, &clone_dir.to_string_lossy()],
            "git clone",
        )?;
    }

    let worktree = root
        .join("worktrees")
        .join(&owner)
        .join(&name)
        .join(&issue_dir);
    validate_workspace_path(&worktree, &root).map_err(|e| e.to_string())?;
    if worktree.exists() {
        let head = git.run(
            Some(&worktree),
            &["rev-parse", "--abbrev-ref", "HEAD"],
            "git rev-parse",
        )?;
        if head == branch {
            return Ok(worktree);
        }
        return Err(format!(
            "worktree {} is on `{head}`, not `{branch}`",
            worktree.display()
        ));
    }
    std::fs::create_dir_all(worktree.parent().expect("worktree has a parent"))
        .map_err(|e| e.to_string())?;

    let worktree_arg = worktree.to_string_lossy().to_string();
    let local_ref = format!("refs/heads/{branch}");
    if git.ok(
        &clone_dir,
        &["rev-parse", "--verify", "--quiet", &local_ref],
        "git rev-parse",
    )? {
        git.run(
            Some(&clone_dir),
            &["worktree", "add", &worktree_arg, branch],
            "git worktree add",
        )?;
    } else {
        // An earlier attempt may have pushed the branch; continue from it when it did.
        git.ok(&clone_dir, &["fetch", "origin", branch], "git fetch")?;
        let remote_ref = format!("refs/remotes/origin/{branch}");
        let start = if git.ok(
            &clone_dir,
            &["rev-parse", "--verify", "--quiet", &remote_ref],
            "git rev-parse",
        )? {
            format!("origin/{branch}")
        } else {
            format!("origin/{default_branch}")
        };
        git.run(
            Some(&clone_dir),
            &["worktree", "add", "-b", branch, &worktree_arg, &start],
            "git worktree add",
        )?;
    }
    Ok(worktree)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: Option<&Path>, args: &[&str], context: &str) -> Result<String, String> {
        Git {
            cancel: Cancel::default(),
            timeout: GIT_TIMEOUT,
        }
        .run(dir, args, context)
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        git(Some(dir), args, "test git").expect("git should succeed")
    }

    /// A git command that runs `sleep 30` through a shell alias.
    const HANG: &[&str] = &["-c", "alias.hang=!sleep 30", "hang"];

    #[test]
    fn a_git_command_that_hangs_is_killed_at_the_timeout() {
        let git = Git {
            cancel: Cancel::default(),
            timeout: Duration::from_secs(1),
        };
        let started = Instant::now();
        assert_eq!(
            git.run(None, HANG, "git hang").unwrap_err(),
            "git hang: timed out after 1s"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn dropping_the_cancel_guard_kills_the_running_git_command() {
        let cancel = Cancel::default();
        let git = Git {
            cancel: cancel.clone(),
            timeout: GIT_TIMEOUT,
        };
        let guard = CancelOnDrop(cancel);
        let dropper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            drop(guard);
        });
        let started = Instant::now();
        assert_eq!(
            git.run(None, HANG, "git hang").unwrap_err(),
            "git hang: cancelled"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        dropper.join().unwrap();
    }

    /// A bare origin at `<base>/acme/widgets.git` with one commit on `main`.
    fn make_origin(base: &Path) -> String {
        let bare = base.join("acme").join("widgets.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(Some(&bare), &["init", "--bare", "-b", "main"], "init").unwrap();
        let seed = base.join("seed");
        git(
            None,
            &["clone", &bare.to_string_lossy(), &seed.to_string_lossy()],
            "clone",
        )
        .unwrap();
        std::fs::write(seed.join("README.md"), "hello\n").unwrap();
        run(&seed, &["add", "."]);
        run(
            &seed,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-m",
                "init",
            ],
        );
        run(&seed, &["push", "origin", "HEAD:main"]);
        run(&seed, &["rev-parse", "HEAD"])
    }

    fn repository() -> Repository {
        Repository {
            owner: "acme".into(),
            name: "widgets".into(),
            default_branch: "main".into(),
        }
    }

    #[tokio::test]
    async fn prepare_creates_the_worktree_on_the_branch_and_reuses_it() {
        let temp = tempfile::tempdir().unwrap();
        let origin = temp.path().join("origin");
        let main_sha = make_origin(&origin);
        let worktrees = Worktrees::with_remote_base(
            temp.path().join("root"),
            origin.to_string_lossy().to_string(),
        );
        let branch = "feature/pl-1-first-issue";

        let path = worktrees
            .prepare(&repository(), branch, "PL-1")
            .await
            .unwrap();
        assert!(path.ends_with("worktrees/acme/widgets/PL-1"));
        assert!(path.join("README.md").exists());
        assert_eq!(run(&path, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
        assert_eq!(run(&path, &["rev-parse", "HEAD"]), main_sha);

        std::fs::write(path.join("marker.txt"), "kept").unwrap();
        let again = worktrees
            .prepare(&repository(), branch, "PL-1")
            .await
            .unwrap();
        assert_eq!(again, path);
        assert_eq!(
            std::fs::read_to_string(path.join("marker.txt")).unwrap(),
            "kept"
        );

        let err = worktrees
            .prepare(&repository(), "feature/other", "PL-1")
            .await
            .unwrap_err();
        assert!(err.contains("is on `feature/pl-1-first-issue`, not `feature/other`"));
    }

    #[tokio::test]
    async fn prepare_rejects_unsafe_repository_names() {
        let temp = tempfile::tempdir().unwrap();
        let worktrees = Worktrees::with_remote_base(temp.path().join("root"), "/nowhere".into());
        let mut repo = repository();
        repo.owner = "../evil".into();
        let err = worktrees
            .prepare(&repo, "feature/x", "PL-1")
            .await
            .unwrap_err();
        assert_eq!(err, "unsafe repository owner `../evil`");
    }
}
