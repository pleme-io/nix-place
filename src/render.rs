//! Render a `FlakeSpec` into a valid `flake.nix` string.

use crate::model::{AppDef, FlakeInput, FlakeSpec};
use indexmap::IndexMap;

/// Render a complete flake.nix from a `FlakeSpec`.
pub fn render(spec: &FlakeSpec) -> String {
    let mut out = String::with_capacity(4096);

    out.push_str("{\n");
    out.push_str(&format!(
        "  description = \"{}\";\n\n",
        spec.description
    ));

    // Inputs
    render_inputs(&mut out, &spec.inputs);

    // Outputs
    out.push_str("  outputs = { self, nixpkgs, flake-utils");
    for name in spec.inputs.keys() {
        if name != "nixpkgs" && name != "flake-utils" {
            out.push_str(&format!(", {name}"));
        }
    }
    out.push_str(", ... }:\n");

    let systems_str: Vec<String> = spec
        .systems
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect();
    out.push_str(&format!(
        "    flake-utils.lib.eachSystem [{}] (system:\n",
        systems_str.join(" ")
    ));
    out.push_str("    let\n");
    out.push_str(
        "      pkgs = import nixpkgs { inherit system; };\n",
    );

    // Fleet binary if flows exist
    if !spec.flows.is_empty() {
        out.push_str("      fleetBin = \"${fleet.packages.${system}.default}/bin/fleet\";\n");
    }

    out.push_str("      ws = \"$PWD\";\n\n");

    // mkApp helper
    out.push_str("      mkApp = name: script: {\n");
    out.push_str("        type = \"app\";\n");
    out.push_str("        program = toString (pkgs.writeShellScript \"ws-${name}\" ''\n");
    out.push_str("          set -euo pipefail\n");
    out.push_str("          ${script}\n");
    out.push_str("        '');\n");
    out.push_str("      };\n\n");

    // Fleet flow YAML if flows exist
    if !spec.flows.is_empty() {
        let flows_json =
            serde_json::to_string(&serde_json::json!({"flows": &spec.flows}))
                .unwrap_or_default();
        out.push_str(&format!(
            "      fleetYaml = pkgs.writeText \"workspace-fleet.yaml\" ''{}'';\n\n",
            flows_json
        ));

        out.push_str("      mkFleetApp = flowName: mkApp \"flow-${flowName}\" ''\n");
        out.push_str("        cd ${ws}\n");
        out.push_str(
            "        [ ! -f fleet.yaml ] && cp ${fleetYaml} fleet.yaml\n",
        );
        out.push_str("        ${fleetBin} flow run ${flowName} \"$@\"\n");
        out.push_str("      '';\n\n");
    }

    // Apps
    out.push_str("    in {\n");
    out.push_str("      apps = {\n");

    for (name, app) in &spec.apps {
        render_app(&mut out, name, app);
    }

    // Flow apps
    if !spec.flows.is_empty() {
        out.push_str("\n        # Fleet flow apps\n");
        out.push_str("        flow-list = mkApp \"flow-list\" ''\n");
        out.push_str("          cd ${ws}\n");
        out.push_str(
            "          [ ! -f fleet.yaml ] && cp ${fleetYaml} fleet.yaml\n",
        );
        out.push_str("          ${fleetBin} flow list\n");
        out.push_str("        '';\n");

        for flow_name in spec.flows.keys() {
            out.push_str(&format!(
                "        flow-{flow_name} = mkFleetApp \"{flow_name}\";\n"
            ));
        }
    }

    out.push_str("      };\n");
    out.push_str("    });\n");
    out.push_str("}\n");

    out
}

fn render_inputs(out: &mut String, inputs: &IndexMap<String, FlakeInput>) {
    out.push_str("  inputs = {\n");
    for (name, input) in inputs {
        if input.follows.is_empty() {
            out.push_str(&format!(
                "    {name}.url = \"{}\";\n",
                input.url
            ));
        } else {
            out.push_str(&format!("    {name} = {{\n"));
            out.push_str(&format!(
                "      url = \"{}\";\n",
                input.url
            ));
            for (follow_name, follow_target) in &input.follows {
                out.push_str(&format!(
                    "      inputs.{follow_name}.follows = \"{follow_target}\";\n"
                ));
            }
            out.push_str("    };\n");
        }
    }
    out.push_str("  };\n\n");
}

fn render_app(out: &mut String, name: &str, app: &AppDef) {
    if let Some(desc) = &app.description {
        out.push_str(&format!("        # {desc}\n"));
    }
    out.push_str(&format!("        {name} = mkApp \"{name}\" ''\n"));
    for line in app.script.lines() {
        // In Nix multi-line strings, bash ${ must be escaped as ''${
        // But Nix interpolations (${pkgs.*}, ${ws}, ${fleetBin}, ${fleetYaml})
        // must NOT be escaped — they're intentional.
        let escaped = escape_nix_interpolation(line);
        out.push_str(&format!("          {escaped}\n"));
    }
    out.push_str("        '';\n\n");
}

/// Escape bash `${...}` for Nix multi-line strings, preserving Nix interpolations.
/// Nix interpolations like `${pkgs.ruby}`, `${ws}`, `${fleetBin}` are kept as-is.
/// Bash constructs like `${dir%/}`, `${1:-}`, `${var}` become `''${...}`.
fn escape_nix_interpolation(line: &str) -> String {
    // Known Nix variables that should NOT be escaped.
    // Maintenance: add entries here when new `let` bindings are added to the
    // rendered flake template (see `render()` above).
    const NIX_VARS: &[&str] = &[
        "${pkgs.", "${ws}", "${fleetBin}", "${fleetYaml}",
        "${system}", "${self.", "${lib.", "${env}",
    ];

    let mut result = String::with_capacity(line.len() + 16);
    let mut chars = line.char_indices().peekable();

    while let Some((byte_pos, ch)) = chars.next() {
        if ch == '$' {
            if let Some(&(_, '{')) = chars.peek() {
                chars.next(); // consume '{'
                let rest = &line[byte_pos..];
                if NIX_VARS.iter().any(|v| rest.starts_with(v)) {
                    result.push_str("${");
                } else {
                    result.push_str("''${");
                }
                continue;
            }
        }
        result.push(ch);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FlakeSpec;

    #[test]
    fn render_minimal_spec() {
        let spec = FlakeSpec::new("test flake");
        let output = render(&spec);
        assert!(output.contains("description = \"test flake\""));
        assert!(output.contains("inputs = {"));
        assert!(output.contains("apps = {"));
    }

    #[test]
    fn render_with_apps() {
        let mut spec = FlakeSpec::new("test");
        spec.apps.insert(
            "hello".to_string(),
            AppDef {
                script: "echo hello".to_string(),
                description: Some("Say hello".to_string()),
                source: None,
            },
        );
        let output = render(&spec);
        assert!(output.contains("hello = mkApp"));
        assert!(output.contains("echo hello"));
        assert!(output.contains("# Say hello"));
    }
}
