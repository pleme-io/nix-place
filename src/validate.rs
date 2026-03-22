//! Validate generated Nix code using the rnix parser.
//!
//! Catches syntax errors at generation time rather than at `nix eval` time.

use rnix::SyntaxKind;

/// Validation result with error details.
#[derive(Debug)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}

#[derive(Debug)]
pub struct ValidationError {
    pub message: String,
    pub line: usize,
    pub column: usize,
}

/// Parse Nix source and return any syntax errors.
pub fn validate_nix(source: &str) -> ValidationResult {
    let parse = rnix::Root::parse(source);
    let errors: Vec<ValidationError> = parse
        .errors()
        .iter()
        .map(|err| {
            // Extract offset from the ParseError variant
            let offset = parse_error_offset(err);
            let (line, col) = offset_to_line_col(source, offset);
            ValidationError {
                message: format!("{err}"),
                line,
                column: col,
            }
        })
        .collect();

    // Also check for ERROR nodes in the syntax tree
    let tree = parse.syntax();
    let mut tree_errors = Vec::new();
    collect_error_nodes(&tree, source, &mut tree_errors);

    let mut all_errors = errors;
    all_errors.extend(tree_errors);

    ValidationResult {
        valid: all_errors.is_empty(),
        errors: all_errors,
    }
}

/// Walk the syntax tree and collect ERROR nodes.
fn collect_error_nodes(
    node: &rnix::SyntaxNode,
    source: &str,
    errors: &mut Vec<ValidationError>,
) {
    if node.kind() == SyntaxKind::NODE_ERROR {
        let (line, col) = offset_to_line_col(source, node.text_range().start().into());
        errors.push(ValidationError {
            message: format!("Syntax error near: {}", &node.to_string()[..node.to_string().len().min(40)]),
            line,
            column: col,
        });
    }
    for child in node.children() {
        collect_error_nodes(&child, source, errors);
    }
}

/// Extract the byte offset from a ParseError variant.
fn parse_error_offset(err: &rnix::parser::ParseError) -> usize {
    use rnix::parser::ParseError;
    match err {
        ParseError::Unexpected(r)
        | ParseError::UnexpectedExtra(r)
        | ParseError::UnexpectedDoubleBind(r) => r.start().into(),
        ParseError::UnexpectedWanted(_, r, _) => r.start().into(),
        ParseError::UnexpectedEOF
        | ParseError::UnexpectedEOFWanted(_)
        | ParseError::RecursionLimitExceeded => 0,
        _ => 0,
    }
}

/// Convert a byte offset to (line, column) — both 1-indexed.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

impl std::fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.valid {
            write!(f, "Valid Nix expression")
        } else {
            writeln!(f, "{} error(s):", self.errors.len())?;
            for err in &self.errors {
                writeln!(f, "  line {}:{}: {}", err.line, err.column, err.message)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_nix() {
        let result = validate_nix("{ a = 1; b = \"hello\"; }");
        assert!(result.valid, "Expected valid, got: {result}");
    }

    #[test]
    fn invalid_nix() {
        let result = validate_nix("{ a = ; }");
        assert!(!result.valid);
        assert!(!result.errors.is_empty());
    }

    #[test]
    fn valid_flake() {
        let nix = r#"{
          description = "test";
          inputs.nixpkgs.url = "github:NixOS/nixpkgs";
          outputs = { self, nixpkgs }: { };
        }"#;
        let result = validate_nix(nix);
        assert!(result.valid, "Expected valid flake, got: {result}");
    }
}
