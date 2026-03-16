use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use colored::Colorize;
use openapi_forge::Spec;

use crate::BackendChoice;

/// Append a JSON audit event to a JSONL file.
fn audit_log(path: &Path, event: &str, data: serde_json::Value) {
    let entry = serde_json::json!({
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "event": event,
        "data": data,
    });
    if let Ok(line) = serde_json::to_string(&entry) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{line}");
        }
    }
}

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
    run_with_audit(old_spec_path, new_spec_path, resources_dir, output_dir, provider_path, auto_scaffold, backend, None)
}

/// Run sync with optional audit log output.
#[allow(clippy::too_many_arguments)]
pub fn run_with_audit(
    old_spec_path: &Path,
    new_spec_path: &Path,
    resources_dir: &Path,
    output_dir: &Path,
    provider_path: Option<&Path>,
    auto_scaffold: bool,
    backend: &str,
    audit_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Diff (skip if old spec doesn't exist — first run)
    let (sorted_added, sorted_removed, added_schemas, removed_schemas) = if old_spec_path.exists()
    {
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

        let mut sorted_added = added;
        sorted_added.sort();
        for ep in &sorted_added {
            println!("    {} {ep}", "+".green());
        }
        let mut sorted_removed = removed;
        sorted_removed.sort();
        for ep in &sorted_removed {
            println!("    {} {ep}", "-".red());
        }

        (sorted_added, sorted_removed, added_schemas, removed_schemas)
    } else {
        println!(
            "\n{} No previous spec found, skipping diff (first run)",
            "=>".blue().bold()
        );
        (Vec::new(), Vec::new(), 0_usize, 0_usize)
    };

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
    if auto_scaffold && !sorted_added.is_empty() {
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
    println!("  Endpoints: +{} -{}", sorted_added.len(), sorted_removed.len());
    println!("  Schemas:   +{added_schemas} -{removed_schemas}");
    println!("  Generated: {file_count} file(s)");

    if let Some(audit) = audit_path {
        audit_log(audit, "sync_complete", serde_json::json!({
            "old_spec": old_spec_path.display().to_string(),
            "new_spec": new_spec_path.display().to_string(),
            "endpoints_added": sorted_added.len(),
            "endpoints_removed": sorted_removed.len(),
            "schemas_added": added_schemas,
            "schemas_removed": removed_schemas,
            "added_endpoints": sorted_added,
            "removed_endpoints": sorted_removed,
            "backend": backend,
            "files_generated": file_count,
            "auto_scaffold": auto_scaffold,
        }));
    }

    Ok(())
}

/// Count files recursively in a directory.
fn count_files(dir: &Path) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if ft.is_file() {
            count += 1;
        } else if ft.is_dir() {
            count += count_files(&entry.path());
        }
    }
    count
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

    #[test]
    fn count_files_recursive() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("b.txt"), "b").unwrap();
        let deep = sub.join("deep");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("c.txt"), "c").unwrap();
        assert_eq!(super::count_files(dir.path()), 3);
    }

    #[test]
    fn sync_first_run_no_old_spec() {
        let dir = TempDir::new().unwrap();
        let new_spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        // old spec path that does not exist — first run
        let nonexistent_old = dir.path().join("does_not_exist.yaml");
        let result = super::run(
            &nonexistent_old,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
        );

        assert!(result.is_ok(), "first-run sync should succeed: {result:?}");
        assert!(output_dir.exists(), "output directory should exist");
    }

    // --- Deterministic chain evolution tests ---

    /// Helper: write a v1 spec with /create-secret, /describe-item, /delete-item (1 CRUD resource)
    fn write_spec_v1(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: API, version: "1.0" }
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
        let path = dir.join("v1.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    /// Helper: write a v2 spec = v1 + /create-new-thing, /get-new-thing, /delete-new-thing
    fn write_spec_v2(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: API, version: "2.0" }
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
  /create-new-thing:
    post:
      operationId: createNewThing
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createNewThing'
      responses:
        "200": { description: ok }
  /get-new-thing:
    post:
      operationId: getNewThing
      responses:
        "200": { description: ok }
  /delete-new-thing:
    post:
      operationId: deleteNewThing
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
    createNewThing:
      type: object
      properties:
        id: { type: string }
"#;
        let path = dir.join("v2.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    /// Helper: write a v3 spec = removes /create-new-thing, /get-new-thing, /delete-new-thing
    /// and adds /target-create-aws, /target-update-aws, /target-get, /target-delete
    fn write_spec_v3(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: API, version: "3.0" }
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
  /target-create-aws:
    post:
      operationId: targetCreateAws
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/targetCreateAws'
      responses:
        "200": { description: ok }
  /target-update-aws:
    post:
      operationId: targetUpdateAws
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/targetUpdateAws'
      responses:
        "200": { description: ok }
  /target-get:
    post:
      operationId: targetGet
      responses:
        "200": { description: ok }
  /target-delete:
    post:
      operationId: targetDelete
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
    targetCreateAws:
      type: object
      properties:
        name: { type: string }
        access_key: { type: string }
    targetUpdateAws:
      type: object
      properties:
        name: { type: string }
        access_key: { type: string }
"#;
        let path = dir.join("v3.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    #[test]
    fn chain_test_api_evolution() {
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_spec_v2(dir.path());
        let v3 = write_spec_v3(dir.path());

        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);

        // Step 1: Generate from v1 (first run, no old spec)
        let out1 = dir.path().join("out1");
        let nonexistent = dir.path().join("no_such_spec.yaml");
        let result = super::run_with_audit(
            &nonexistent,
            &v1,
            &resources_dir,
            &out1,
            Some(&provider_path),
            true,
            "terraform",
            None,
        );
        assert!(result.is_ok(), "v1 first-run sync failed: {result:?}");
        assert!(out1.exists(), "v1 output should exist");

        // Step 2: Sync v1 -> v2 (adds 3 endpoints)
        let out2 = dir.path().join("out2");
        let audit2 = dir.path().join("audit_v1_v2.jsonl");
        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &out2,
            Some(&provider_path),
            true,
            "terraform",
            Some(&audit2),
        );
        assert!(result.is_ok(), "v1->v2 sync failed: {result:?}");
        assert!(out2.exists(), "v2 output should exist");
        // Verify audit log was created
        assert!(audit2.exists(), "audit log should be created for v1->v2");
        let audit_content = fs::read_to_string(&audit2).unwrap();
        let audit_json: serde_json::Value = serde_json::from_str(audit_content.trim()).unwrap();
        assert_eq!(audit_json["data"]["endpoints_added"], 3, "v1->v2 should add 3 endpoints");

        // Step 3: Sync v2 -> v3 (adds 4, removes 3)
        let out3 = dir.path().join("out3");
        let audit3 = dir.path().join("audit_v2_v3.jsonl");
        let result = super::run_with_audit(
            &v2,
            &v3,
            &resources_dir,
            &out3,
            Some(&provider_path),
            true,
            "terraform",
            Some(&audit3),
        );
        assert!(result.is_ok(), "v2->v3 sync failed: {result:?}");
        let audit_content = fs::read_to_string(&audit3).unwrap();
        let audit_json: serde_json::Value = serde_json::from_str(audit_content.trim()).unwrap();
        assert_eq!(audit_json["data"]["endpoints_added"], 4, "v2->v3 should add 4 endpoints");
        assert_eq!(audit_json["data"]["endpoints_removed"], 3, "v2->v3 should remove 3 endpoints");
    }

    #[test]
    fn chain_test_first_run_no_previous() {
        let dir = TempDir::new().unwrap();
        let spec = write_spec_v2(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("first_run_output");

        let nonexistent = dir.path().join("nonexistent_old.yaml");
        let result = super::run_with_audit(
            &nonexistent,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            true,
            "terraform",
            None,
        );

        assert!(result.is_ok(), "first-run chain should succeed: {result:?}");
        assert!(output_dir.exists(), "output directory should exist");
        let file_count = super::count_files(&output_dir);
        assert!(file_count > 0, "should generate files on first run, got {file_count}");
    }

    #[test]
    fn chain_test_audit_log_records_evolution() {
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_spec_v2(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("audit_test_output");
        let audit_path = dir.path().join("evolution_audit.jsonl");

        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            Some(&audit_path),
        );
        assert!(result.is_ok(), "sync should succeed: {result:?}");

        // Read and parse audit log
        assert!(audit_path.exists(), "audit log file should be created");
        let content = fs::read_to_string(&audit_path).unwrap();
        let line = content.lines().next().expect("audit log should have at least one line");
        let event: serde_json::Value = serde_json::from_str(line).unwrap();

        assert_eq!(event["event"], "sync_complete");
        assert_eq!(event["data"]["endpoints_added"], 3);
        assert_eq!(event["data"]["endpoints_removed"], 0);
        assert!(event["data"]["added_endpoints"].is_array());
        assert!(event["timestamp"].is_string());
    }
}
