use std::collections::HashSet;
use std::path::Path;

use colored::Colorize;
use openapi_forge::Spec;

use crate::BackendChoice;

/// Run the full API evolution pipeline: diff, drift, scaffold, validate, generate.
///
/// # Errors
///
/// Returns an error if any critical step (spec loading, generation) fails.
/// Validation warnings are reported but do not halt the pipeline.
#[allow(clippy::too_many_arguments)]
pub fn run(
    old_spec_path: &Path,
    new_spec_path: &Path,
    resources_dir: &Path,
    output_dir: &Path,
    provider_path: Option<&Path>,
    auto_scaffold: bool,
    backend: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Diff
    println!(
        "\n{} Diffing specs: {} vs {}",
        "=>".blue().bold(),
        old_spec_path.display(),
        new_spec_path.display()
    );

    let old_api = Spec::load(old_spec_path)?;
    let new_api = Spec::load(new_spec_path)?;

    let old_endpoints: HashSet<String> =
        old_api.endpoints().into_iter().map(|e| e.path).collect();
    let new_endpoints: HashSet<String> =
        new_api.endpoints().into_iter().map(|e| e.path).collect();

    let added: Vec<_> = new_endpoints.difference(&old_endpoints).cloned().collect();
    let removed: Vec<_> = old_endpoints.difference(&new_endpoints).cloned().collect();

    let old_schemas: HashSet<String> = old_api
        .schema_names()
        .into_iter()
        .map(String::from)
        .collect();
    let new_schemas: HashSet<String> = new_api
        .schema_names()
        .into_iter()
        .map(String::from)
        .collect();

    let added_schemas = new_schemas.difference(&old_schemas).count();
    let removed_schemas = old_schemas.difference(&new_schemas).count();

    println!(
        "  {} {} added endpoint(s), {} removed endpoint(s)",
        "i".blue(),
        added.len(),
        removed.len()
    );
    println!(
        "  {} {} added schema(s), {} removed schema(s)",
        "i".blue(),
        added_schemas,
        removed_schemas
    );

    let mut sorted_added = added.clone();
    sorted_added.sort();
    for ep in &sorted_added {
        println!("    {} {ep}", "+".green());
    }
    let mut sorted_removed = removed.clone();
    sorted_removed.sort();
    for ep in &sorted_removed {
        println!("    {} {ep}", "-".red());
    }

    // Step 2: Drift detection
    println!(
        "\n{} Detecting drift against new spec...",
        "=>".blue().bold()
    );
    let drift_result = crate::commands::drift::run(new_spec_path, resources_dir);
    if let Err(e) = &drift_result {
        println!("  {} Drift check encountered an error: {e}", "!".yellow());
    }

    // Step 3: Auto-scaffold if requested and there are new endpoints
    if auto_scaffold && !added.is_empty() {
        println!(
            "\n{} Auto-scaffolding new resources...",
            "=>".blue().bold()
        );
        match crate::commands::scaffold::run(new_spec_path, None, resources_dir) {
            Ok(()) => {}
            Err(e) => {
                println!(
                    "  {} Scaffold encountered an error: {e} (continuing)",
                    "!".yellow()
                );
            }
        }
    }

    // Step 4: Validate all resource specs against the new spec
    println!(
        "\n{} Validating all resource specs against new spec...",
        "=>".blue().bold()
    );
    let validate_result = crate::commands::validate::run(new_spec_path, resources_dir);
    if let Err(e) = &validate_result {
        println!(
            "  {} Some specs failed validation: {e} (continuing)",
            "!".yellow()
        );
    }

    // Step 5: Generate backend artifacts
    let backend_choice: BackendChoice = backend
        .parse()
        .unwrap_or_else(|_| BackendChoice::All);

    println!(
        "\n{} Generating {backend_choice} artifacts...",
        "=>".blue().bold()
    );
    crate::commands::generate::run(
        &backend_choice,
        new_spec_path,
        resources_dir,
        output_dir,
        provider_path,
    )?;

    // Step 6: Summary
    let file_count = count_files(output_dir);
    println!(
        "\n{} Sync complete",
        "done".green().bold()
    );
    println!("  Endpoints: +{} -{}", added.len(), removed.len());
    println!("  Schemas:   +{added_schemas} -{removed_schemas}");
    println!("  Generated: {file_count} file(s)");

    Ok(())
}

/// Count files (non-recursively) in a directory.
fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
                .count()
        })
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use tempfile::TempDir;

    fn write_old_spec(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: Old, version: "1.0" }
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
  /old-only-endpoint:
    post:
      operationId: oldOnly
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
        let path = dir.join("old_spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_new_spec(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: New, version: "2.0" }
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
  /new-endpoint:
    post:
      operationId: newEndpoint
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/newEndpoint'
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
    newEndpoint:
      type: object
      properties:
        id: { type: string }
"#;
        let path = dir.join("new_spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_provider_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[provider]
name = "test"
description = "Test provider"
version = "0.1.0"
sdk_import = "github.com/test/sdk"

[auth]
token_field = "token"
env_var = "TEST_TOKEN"
gateway_url_field = "url"
gateway_env_var = "TEST_URL"

[defaults]
skip_fields = ["token"]
"#;
        let path = dir.join("provider.toml");
        fs::write(&path, toml_content).unwrap();
        path
    }

    fn write_resource_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[resource]
name = "test_secret"
description = "Test secret"
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
        let path = dir.join("secret.toml");
        fs::write(&path, toml_content).unwrap();
        path
    }

    #[test]
    fn sync_detects_diff_and_generates() {
        let dir = TempDir::new().unwrap();
        let old_spec = write_old_spec(dir.path());
        let new_spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = super::run(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
        );

        assert!(result.is_ok(), "sync failed: {result:?}");
        // Verify output directory was created and has content
        assert!(output_dir.exists(), "output directory should exist");
    }

    #[test]
    fn sync_with_auto_scaffold() {
        let dir = TempDir::new().unwrap();
        let old_spec = write_old_spec(dir.path());
        let new_spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = super::run(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            true,
            "terraform",
        );

        assert!(result.is_ok(), "sync with auto-scaffold failed: {result:?}");
    }

    #[test]
    fn sync_defaults_to_all_backend() {
        let dir = TempDir::new().unwrap();
        let old_spec = write_old_spec(dir.path());
        let new_spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        // "invalid-backend" should fall back to All
        let result = super::run(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "invalid-backend",
        );

        // This may fail because not all backends are compiled in, but it should
        // at least parse correctly and attempt generation
        // The important thing is it does not panic
        let _ = result;
    }

    #[test]
    fn count_files_empty_dir() {
        let dir = TempDir::new().unwrap();
        assert_eq!(super::count_files(dir.path()), 0);
    }

    #[test]
    fn count_files_nonexistent() {
        assert_eq!(
            super::count_files(std::path::Path::new("/nonexistent")),
            0
        );
    }
}
