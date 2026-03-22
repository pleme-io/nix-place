//! File sync operations — write flake.nix atomically, manage git context.

use crate::model::FlakeSpec;
use crate::render;
use crate::validate;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug)]
pub enum SyncAction {
    Created,
    Updated { changed_apps: Vec<String> },
    Unchanged,
}

/// Sync a `FlakeSpec` to a target directory.
///
/// - Renders the spec to `flake.nix`
/// - Writes atomically (temp file + rename)
/// - Initializes git if needed
/// - Manages `.gitignore`
pub fn sync(spec: &FlakeSpec, target: &Path) -> Result<SyncAction, crate::Error> {
    let content = render::render(spec);

    // Validate generated Nix before writing
    let validation = validate::validate_nix(&content);
    if !validation.valid {
        return Err(crate::Error::Other(format!(
            "Generated Nix has syntax errors:\n{validation}"
        )));
    }

    let flake_path = target.join("flake.nix");

    // Check if content has changed
    if flake_path.exists() {
        let existing = fs::read_to_string(&flake_path)?;
        if content_hash(&existing) == content_hash(&content) {
            return Ok(SyncAction::Unchanged);
        }

        // Find which apps changed
        let changed_apps = diff_apps(&existing, &content);
        write_atomic(&flake_path, &content)?;
        ensure_git(target)?;
        ensure_gitignore(target)?;
        return Ok(SyncAction::Updated { changed_apps });
    }

    // First write
    fs::create_dir_all(target)?;
    write_atomic(&flake_path, &content)?;
    ensure_git(target)?;
    ensure_gitignore(target)?;
    Ok(SyncAction::Created)
}

/// Show what would change without writing.
pub fn diff(spec: &FlakeSpec, target: &Path) -> Result<String, crate::Error> {
    let content = render::render(spec);
    let flake_path = target.join("flake.nix");

    if !flake_path.exists() {
        return Ok(format!("Would create {} ({} apps, {} flows)",
            flake_path.display(),
            spec.apps.len(),
            spec.flows.len(),
        ));
    }

    let existing = fs::read_to_string(&flake_path)?;
    if content_hash(&existing) == content_hash(&content) {
        return Ok("No changes.".to_string());
    }

    let changed = diff_apps(&existing, &content);
    if changed.is_empty() {
        Ok("Content changed (non-app changes).".to_string())
    } else {
        Ok(format!("Changed apps: {}", changed.join(", ")))
    }
}

/// Remove a managed flake.nix.
pub fn clean(target: &Path) -> Result<bool, crate::Error> {
    let flake_path = target.join("flake.nix");
    if flake_path.exists() {
        fs::remove_file(&flake_path)?;
        // Also clean fleet.yaml if it exists
        let fleet_path = target.join("fleet.yaml");
        if fleet_path.exists() {
            fs::remove_file(&fleet_path)?;
        }
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Atomic write: write to temp file in same dir, then rename.
fn write_atomic(path: &Path, content: &str) -> Result<(), crate::Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temp = tempfile::NamedTempFile::new_in(dir)?;
    fs::write(temp.path(), content)?;
    temp.persist(path)?;
    Ok(())
}

/// Ensure the target directory has a git repo initialized.
fn ensure_git(target: &Path) -> Result<(), crate::Error> {
    let git_dir = target.join(".git");
    if git_dir.exists() {
        return Ok(());
    }

    Command::new("git")
        .args(["init"])
        .current_dir(target)
        .output()?;

    Ok(())
}

/// Ensure .gitignore exists and tracks only managed files.
fn ensure_gitignore(target: &Path) -> Result<(), crate::Error> {
    let gitignore_path = target.join(".gitignore");
    let desired = "*\n!flake.nix\n!flake.lock\n!fleet.yaml\n!.gitignore\n!CLAUDE.md\n!README.md\n";

    if gitignore_path.exists() {
        let existing = fs::read_to_string(&gitignore_path)?;
        if existing == desired {
            return Ok(());
        }
    }

    fs::write(&gitignore_path, desired)?;
    Ok(())
}

fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Simple diff: find app names that appear in one but not the other.
fn diff_apps(old: &str, new: &str) -> Vec<String> {
    let old_apps = extract_app_names(old);
    let new_apps = extract_app_names(new);

    let mut changed = Vec::new();
    for app in &new_apps {
        if !old_apps.contains(app) {
            changed.push(format!("+{app}"));
        }
    }
    for app in &old_apps {
        if !new_apps.contains(app) {
            changed.push(format!("-{app}"));
        }
    }
    changed
}

fn extract_app_names(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.contains("= mkApp ") || trimmed.contains("= mkFleetApp ") {
                trimmed.split_whitespace().next().map(String::from)
            } else {
                None
            }
        })
        .collect()
}

/// Resolve the target path from a string, expanding `~`.
pub fn resolve_target(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}
