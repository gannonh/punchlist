// Copied from kata-symphony apps/symphony/src/workspace.rs at e7c160f5; trimmed: kept validate_workspace_path, scan_workspace_root, the git command helpers and worktree removal; dropped docker, hooks, ssh, skill-ignore, refresh policy and the clone/bootstrap strategies (worktree.rs creates worktrees instead).
//! Workspace path checks, workspace scanning and cleanup.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, SymphonyError};
use crate::path_safety;

/// Validate that `workspace` is a proper child of `root`:
/// - Not equal to root
/// - Canonically under root/
/// - No symlink escapes
pub fn validate_workspace_path(workspace: &Path, root: &Path) -> Result<()> {
    let canonical_workspace = path_safety::canonicalize(workspace)?;
    let canonical_root = path_safety::canonicalize(root)?;

    if canonical_workspace == canonical_root {
        return Err(SymphonyError::WorkspaceOutsideRoot {
            workspace: canonical_workspace.to_string_lossy().to_string(),
            root: canonical_root.to_string_lossy().to_string(),
        });
    }

    // Must be a proper descendant of root (component-level check)
    if !canonical_workspace.starts_with(&canonical_root) {
        return Err(SymphonyError::WorkspaceOutsideRoot {
            workspace: canonical_workspace.to_string_lossy().to_string(),
            root: canonical_root.to_string_lossy().to_string(),
        });
    }

    Ok(())
}

/// Scan workspace directories and map them by issue identifier.
///
/// The canonical workspace layout is `<root>/<identifier>`, but we also scan the
/// branch-prefixed fallback `<root>/<branch_prefix>/<identifier>` to cover legacy/manual
/// directory layouts.
///
/// This is used by orchestrator startup cleanup to recover orphan workspace paths for issues
/// that reached terminal state while Symphony was not running.
pub fn scan_workspace_root(root: &Path, branch_prefix: &str) -> HashMap<String, PathBuf> {
    let mut discovered = HashMap::new();

    scan_workspace_directory(root, &mut discovered);

    let normalized_prefix = branch_prefix.trim_matches('/');
    if normalized_prefix.is_empty() {
        return discovered;
    }

    let prefix_path = normalized_prefix
        .split('/')
        .fold(root.to_path_buf(), |acc, segment| acc.join(segment));

    if prefix_path != root {
        scan_workspace_directory(&prefix_path, &mut discovered);
    }

    discovered
}

fn scan_workspace_directory(scan_root: &Path, discovered: &mut HashMap<String, PathBuf>) {
    let entries = match std::fs::read_dir(scan_root) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!(
                event = "startup_workspace_scan_unavailable",
                scan_root = %scan_root.display(),
                error = %err,
                "workspace directory unavailable during startup scan"
            );
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!(
                    event = "startup_workspace_scan_entry_error",
                    scan_root = %scan_root.display(),
                    error = %err,
                    "failed to read workspace entry; skipping"
                );
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                tracing::warn!(
                    event = "startup_workspace_scan_file_type_error",
                    path = %entry.path().display(),
                    error = %err,
                    "failed to inspect workspace entry type; skipping"
                );
                continue;
            }
        };

        if !file_type.is_dir() {
            continue;
        }

        let identifier = entry
            .file_name()
            .to_str()
            .map(str::trim)
            .filter(|candidate| looks_like_issue_identifier(candidate))
            .map(ToString::to_string);

        let Some(identifier) = identifier else {
            continue;
        };

        let path = std::fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
        tracing::debug!(
            event = "startup_workspace_scan_match",
            issue_identifier = %identifier,
            workspace_path = %path.display(),
            "discovered startup workspace candidate"
        );

        discovered.entry(identifier).or_insert(path);
    }
}

fn looks_like_issue_identifier(value: &str) -> bool {
    let Some((team, number)) = value.split_once('-') else {
        return false;
    };

    !team.is_empty()
        && !number.is_empty()
        && team.chars().all(|ch| ch.is_ascii_alphanumeric())
        && number.chars().all(|ch| ch.is_ascii_digit())
}

/// Remove a worktree checkout from the source repository, then its directory.
pub fn remove_worktree(repo: &Path, workspace: &Path, root: &Path) -> Result<()> {
    if !workspace.exists() {
        return Ok(());
    }
    validate_workspace_path(workspace, root)?;
    let mut worktree_remove = Command::new("git");
    worktree_remove
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(workspace);
    run_git_command(worktree_remove, "workspace worktree cleanup")?;
    if workspace.exists() {
        std::fs::remove_dir_all(workspace)?;
    }
    Ok(())
}

pub(crate) fn run_git_command(mut command: Command, context: &str) -> Result<()> {
    let output = command.output().map_err(SymphonyError::Io)?;
    if output.status.success() {
        return Ok(());
    }

    git_output_error(output, context)
}

pub(crate) fn git_output_error<T>(output: std::process::Output, context: &str) -> Result<T> {
    let status = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    let redacted = combined;
    let truncated = truncate_output(&redacted, 2048);

    Err(SymphonyError::Other(format!(
        "{context} failed (status {status}): {truncated}"
    )))
}

/// Truncate output to max_bytes, appending "... (truncated)" if necessary.
fn truncate_output(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        output.to_string()
    } else {
        // Find a safe UTF-8 boundary
        let truncated = &output[..output.floor_char_boundary(max_bytes)];
        format!("{}... (truncated)", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::scan_workspace_root;

    #[test]
    fn scan_workspace_root_maps_matching_directories() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path();

        let matching_a = root.join("KAT-100");
        let matching_b = root.join("KAT-200");
        let non_matching = root.join("not-an-issue");

        std::fs::create_dir_all(&matching_a).expect("matching workspace A should be created");
        std::fs::create_dir_all(&matching_b).expect("matching workspace B should be created");
        std::fs::create_dir_all(&non_matching).expect("non-matching directory should exist");

        let discovered = scan_workspace_root(root, "symphony");
        let expected_a = std::fs::canonicalize(&matching_a).expect("canonical path should resolve");
        let expected_b = std::fs::canonicalize(&matching_b).expect("canonical path should resolve");

        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered.get("KAT-100"), Some(&expected_a));
        assert_eq!(discovered.get("KAT-200"), Some(&expected_b));
    }

    #[test]
    fn scan_workspace_root_supports_nested_branch_prefix() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path();

        let nested_match = root.join("symphony").join("backend").join("KAT-900");
        std::fs::create_dir_all(&nested_match)
            .expect("nested branch prefix workspace should be created");

        let discovered = scan_workspace_root(root, "symphony/backend");
        let expected = std::fs::canonicalize(&nested_match).expect("canonical path should resolve");
        assert_eq!(discovered.get("KAT-900"), Some(&expected));
    }

    #[test]
    fn scan_workspace_root_falls_back_to_root_when_prefix_missing() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let root = temp.path();

        let root_match = root.join("KAT-321");
        std::fs::create_dir_all(&root_match).expect("root workspace should be created");

        let discovered = scan_workspace_root(root, "symphony");
        let expected = std::fs::canonicalize(&root_match).expect("canonical path should resolve");
        assert_eq!(discovered.get("KAT-321"), Some(&expected));
    }
}
