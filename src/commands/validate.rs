use std::path::Path;

use colored::Colorize;
use iac_forge::ResourceSpec;
use openapi_forge::Spec;

pub fn run(spec_path: &Path, resources_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} Validating resource specs against {}",
        "=>".blue().bold(),
        spec_path.display()
    );
    let api = Spec::load(spec_path)?;

    let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", resources_dir.display()))?
        .filter_map(Result::ok)
        .collect();

    let mut errors = 0;
    let mut ok_count = 0;

    for file in &files {
        let resource = match ResourceSpec::load(file) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "  {} {}: parse error: {e}",
                    "FAIL".red().bold(),
                    file.display()
                );
                errors += 1;
                continue;
            }
        };

        match resource.validate(&api) {
            Ok(()) => {
                println!("  {} {}", "OK".green(), resource.resource.name);
                ok_count += 1;
            }
            Err(e) => {
                eprintln!("  {} {}: {e}", "FAIL".red().bold(), resource.resource.name);
                errors += 1;
            }
        }
    }

    println!(
        "\n{} {ok_count} passed, {errors} failed",
        if errors == 0 {
            "result:".green().bold()
        } else {
            "result:".red().bold()
        }
    );

    if errors > 0 {
        Err(format!("{errors} validation error(s)").into())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_spec(dir: &Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: Test, version: "1.0" }
paths:
  /create-secret:
    post:
      operationId: createSecret
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createSecret'
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
    createSecret:
      type: object
      required: [name, value]
      properties:
        name: { type: string }
        value: { type: string }
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

    fn write_valid_resource(dir: &Path) {
        let toml = r#"
[resource]
name = "test_secret"
description = "Test"
category = "secret"

[crud]
create_endpoint = "/create-secret"
create_schema = "createSecret"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
force_new_fields = ["name"]

[fields]
token = { skip = true }
"#;
        fs::write(dir.join("secret.toml"), toml).unwrap();
    }

    #[test]
    fn validate_empty_resources_dir_succeeds() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn validate_valid_resource_succeeds() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_valid_resource(&resources_dir);
        assert!(run(&spec, &resources_dir).is_ok());
    }

    #[test]
    fn validate_invalid_schema_ref_fails() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        let bad = r#"
[resource]
name = "bad"
description = "Bad"
category = "test"

[crud]
create_endpoint = "/create-secret"
create_schema = "DoesNotExist"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#;
        fs::write(resources_dir.join("bad.toml"), bad).unwrap();
        let result = run(&spec, &resources_dir);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("1 validation error"), "error message: {err}");
    }

    #[test]
    fn validate_malformed_toml_counts_as_error() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        fs::write(resources_dir.join("broken.toml"), "this is not [valid toml [[[").unwrap();
        let result = run(&spec, &resources_dir);
        assert!(result.is_err());
    }

    #[test]
    fn validate_mixed_valid_and_invalid() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_valid_resource(&resources_dir);
        let bad = r#"
[resource]
name = "bad2"
description = "Bad"
category = "test"

[crud]
create_endpoint = "/create-secret"
create_schema = "Nonexistent"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#;
        fs::write(resources_dir.join("bad2.toml"), bad).unwrap();
        let result = run(&spec, &resources_dir);
        assert!(result.is_err());
    }

    #[test]
    fn validate_nonexistent_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("missing.yaml");
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_err());
    }

    #[test]
    fn validate_invalid_spec_yaml_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("bad.yaml");
        fs::write(&spec, "not: valid: openapi ][").unwrap();
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        assert!(run(&spec, &resources_dir).is_err());
    }

    #[test]
    fn validate_multiple_valid_resources() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_valid_resource(&resources_dir);
        let toml2 = r#"
[resource]
name = "test_secret2"
description = "Another valid"
category = "secret"

[crud]
create_endpoint = "/create-secret"
create_schema = "createSecret"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#;
        fs::write(resources_dir.join("secret2.toml"), toml2).unwrap();
        assert!(run(&spec, &resources_dir).is_ok());
    }
}
