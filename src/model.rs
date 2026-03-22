//! Core data model for flake composition.
//!
//! A `FlakeSpec` describes the desired state of a flake.nix file.
//! Multiple `FlakeFragment`s are merged into a single `FlakeSpec`
//! using priority-based composition.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// A single input to a Nix flake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeInput {
    pub url: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub follows: IndexMap<String, String>,
}

/// An app definition within a flake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDef {
    /// Shell script body for the app.
    pub script: String,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Source fragment that contributed this app (for provenance tracking).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// A fleet flow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowStep {
    pub id: String,
    pub action: serde_json::Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
}

/// A fleet flow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowDef {
    #[serde(default)]
    pub description: String,
    pub steps: Vec<FlowStep>,
}

/// A fragment — a partial flake specification from one source.
/// Multiple fragments are merged to produce a complete `FlakeSpec`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeFragment {
    /// Unique identifier for this fragment (e.g. "pleme-gems", "pleme-tests").
    pub id: String,
    /// Priority for merge conflicts (higher wins). Default: 50.
    /// Convention: 50 = base workspace, 100+ = specialized overrides.
    #[serde(default = "default_priority")]
    pub priority: u32,
    /// Flake inputs contributed by this fragment.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub inputs: IndexMap<String, FlakeInput>,
    /// Apps contributed by this fragment.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub apps: IndexMap<String, AppDef>,
    /// Fleet flows contributed by this fragment.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub flows: IndexMap<String, FlowDef>,
    /// Target systems.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub systems: Vec<String>,
}

fn default_priority() -> u32 {
    50
}

/// The complete flake specification — result of merging all fragments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlakeSpec {
    pub description: String,
    pub inputs: IndexMap<String, FlakeInput>,
    pub apps: IndexMap<String, AppDef>,
    pub flows: IndexMap<String, FlowDef>,
    pub systems: Vec<String>,
    /// Track which fragment contributed each app (for diagnostics).
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub provenance: IndexMap<String, String>,
}

impl FlakeSpec {
    /// Default systems used when no fragment declares any.
    const DEFAULT_SYSTEMS: &[&str] = &["aarch64-darwin", "x86_64-linux", "aarch64-linux"];

    /// Create an empty spec with defaults.
    /// Systems start empty — populated from fragments or DEFAULT_SYSTEMS during render.
    pub fn new(description: &str) -> Self {
        Self {
            description: description.to_string(),
            inputs: IndexMap::new(),
            apps: IndexMap::new(),
            flows: IndexMap::new(),
            systems: Vec::new(),
            provenance: IndexMap::new(),
        }
    }

    /// Returns the systems list, falling back to DEFAULT_SYSTEMS if none declared.
    pub fn effective_systems(&self) -> Vec<String> {
        if self.systems.is_empty() {
            Self::DEFAULT_SYSTEMS.iter().map(|s| (*s).to_string()).collect()
        } else {
            self.systems.clone()
        }
    }

    /// Merge a fragment into this spec.
    /// Higher priority fragments overwrite lower priority on conflicts.
    pub fn merge(&mut self, fragment: &FlakeFragment) {
        // Merge inputs (later fragments can override URLs)
        for (name, input) in &fragment.inputs {
            self.inputs.insert(name.clone(), input.clone());
        }

        // Merge apps with provenance tracking
        for (name, app) in &fragment.apps {
            if let Some(existing_source) = self.provenance.get(name) {
                // Check if existing app has higher priority
                // For now: last writer wins (fragments should be sorted by priority)
                let _ = existing_source;
            }
            let mut app = app.clone();
            app.source = Some(fragment.id.clone());
            self.apps.insert(name.clone(), app);
            self.provenance
                .insert(name.clone(), fragment.id.clone());
        }

        // Merge flows
        for (name, flow) in &fragment.flows {
            self.flows.insert(name.clone(), flow.clone());
        }

        // Take systems from first fragment that declares them
        if self.systems.is_empty() && !fragment.systems.is_empty() {
            self.systems.clone_from(&fragment.systems);
        }
    }

    /// Merge multiple fragments, sorted by priority (lowest first, highest wins).
    pub fn merge_all(&mut self, fragments: &mut [FlakeFragment]) {
        fragments.sort_by_key(|f| f.priority);
        for fragment in fragments.iter() {
            self.merge(fragment);
        }
    }

    /// Filter apps by name pattern.
    pub fn filter_apps(&mut self, pattern: &str) {
        self.apps
            .retain(|name, _| name.contains(pattern));
        self.provenance
            .retain(|name, _| self.apps.contains_key(name));
    }

    /// Remove all apps from a specific source fragment.
    pub fn remove_source(&mut self, source_id: &str) {
        self.apps.retain(|_, app| {
            app.source.as_deref() != Some(source_id)
        });
        self.provenance.retain(|_, source| source != source_id);
    }

    /// List all app names with their source fragment.
    pub fn app_summary(&self) -> Vec<(String, Option<String>)> {
        self.apps
            .iter()
            .map(|(name, app)| (name.clone(), app.source.clone()))
            .collect()
    }
}
