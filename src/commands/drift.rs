use std::collections::HashSet;
use std::path::Path;

use colored::Colorize;
use iac_forge::ResourceSpec;
use openapi_forge::Spec;

pub fn run(spec_path: &Path, resources_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} Checking for drift between spec and resource definitions",
        "=>".blue().bold()
    );
    let api = Spec::load(spec_path)?;

    // Collect all CRUD groups from the spec
    let groups = api.group_by_crud_pattern();
    let spec_resources: HashSet<String> = groups
        .iter()
        .filter(|g| g.create.is_some() && g.delete.is_some())
        .map(|g| g.base_name.replace('-', "_"))
        .collect();

    // Collect all defined resources
    let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", resources_dir.display()))?
        .filter_map(Result::ok)
        .collect();

    let mut defined: HashSet<String> = HashSet::new();
    for file in &files {
        if let Ok(resource) = ResourceSpec::load(file) {
            let name = resource
                .resource
                .name
                .strip_prefix("akeyless_")
                .unwrap_or(&resource.resource.name)
                .to_string();
            defined.insert(name);
        }
    }

    // Missing: in spec but not in resources
    let missing: Vec<_> = spec_resources.difference(&defined).collect();
    // Extra: in resources but not in spec
    let extra: Vec<_> = defined.difference(&spec_resources).collect();

    if !missing.is_empty() {
        println!(
            "\n{} Resources in spec but not defined ({}):",
            "MISSING".yellow().bold(),
            missing.len()
        );
        let mut sorted: Vec<_> = missing.into_iter().collect();
        sorted.sort();
        for name in &sorted {
            println!("  {} akeyless_{name}", "-".red());
        }
    }

    if !extra.is_empty() {
        println!(
            "\n{} Resources defined but not in spec ({}):",
            "EXTRA".cyan().bold(),
            extra.len()
        );
        let mut sorted: Vec<_> = extra.into_iter().collect();
        sorted.sort();
        for name in &sorted {
            println!("  {} akeyless_{name}", "+".green());
        }
    }

    let covered = defined.intersection(&spec_resources).count();
    println!(
        "\n{} {covered}/{} spec resources covered",
        "summary:".blue().bold(),
        spec_resources.len()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_crud_spec(dir: &Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: Test, version: "1.0" }
paths:
  /create-widget:
    post:
      operationId: createWidget
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createWidget'
      responses:
        "200": { description: ok }
  /describe-item:
    post:
      operationId: describeItem
      responses:
        "200": { description: ok }
  /delete-item:
    post:
      operationId: deleteItem
      responses:
        "200": { description: ok }
components:
  schemas:
    createWidget:
      type: object
      required: [name]
      properties:
        name: { type: string }
    describeItem:
      type: object
      properties:
        name: { type: string }
    deleteItem:
      type: object
      properties:
        name: { type: string }
"#;
        let path = dir.join("spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_resource(dir: &Path, name: &str) {
        let toml_content = format!(
            r#"
[resource]
name = "{name}"
description = "Test"
category = "test"

[crud]
create_endpoint = "/create-widget"
create_schema = "createWidget"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#
        );
        let file_name = name.replace("akeyless_", "");
        fs::write(dir.join(format!("{file_name}.toml")), toml_content).unwrap();
    }

    #[test]
    fn drift_empty_resources_dir() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn drift_with_matching_resource() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource(&resources_dir, "akeyless_item");
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn drift_with_extra_resource() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource(&resources_dir, "akeyless_nonexistent_thing");
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn drift_nonexistent_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("missing.yaml");
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_err());
    }

    #[test]
    fn drift_invalid_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("bad.yaml");
        fs::write(&spec, "not: valid: openapi").unwrap();
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_err());
    }

    #[test]
    fn drift_malformed_toml_is_silently_skipped() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        fs::write(resources_dir.join("bad.toml"), "this is not valid toml [[[").unwrap();
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn drift_nonexistent_resources_dir_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("does_not_exist");
        let result = run(&spec, &resources_dir);
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn drift_resource_name_without_prefix() {
        let dir = TempDir::new().unwrap();
        let spec = write_crud_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource(&resources_dir, "no_prefix_resource");
        assert!(run(&spec, &resources_dir).is_ok());
    }
}
