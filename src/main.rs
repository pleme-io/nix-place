//! nix-place — Managed Nix flake placement.
//!
//! Merges flake fragments from multiple sources into a single coherent
//! flake.nix, writes it atomically, and manages the git context.
//!
//! Usage:
//!   nix-place sync --target ~/code/github/pleme-io/ --fragments fragments/*.yaml
//!   nix-place diff --target ~/code/github/pleme-io/ --fragments fragments/*.yaml
//!   nix-place clean --target ~/code/github/pleme-io/
//!   nix-place render --fragments fragments/*.yaml  # stdout
//!   nix-place show --fragments fragments/*.yaml    # show merged spec

use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod model;
mod render;
mod sync;
mod validate;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Persist error: {0}")]
    Persist(#[from] tempfile::PersistError),
    #[error("{0}")]
    Other(String),
}

#[derive(Parser)]
#[command(name = "nix-place", about = "Managed Nix flake placement")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Merge fragments and write flake.nix to target directory
    Sync {
        /// Target directory for flake.nix
        #[arg(short, long)]
        target: String,
        /// Fragment files (YAML/JSON) to merge
        #[arg(short, long, num_args = 1..)]
        fragments: Vec<PathBuf>,
        /// Description for the generated flake
        #[arg(short, long, default_value = "Managed workspace")]
        description: String,
    },
    /// Show what would change without writing
    Diff {
        #[arg(short, long)]
        target: String,
        #[arg(short, long, num_args = 1..)]
        fragments: Vec<PathBuf>,
        #[arg(short, long, default_value = "Managed workspace")]
        description: String,
    },
    /// Remove managed flake.nix from target
    Clean {
        #[arg(short, long)]
        target: String,
    },
    /// Render merged flake.nix to stdout (no file write)
    Render {
        #[arg(short, long, num_args = 1..)]
        fragments: Vec<PathBuf>,
        #[arg(short, long, default_value = "Managed workspace")]
        description: String,
    },
    /// Show the merged spec as YAML (for debugging)
    Show {
        #[arg(short, long, num_args = 1..)]
        fragments: Vec<PathBuf>,
        #[arg(short, long, default_value = "Managed workspace")]
        description: String,
    },
}

fn load_fragments(paths: &[PathBuf]) -> Result<Vec<model::FlakeFragment>, Error> {
    let mut fragments = Vec::new();
    for path in paths {
        let content = std::fs::read_to_string(path)?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("yaml");

        let fragment: model::FlakeFragment = match ext {
            "json" => serde_json::from_str(&content)?,
            _ => serde_yaml_ng::from_str(&content)?,
        };
        fragments.push(fragment);
    }
    Ok(fragments)
}

fn build_spec(
    description: &str,
    fragments: &[PathBuf],
) -> Result<model::FlakeSpec, Error> {
    let mut frags = load_fragments(fragments)?;
    let mut spec = model::FlakeSpec::new(description);
    spec.merge_all(&mut frags);
    Ok(spec)
}

fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Sync {
            target,
            fragments,
            description,
        } => {
            let spec = build_spec(&description, &fragments)?;
            let target_path = sync::resolve_target(&target);
            match sync::sync(&spec, &target_path)? {
                sync::SyncAction::Created => {
                    eprintln!(
                        "[nix-place] Created {}/flake.nix ({} apps, {} flows)",
                        target_path.display(),
                        spec.apps.len(),
                        spec.flows.len()
                    );
                }
                sync::SyncAction::Updated { changed_apps } => {
                    eprintln!(
                        "[nix-place] Updated {}/flake.nix ({})",
                        target_path.display(),
                        if changed_apps.is_empty() {
                            "content changed".to_string()
                        } else {
                            changed_apps.join(", ")
                        }
                    );
                }
                sync::SyncAction::Unchanged => {
                    eprintln!("[nix-place] No changes.");
                }
            }
        }
        Commands::Diff {
            target,
            fragments,
            description,
        } => {
            let spec = build_spec(&description, &fragments)?;
            let target_path = sync::resolve_target(&target);
            let result = sync::diff(&spec, &target_path)?;
            println!("{result}");
        }
        Commands::Clean { target } => {
            let target_path = sync::resolve_target(&target);
            if sync::clean(&target_path)? {
                eprintln!(
                    "[nix-place] Cleaned {}/flake.nix",
                    target_path.display()
                );
            } else {
                eprintln!("[nix-place] Nothing to clean.");
            }
        }
        Commands::Render {
            fragments,
            description,
        } => {
            let spec = build_spec(&description, &fragments)?;
            let nix_code = render::render(&spec);

            // Validate before outputting
            let validation = validate::validate_nix(&nix_code);
            if !validation.valid {
                eprintln!("[nix-place] WARNING: Generated Nix has syntax errors:");
                eprintln!("{validation}");
            }

            print!("{nix_code}");
        }
        Commands::Show {
            fragments,
            description,
        } => {
            let spec = build_spec(&description, &fragments)?;
            println!("{}", serde_yaml_ng::to_string(&spec)?);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_spec() {
        let spec = model::FlakeSpec::new("test");
        let rendered = render::render(&spec);
        assert!(rendered.contains("description = \"test\""));
    }

    #[test]
    fn test_fragment_merge() {
        let mut spec = model::FlakeSpec::new("test");
        let frag1 = model::FlakeFragment {
            id: "source1".to_string(),
            priority: 100,
            inputs: indexmap::IndexMap::new(),
            apps: indexmap::indexmap! {
                "app1".to_string() => model::AppDef {
                    script: "echo app1".to_string(),
                    description: None,
                    source: None,
                },
            },
            flows: indexmap::IndexMap::new(),
            systems: vec![],
        };
        let frag2 = model::FlakeFragment {
            id: "source2".to_string(),
            priority: 200,
            inputs: indexmap::IndexMap::new(),
            apps: indexmap::indexmap! {
                "app2".to_string() => model::AppDef {
                    script: "echo app2".to_string(),
                    description: None,
                    source: None,
                },
            },
            flows: indexmap::IndexMap::new(),
            systems: vec![],
        };
        spec.merge(&frag1);
        spec.merge(&frag2);

        assert_eq!(spec.apps.len(), 2);
        assert_eq!(
            spec.provenance.get("app1").unwrap(),
            "source1"
        );
        assert_eq!(
            spec.provenance.get("app2").unwrap(),
            "source2"
        );
    }

    #[test]
    fn test_remove_source() {
        let mut spec = model::FlakeSpec::new("test");
        let frag = model::FlakeFragment {
            id: "removable".to_string(),
            priority: 100,
            inputs: indexmap::IndexMap::new(),
            apps: indexmap::indexmap! {
                "temp-app".to_string() => model::AppDef {
                    script: "echo temp".to_string(),
                    description: None,
                    source: None,
                },
            },
            flows: indexmap::IndexMap::new(),
            systems: vec![],
        };
        spec.merge(&frag);
        assert_eq!(spec.apps.len(), 1);

        spec.remove_source("removable");
        assert_eq!(spec.apps.len(), 0);
    }

    #[test]
    fn test_priority_merge() {
        let mut spec = model::FlakeSpec::new("test");
        let mut frags = vec![
            model::FlakeFragment {
                id: "low".to_string(),
                priority: 50,
                inputs: indexmap::IndexMap::new(),
                apps: indexmap::indexmap! {
                    "shared".to_string() => model::AppDef {
                        script: "echo low".to_string(),
                        description: None,
                        source: None,
                    },
                },
                flows: indexmap::IndexMap::new(),
                systems: vec![],
            },
            model::FlakeFragment {
                id: "high".to_string(),
                priority: 200,
                inputs: indexmap::IndexMap::new(),
                apps: indexmap::indexmap! {
                    "shared".to_string() => model::AppDef {
                        script: "echo high".to_string(),
                        description: None,
                        source: None,
                    },
                },
                flows: indexmap::IndexMap::new(),
                systems: vec![],
            },
        ];
        spec.merge_all(&mut frags);

        // Higher priority wins
        assert_eq!(spec.apps["shared"].script, "echo high");
        assert_eq!(
            spec.provenance.get("shared").unwrap(),
            "high"
        );
    }
}
