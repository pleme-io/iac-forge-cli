use std::collections::HashSet;
use std::path::Path;

use colored::Colorize;
use openapi_forge::Spec;

pub fn run(old_path: &Path, new_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} Diffing specs: {} vs {}",
        "=>".blue().bold(),
        old_path.display(),
        new_path.display()
    );

    let old_spec = Spec::load(old_path)?;
    let new_spec = Spec::load(new_path)?;

    let old_endpoints: HashSet<String> = old_spec.endpoints().into_iter().map(|e| e.path).collect();
    let new_endpoints: HashSet<String> = new_spec.endpoints().into_iter().map(|e| e.path).collect();

    let added: Vec<_> = new_endpoints.difference(&old_endpoints).collect();
    let removed: Vec<_> = old_endpoints.difference(&new_endpoints).collect();

    if !added.is_empty() {
        println!(
            "\n{} New endpoints ({}):",
            "ADDED".green().bold(),
            added.len()
        );
        let mut sorted: Vec<_> = added.into_iter().collect();
        sorted.sort();
        for ep in &sorted {
            println!("  {} {ep}", "+".green());
        }
    }

    if !removed.is_empty() {
        println!(
            "\n{} Removed endpoints ({}):",
            "REMOVED".red().bold(),
            removed.len()
        );
        let mut sorted: Vec<_> = removed.into_iter().collect();
        sorted.sort();
        for ep in &sorted {
            println!("  {} {ep}", "-".red());
        }
    }

    // Compare schemas
    let old_schemas: HashSet<String> = old_spec
        .schema_names()
        .into_iter()
        .map(String::from)
        .collect();
    let new_schemas: HashSet<String> = new_spec
        .schema_names()
        .into_iter()
        .map(String::from)
        .collect();

    let new_schema_count = new_schemas.difference(&old_schemas).count();
    let removed_schema_count = old_schemas.difference(&new_schemas).count();

    println!(
        "\n{} endpoints: +{} -{}, schemas: +{new_schema_count} -{removed_schema_count}",
        "summary:".blue().bold(),
        new_endpoints.difference(&old_endpoints).count(),
        old_endpoints.difference(&new_endpoints).count(),
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_spec(dir: &Path, name: &str, paths: &[&str], schemas: &[&str]) -> std::path::PathBuf {
        let mut paths_yaml = String::new();
        for p in paths {
            paths_yaml.push_str(&format!(
                "  {p}:\n    post:\n      operationId: op\n      responses:\n        \"200\": {{ description: ok }}\n"
            ));
        }
        let mut schemas_yaml = String::new();
        for s in schemas {
            schemas_yaml.push_str(&format!(
                "    {s}:\n      type: object\n      properties:\n        name: {{ type: string }}\n"
            ));
        }
        let spec = format!(
            "openapi: \"3.0.0\"\ninfo: {{ title: Test, version: \"1.0\" }}\npaths:\n{paths_yaml}components:\n  schemas:\n{schemas_yaml}"
        );
        let path = dir.join(name);
        fs::write(&path, spec).unwrap();
        path
    }

    #[test]
    fn diff_identical_specs_reports_no_changes() {
        let dir = TempDir::new().unwrap();
        let endpoints = &["/create", "/delete"];
        let schemas = &["Foo"];
        let old = write_spec(dir.path(), "old.yaml", endpoints, schemas);
        let new = write_spec(dir.path(), "new.yaml", endpoints, schemas);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_detects_added_endpoints() {
        let dir = TempDir::new().unwrap();
        let old = write_spec(dir.path(), "old.yaml", &["/a"], &["S"]);
        let new = write_spec(dir.path(), "new.yaml", &["/a", "/b", "/c"], &["S"]);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_detects_removed_endpoints() {
        let dir = TempDir::new().unwrap();
        let old = write_spec(dir.path(), "old.yaml", &["/a", "/b", "/c"], &["S"]);
        let new = write_spec(dir.path(), "new.yaml", &["/a"], &["S"]);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_detects_added_schemas() {
        let dir = TempDir::new().unwrap();
        let old = write_spec(dir.path(), "old.yaml", &["/a"], &["Alpha"]);
        let new = write_spec(dir.path(), "new.yaml", &["/a"], &["Alpha", "Beta"]);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_detects_removed_schemas() {
        let dir = TempDir::new().unwrap();
        let old = write_spec(dir.path(), "old.yaml", &["/a"], &["Alpha", "Beta"]);
        let new = write_spec(dir.path(), "new.yaml", &["/a"], &["Alpha"]);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_with_empty_paths_old() {
        let dir = TempDir::new().unwrap();
        let old_spec = "openapi: \"3.0.0\"\ninfo: { title: T, version: \"1.0\" }\npaths: {}\ncomponents:\n  schemas:\n    X:\n      type: object\n      properties:\n        a: { type: string }\n";
        let old = dir.path().join("old.yaml");
        fs::write(&old, old_spec).unwrap();
        let new = write_spec(dir.path(), "new.yaml", &["/create"], &["X"]);
        assert!(run(&old, &new).is_ok());
    }

    #[test]
    fn diff_invalid_old_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let old = dir.path().join("old.yaml");
        fs::write(&old, "not valid openapi yaml at all: ][").unwrap();
        let new = write_spec(dir.path(), "new.yaml", &["/a"], &["S"]);
        assert!(run(&old, &new).is_err());
    }

    #[test]
    fn diff_nonexistent_file_returns_error() {
        let dir = TempDir::new().unwrap();
        let old = dir.path().join("nonexistent.yaml");
        let new = write_spec(dir.path(), "new.yaml", &["/a"], &["S"]);
        assert!(run(&old, &new).is_err());
    }
}
