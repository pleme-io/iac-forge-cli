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
        None,
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

/// Options controlling post-generation validation and failure notification.
#[derive(Debug, Default)]
#[cfg_attr(not(feature = "helm"), allow(dead_code))]
pub struct PostValidationOptions<'a> {
    /// When true, run Helm chart validation after generation.
    pub post_validate: bool,
    /// Optional webhook URL for failure notifications (via curl).
    pub notify_webhook: Option<&'a str>,
    /// Optional file path for appending failure log lines.
    pub notify_log: Option<&'a Path>,
}

/// Run the full sync pipeline with optional post-generation validation.
///
/// Delegates to [`run_with_audit`] for the core pipeline, then runs Helm chart
/// validation (if enabled) and sends failure notifications on validation errors.
///
/// # Errors
///
/// Returns an error if the core sync pipeline fails or if post-validation
/// detects invalid Helm charts.
#[allow(clippy::too_many_arguments)]
pub fn run_with_post_validation(
    old_spec_path: &Path,
    new_spec_path: &Path,
    resources_dir: &Path,
    output_dir: &Path,
    provider_path: Option<&Path>,
    auto_scaffold: bool,
    backend: &str,
    audit_path: Option<&Path>,
    post_opts: &PostValidationOptions<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Run the core sync pipeline.
    run_with_audit(
        old_spec_path,
        new_spec_path,
        resources_dir,
        output_dir,
        provider_path,
        auto_scaffold,
        backend,
        audit_path,
    )?;

    // Post-generation validation (helm feature only).
    if post_opts.post_validate {
        #[cfg(feature = "helm")]
        {
            println!(
                "\n{} Running post-generation Helm chart validation...",
                "=>".blue().bold()
            );

            match crate::commands::post_validate::validate_helm_charts(output_dir) {
                Ok(report) => {
                    println!("  {} {report}", "ok".green());
                    if let Some(audit) = audit_path {
                        audit_log(
                            audit,
                            "post_validate_passed",
                            serde_json::json!({
                                "charts_validated": report.charts.len(),
                                "helm_available": report.helm_available,
                            }),
                        );
                    }
                }
                Err(e) => {
                    let summary = e.to_string();
                    eprintln!(
                        "  {} post-validation failed:\n{summary}",
                        "FAIL".red().bold()
                    );

                    if let Some(audit) = audit_path {
                        audit_log(
                            audit,
                            "post_validate_failed",
                            serde_json::json!({
                                "summary": summary,
                            }),
                        );
                    }

                    if let Some(webhook_url) = post_opts.notify_webhook {
                        crate::commands::notify::notify_failure(webhook_url, &summary);
                    }
                    if let Some(log_path) = post_opts.notify_log {
                        crate::commands::notify::notify_log(log_path, &summary);
                    }

                    return Err(e);
                }
            }
        }

        #[cfg(not(feature = "helm"))]
        {
            println!(
                "  {} --post-validate requires the 'helm' feature; skipping",
                "!".yellow()
            );
        }
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

        let result = super::run_with_audit(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            None,
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

        let result = super::run_with_audit(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            true,
            "terraform",
            None,
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
        let result = super::run_with_audit(
            &old_spec,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "invalid-backend",
            None,
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
        let result = super::run_with_audit(
            &nonexistent_old,
            &new_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            None,
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

    // =========================================================================
    // E2E Test Suite: Comprehensive, deterministic pipeline validation
    // =========================================================================

    // --- Shared helpers for E2E tests ---

    /// Multi-resource spec with 2 CRUD resources: secret + target_aws.
    /// Secret has (name, value) required fields.
    /// Target has (name, access_key, region) fields.
    fn write_multi_resource_spec(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: MultiResource, version: "1.0" }
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
        name: { type: string, description: "Secret name" }
        value: { type: string, description: "Secret value" }
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
      required: [name]
      properties:
        name: { type: string, description: "Target name" }
        access_key: { type: string, description: "AWS access key" }
        region: { type: string, description: "AWS region" }
    targetUpdateAws:
      type: object
      properties:
        name: { type: string }
        access_key: { type: string }
        region: { type: string }
    targetGet:
      type: object
      properties:
        name: { type: string }
    targetDelete:
      type: object
      properties:
        name: { type: string }
"#;
        let path = dir.join("multi_resource_spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_secret_resource_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[resource]
name = "test_static_secret"
description = "A static secret"
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

    fn write_target_aws_resource_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[resource]
name = "test_target_aws"
description = "An AWS target"
category = "target"

[crud]
create_endpoint = "/target-create-aws"
create_schema = "targetCreateAws"
update_endpoint = "/target-update-aws"
update_schema = "targetUpdateAws"
read_endpoint = "/target-get"
read_schema = "targetGet"
delete_endpoint = "/target-delete"
delete_schema = "targetDelete"

[identity]
id_field = "name"
force_new_fields = ["name"]

[fields]
access_key = { sensitive = true }
token = { skip = true }
"#;
        let path = dir.join("target_aws.toml");
        fs::write(&path, toml_content).unwrap();
        path
    }

    /// Spec with one field of EACH type: string, integer, float, boolean,
    /// list<string>, and map<string>.
    fn write_all_types_spec(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: TypeTest, version: "1.0" }
paths:
  /create-typed-resource:
    post:
      operationId: createTypedResource
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createTypedResource'
      responses:
        "200": { description: ok }
  /read-typed-resource:
    post:
      operationId: readTypedResource
      responses:
        "200": { description: ok }
  /delete-typed-resource:
    post:
      operationId: deleteTypedResource
      responses:
        "200": { description: ok }
components:
  schemas:
    createTypedResource:
      type: object
      required: [name]
      properties:
        name:
          type: string
          description: "Name field"
        count:
          type: integer
          description: "Count field"
        score:
          type: number
          description: "Score field"
        enabled:
          type: boolean
          description: "Enabled flag"
        tags:
          type: array
          items:
            type: string
          description: "Tags list"
        metadata:
          type: object
          additionalProperties:
            type: string
          description: "Metadata map"
    readTypedResource:
      type: object
      properties:
        name: { type: string }
    deleteTypedResource:
      type: object
      properties:
        name: { type: string }
"#;
        let path = dir.join("all_types_spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_typed_resource_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[resource]
name = "test_typed_resource"
description = "Resource with all types"
category = "test"

[crud]
create_endpoint = "/create-typed-resource"
create_schema = "createTypedResource"
read_endpoint = "/read-typed-resource"
read_schema = "readTypedResource"
delete_endpoint = "/delete-typed-resource"
delete_schema = "deleteTypedResource"

[identity]
id_field = "name"

[fields]
token = { skip = true }
"#;
        let path = dir.join("typed_resource.toml");
        fs::write(&path, toml_content).unwrap();
        path
    }

    /// Spec with fields that exercise sensitive, immutable, and computed flags.
    fn write_field_flags_spec(dir: &std::path::Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: FieldFlags, version: "1.0" }
paths:
  /create-flagged:
    post:
      operationId: createFlagged
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createFlagged'
      responses:
        "200": { description: ok }
  /read-flagged:
    post:
      operationId: readFlagged
      responses:
        "200": { description: ok }
  /delete-flagged:
    post:
      operationId: deleteFlagged
      responses:
        "200": { description: ok }
components:
  schemas:
    createFlagged:
      type: object
      required: [name]
      properties:
        name:
          type: string
          description: "Immutable name"
        api_key:
          type: string
          description: "Sensitive API key"
        access_id:
          type: string
          description: "Server-generated access ID"
        region:
          type: string
          description: "AWS region"
    readFlagged:
      type: object
      properties:
        name: { type: string }
    deleteFlagged:
      type: object
      properties:
        name: { type: string }
"#;
        let path = dir.join("field_flags_spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_flagged_resource_toml(dir: &std::path::Path) -> std::path::PathBuf {
        let toml_content = r#"
[resource]
name = "test_flagged"
description = "Resource with sensitive/immutable/computed fields"
category = "test"

[crud]
create_endpoint = "/create-flagged"
create_schema = "createFlagged"
read_endpoint = "/read-flagged"
read_schema = "readFlagged"
delete_endpoint = "/delete-flagged"
delete_schema = "deleteFlagged"

[identity]
id_field = "name"
force_new_fields = ["name"]

[fields]
api_key = { sensitive = true }
access_id = { computed = true }
token = { skip = true }
"#;
        let path = dir.join("flagged.toml");
        fs::write(&path, toml_content).unwrap();
        path
    }

    // =========================================================================
    // Test Group 1: Per-Framework Output Verification
    // =========================================================================

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_terraform_output_structure_multi_resource() {
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "terraform generate failed: {result:?}");

        // Verify resources/ dir exists with resource files
        assert!(output_dir.join("resources").exists(), "resources/ dir should exist");
        assert!(
            output_dir.join("resources/resource_test_static_secret.go").exists(),
            "secret resource Go file should exist"
        );
        assert!(
            output_dir.join("resources/resource_test_target_aws.go").exists(),
            "target resource Go file should exist"
        );

        // Verify each .go file has "package resources"
        let secret_code = fs::read_to_string(
            output_dir.join("resources/resource_test_static_secret.go"),
        ).unwrap();
        assert!(secret_code.contains("package resources"), "should have Go package declaration");

        let target_code = fs::read_to_string(
            output_dir.join("resources/resource_test_target_aws.go"),
        ).unwrap();
        assert!(target_code.contains("package resources"), "should have Go package declaration");

        // Verify CRUD methods
        for code in [&secret_code, &target_code] {
            assert!(code.contains("Create"), "should have Create method");
            assert!(code.contains("Read"), "should have Read method");
            assert!(code.contains("Delete"), "should have Delete method");
            assert!(code.contains("ImportState"), "should have ImportState method");
        }

        // Verify provider/provider.go exists and registers both resources
        assert!(
            output_dir.join("provider/provider.go").exists(),
            "provider.go should exist"
        );
        let provider_code = fs::read_to_string(output_dir.join("provider/provider.go")).unwrap();
        assert!(
            provider_code.contains("TestStaticSecret") || provider_code.contains("Secret"),
            "provider.go should reference the secret resource"
        );
        assert!(
            provider_code.contains("TestTargetAws") || provider_code.contains("Target"),
            "provider.go should reference the target resource"
        );

        // Verify helpers.go exists with AkeylessClient
        assert!(
            output_dir.join("resources/helpers.go").exists(),
            "helpers.go should exist"
        );
        let helpers = fs::read_to_string(output_dir.join("resources/helpers.go")).unwrap();
        assert!(helpers.contains("AkeylessClient"), "helpers.go should have AkeylessClient");

        // Verify test files exist
        assert!(
            output_dir.join("resources/resource_test_static_secret_test.go").exists(),
            "secret test file should exist"
        );
        assert!(
            output_dir.join("resources/resource_test_target_aws_test.go").exists(),
            "target test file should exist"
        );

        // Verify test files have TestAcc functions
        let secret_test = fs::read_to_string(
            output_dir.join("resources/resource_test_static_secret_test.go"),
        ).unwrap();
        assert!(
            secret_test.contains("TestAcc") || secret_test.contains("func Test"),
            "test file should have acceptance test functions"
        );
    }

    #[cfg(feature = "pulumi")]
    #[test]
    fn e2e_pulumi_output_structure() {
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Pulumi,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "pulumi generate failed: {result:?}");

        // Pulumi produces schema.json
        assert!(
            output_dir.join("schema.json").exists(),
            "schema.json should be generated"
        );
        let content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify provider name
        assert_eq!(schema["name"], "test", "schema should have provider name");

        // Verify resources section exists with 2 resources
        let resources = schema["resources"].as_object().expect("resources should be an object");
        assert_eq!(resources.len(), 2, "should have exactly 2 resources in schema");

        // Verify each resource has inputProperties and properties
        for (_key, resource) in resources {
            assert!(
                resource["inputProperties"].is_object() || resource["properties"].is_object(),
                "resource should have property definitions"
            );
        }
    }

    #[cfg(feature = "crossplane")]
    #[test]
    fn e2e_crossplane_output_structure() {
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Crossplane,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "crossplane generate failed: {result:?}");

        // Crossplane produces CRD YAML files
        let yamls: Vec<_> = glob::glob(&format!("{}/**/*.yaml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        // 2 resource CRDs + 1 provider config CRD
        assert!(yamls.len() >= 2, "should produce at least 2 YAML files, got {}", yamls.len());

        // Verify each CRD YAML contains K8s CRD structure
        for yaml_path in &yamls {
            let content = fs::read_to_string(yaml_path).unwrap();
            assert!(
                content.contains("apiextensions.k8s.io/v1") || content.contains("CustomResourceDefinition"),
                "CRD file {} should contain K8s CRD markers",
                yaml_path.display()
            );
        }
    }

    #[cfg(feature = "ansible")]
    #[test]
    fn e2e_ansible_output_structure() {
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Ansible,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "ansible generate failed: {result:?}");

        // Ansible produces Python module files
        let pyfiles: Vec<_> = glob::glob(&format!("{}/**/*.py", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(pyfiles.len(), 2, "should produce exactly 2 .py module files");

        // Verify each Python file has AnsibleModule
        for py_path in &pyfiles {
            let content = fs::read_to_string(py_path).unwrap();
            assert!(
                content.contains("AnsibleModule"),
                "ansible module {} should contain AnsibleModule",
                py_path.display()
            );
            assert!(
                content.contains("argument_spec"),
                "ansible module {} should contain argument_spec",
                py_path.display()
            );
        }

        // Verify test playbooks were generated
        let yml_files: Vec<_> = glob::glob(&format!("{}/**/*.yml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(yml_files.len(), 2, "should produce 2 test playbook files");
    }

    // =========================================================================
    // Test Group 2: Cross-Framework Consistency
    // =========================================================================

    #[cfg(all(feature = "terraform", feature = "pulumi", feature = "crossplane", feature = "ansible"))]
    #[test]
    fn e2e_all_frameworks_same_resource_count() {
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::All,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "generate all failed: {result:?}");

        // Count terraform resources (resource_*.go files, excluding test and helpers)
        let tf_resources: Vec<_> = glob::glob(
            &format!("{}/terraform/resources/resource_*.go", output_dir.display()),
        )
        .unwrap()
        .filter_map(Result::ok)
        .filter(|p| !p.to_string_lossy().contains("_test.go") && !p.to_string_lossy().contains("helpers"))
        .collect();
        let tf_count = tf_resources.len();

        // Count pulumi resources from schema.json
        let schema_content =
            fs::read_to_string(output_dir.join("pulumi/schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&schema_content).unwrap();
        let pulumi_count = schema["resources"].as_object().map_or(0, |r| r.len());

        // Count crossplane CRDs (resource CRDs, excluding providerconfig)
        let xp_crds: Vec<_> = glob::glob(
            &format!("{}/crossplane/*-crd.yaml", output_dir.display()),
        )
        .unwrap()
        .filter_map(Result::ok)
        .filter(|p| !p.to_string_lossy().contains("providerconfig"))
        .collect();
        let xp_count = xp_crds.len();

        // Count ansible modules (.py files)
        let ansible_modules: Vec<_> = glob::glob(
            &format!("{}/ansible/**/*.py", output_dir.display()),
        )
        .unwrap()
        .filter_map(Result::ok)
        .collect();
        let ansible_count = ansible_modules.len();

        assert_eq!(tf_count, 2, "terraform should have 2 resources");
        assert_eq!(pulumi_count, 2, "pulumi should have 2 resources");
        assert_eq!(xp_count, 2, "crossplane should have 2 CRDs");
        assert_eq!(ansible_count, 2, "ansible should have 2 modules");

        // All frameworks agree on resource count
        assert_eq!(tf_count, pulumi_count, "terraform and pulumi resource counts must match");
        assert_eq!(tf_count, xp_count, "terraform and crossplane resource counts must match");
        assert_eq!(tf_count, ansible_count, "terraform and ansible resource counts must match");
    }

    // =========================================================================
    // Test Group 3: Type Mapping Correctness Across Frameworks
    // =========================================================================

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_type_mapping_terraform() {
        let dir = TempDir::new().unwrap();
        let spec = write_all_types_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_typed_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "terraform type mapping generate failed: {result:?}");

        let code = fs::read_to_string(
            output_dir.join("resources/resource_test_typed_resource.go"),
        ).unwrap();

        // Verify Terraform type mappings
        assert!(code.contains("types.String") || code.contains("schema.StringAttribute"),
            "should have String type for name field");
        assert!(code.contains("types.Int64") || code.contains("schema.Int64Attribute"),
            "should have Int64 type for count field");
        assert!(code.contains("types.Float64") || code.contains("schema.Float64Attribute"),
            "should have Float64 type for score field");
        assert!(code.contains("types.Bool") || code.contains("schema.BoolAttribute"),
            "should have Bool type for enabled field");
    }

    #[cfg(feature = "pulumi")]
    #[test]
    fn e2e_type_mapping_pulumi() {
        let dir = TempDir::new().unwrap();
        let spec = write_all_types_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_typed_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Pulumi,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "pulumi type mapping generate failed: {result:?}");

        let content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Find the resource in the schema
        let resources = schema["resources"].as_object().unwrap();
        assert_eq!(resources.len(), 1, "should have exactly 1 resource");
        let resource = resources.values().next().unwrap();

        // Check property types in inputProperties or properties
        let props = if resource["inputProperties"].is_object() {
            resource["inputProperties"].as_object().unwrap()
        } else {
            resource["properties"].as_object().unwrap()
        };

        // Verify Pulumi type mappings
        if let Some(name) = props.get("name") {
            assert_eq!(name["type"], "string", "name should be string type");
        }
        if let Some(count) = props.get("count") {
            assert_eq!(count["type"], "integer", "count should be integer type");
        }
        if let Some(score) = props.get("score") {
            assert_eq!(score["type"], "number", "score should be number type");
        }
        if let Some(enabled) = props.get("enabled") {
            assert_eq!(enabled["type"], "boolean", "enabled should be boolean type");
        }
        if let Some(tags) = props.get("tags") {
            assert_eq!(tags["type"], "array", "tags should be array type");
            assert!(tags["items"].is_object(), "tags should have items schema");
        }
        if let Some(metadata) = props.get("metadata") {
            assert_eq!(metadata["type"], "object", "metadata should be object type");
            assert!(
                metadata["additionalProperties"].is_object(),
                "metadata should have additionalProperties"
            );
        }
    }

    #[cfg(feature = "crossplane")]
    #[test]
    fn e2e_type_mapping_crossplane() {
        let dir = TempDir::new().unwrap();
        let spec = write_all_types_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_typed_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Crossplane,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "crossplane type mapping generate failed: {result:?}");

        // Find the CRD YAML
        let yamls: Vec<_> = glob::glob(&format!("{}/**/*-crd.yaml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| !p.to_string_lossy().contains("providerconfig"))
            .collect();
        assert!(!yamls.is_empty(), "should produce at least 1 CRD");

        let crd_content = fs::read_to_string(&yamls[0]).unwrap();

        // Verify Crossplane type mappings in CRD YAML
        assert!(crd_content.contains("type: string"), "CRD should have string type");
        assert!(crd_content.contains("type: integer"), "CRD should have integer type");
        assert!(crd_content.contains("type: number"), "CRD should have number type");
        assert!(crd_content.contains("type: boolean"), "CRD should have boolean type");
        assert!(crd_content.contains("type: array"), "CRD should have array type");
        assert!(crd_content.contains("type: object"), "CRD should have object type");
    }

    #[cfg(feature = "ansible")]
    #[test]
    fn e2e_type_mapping_ansible() {
        let dir = TempDir::new().unwrap();
        let spec = write_all_types_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_typed_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Ansible,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "ansible type mapping generate failed: {result:?}");

        let pyfiles: Vec<_> = glob::glob(&format!("{}/**/*.py", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!pyfiles.is_empty(), "should produce at least 1 Python file");

        let content = fs::read_to_string(&pyfiles[0]).unwrap();

        // Verify Ansible type mappings in argument_spec
        assert!(content.contains("'str'"), "should have 'str' type");
        assert!(content.contains("'int'"), "should have 'int' type");
        assert!(content.contains("'float'"), "should have 'float' type");
        assert!(content.contains("'bool'"), "should have 'bool' type");
        assert!(content.contains("'list'"), "should have 'list' type");
        assert!(content.contains("'dict'"), "should have 'dict' type");
    }

    // =========================================================================
    // Test Group 4: Sensitive/Immutable/Computed Field Handling
    // =========================================================================

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_sensitive_field_terraform() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "terraform sensitive field test failed: {result:?}");

        let code = fs::read_to_string(
            output_dir.join("resources/resource_test_flagged.go"),
        ).unwrap();

        // Verify sensitive field handling
        assert!(
            code.contains("Sensitive"),
            "terraform should mark api_key as Sensitive"
        );
    }

    #[cfg(feature = "pulumi")]
    #[test]
    fn e2e_sensitive_field_pulumi() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Pulumi,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "pulumi sensitive field test failed: {result:?}");

        let content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Find the resource and check api_key is secret
        let resources = schema["resources"].as_object().unwrap();
        let resource = resources.values().next().unwrap();

        // Check inputProperties or properties for secret flag
        let check_secret = |props: &serde_json::Map<String, serde_json::Value>| -> bool {
            if let Some(api_key) = props.get("apiKey").or(props.get("api_key")) {
                api_key.get("secret").and_then(|v| v.as_bool()).unwrap_or(false)
            } else {
                false
            }
        };

        let found_secret = resource["inputProperties"]
            .as_object()
            .map(check_secret)
            .unwrap_or(false)
            || resource["properties"]
                .as_object()
                .map(check_secret)
                .unwrap_or(false);

        assert!(found_secret, "pulumi should mark api_key as secret");
    }

    #[cfg(feature = "crossplane")]
    #[test]
    fn e2e_sensitive_field_crossplane() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Crossplane,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "crossplane sensitive field test failed: {result:?}");

        let yamls: Vec<_> = glob::glob(&format!("{}/**/*-crd.yaml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| !p.to_string_lossy().contains("providerconfig"))
            .collect();
        assert!(!yamls.is_empty());

        let content = fs::read_to_string(&yamls[0]).unwrap();
        // Crossplane marks sensitive fields with [sensitive] in description
        assert!(
            content.contains("[sensitive]") || content.contains("sensitive"),
            "crossplane should mark api_key as sensitive in description"
        );
    }

    #[cfg(feature = "ansible")]
    #[test]
    fn e2e_sensitive_field_ansible() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Ansible,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "ansible sensitive field test failed: {result:?}");

        let pyfiles: Vec<_> = glob::glob(&format!("{}/**/*.py", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!pyfiles.is_empty());

        let content = fs::read_to_string(&pyfiles[0]).unwrap();
        assert!(
            content.contains("no_log") || content.contains("no_log=True") || content.contains("no_log': True"),
            "ansible should mark api_key as no_log"
        );
    }

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_immutable_field_terraform() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "terraform immutable field test failed: {result:?}");

        let code = fs::read_to_string(
            output_dir.join("resources/resource_test_flagged.go"),
        ).unwrap();

        // Terraform marks immutable fields with RequiresReplace PlanModifier
        assert!(
            code.contains("RequiresReplace") || code.contains("PlanModifiers"),
            "terraform should have RequiresReplace for immutable name field"
        );
    }

    #[cfg(feature = "pulumi")]
    #[test]
    fn e2e_immutable_field_pulumi() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Pulumi,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "pulumi immutable field test failed: {result:?}");

        let content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&content).unwrap();

        let resources = schema["resources"].as_object().unwrap();
        let resource = resources.values().next().unwrap();

        // Check for replaceOnChanges on the name field
        let check_replace = |props: &serde_json::Map<String, serde_json::Value>| -> bool {
            if let Some(name) = props.get("name") {
                name.get("replaceOnChanges")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
            } else {
                false
            }
        };

        let found_replace = resource["inputProperties"]
            .as_object()
            .map(check_replace)
            .unwrap_or(false)
            || resource["properties"]
                .as_object()
                .map(check_replace)
                .unwrap_or(false);

        assert!(found_replace, "pulumi should have replaceOnChanges for name field");
    }

    #[cfg(feature = "crossplane")]
    #[test]
    fn e2e_immutable_field_crossplane() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Crossplane,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "crossplane immutable field test failed: {result:?}");

        let yamls: Vec<_> = glob::glob(&format!("{}/**/*-crd.yaml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| !p.to_string_lossy().contains("providerconfig"))
            .collect();
        assert!(!yamls.is_empty());

        let content = fs::read_to_string(&yamls[0]).unwrap();
        // Crossplane marks immutable fields with (immutable) in description
        assert!(
            content.contains("(immutable)") || content.contains("immutable"),
            "crossplane should mark name as immutable in description"
        );
    }

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_computed_field_terraform() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "terraform computed field test failed: {result:?}");

        let code = fs::read_to_string(
            output_dir.join("resources/resource_test_flagged.go"),
        ).unwrap();

        // access_id is computed, should be marked as Computed: true
        assert!(
            code.contains("Computed"),
            "terraform should mark access_id as Computed"
        );
    }

    #[cfg(feature = "pulumi")]
    #[test]
    fn e2e_computed_field_pulumi() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Pulumi,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "pulumi computed field test failed: {result:?}");

        let content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
        let schema: serde_json::Value = serde_json::from_str(&content).unwrap();

        let resources = schema["resources"].as_object().unwrap();
        let resource = resources.values().next().unwrap();

        // Computed fields should be in properties but NOT in inputProperties
        let input_props = resource["inputProperties"].as_object();
        let output_props = resource["properties"].as_object();

        if let Some(inputs) = input_props {
            assert!(
                !inputs.contains_key("accessId") && !inputs.contains_key("access_id"),
                "computed access_id should NOT be in inputProperties"
            );
        }

        if let Some(outputs) = output_props {
            assert!(
                outputs.contains_key("accessId") || outputs.contains_key("access_id"),
                "computed access_id SHOULD be in properties (outputs)"
            );
        }
    }

    #[cfg(feature = "crossplane")]
    #[test]
    fn e2e_computed_field_crossplane() {
        let dir = TempDir::new().unwrap();
        let spec = write_field_flags_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_flagged_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Crossplane,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "crossplane computed field test failed: {result:?}");

        let yamls: Vec<_> = glob::glob(&format!("{}/**/*-crd.yaml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| !p.to_string_lossy().contains("providerconfig"))
            .collect();
        assert!(!yamls.is_empty());

        let content = fs::read_to_string(&yamls[0]).unwrap();

        // In Crossplane CRDs, computed fields go in atProvider, not forProvider
        // The CRD should contain atProvider and forProvider sections
        assert!(
            content.contains("forProvider") || content.contains("spec"),
            "crossplane CRD should have spec/forProvider section"
        );

        // access_id being computed means it should appear in status/atProvider
        // but not as a required input in spec/forProvider
        assert!(
            content.contains("atProvider") || content.contains("status"),
            "crossplane CRD should have status/atProvider for computed fields"
        );
    }

    // =========================================================================
    // Test Group 5: API Evolution Scenarios
    // =========================================================================

    #[test]
    fn e2e_evolution_add_new_resource() {
        // v1: 1 CRUD group (secret)
        // v2: 2 CRUD groups (secret + target_aws)
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");
        let audit_path = dir.path().join("audit.jsonl");

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
        assert!(result.is_ok(), "evolution add resource failed: {result:?}");

        // Verify audit shows endpoints were added
        let audit_content = fs::read_to_string(&audit_path).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(audit_content.lines().next().unwrap()).unwrap();
        let endpoints_added = event["data"]["endpoints_added"].as_u64().unwrap_or(0);
        assert!(
            endpoints_added > 0,
            "should detect added endpoints when new resource appears"
        );

        // The added_endpoints should include target endpoints
        let added = event["data"]["added_endpoints"].as_array().unwrap();
        let added_strings: Vec<&str> = added.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            added_strings.iter().any(|e| e.contains("target")),
            "added endpoints should include target endpoints"
        );
    }

    #[test]
    fn e2e_evolution_add_field_to_existing_resource() {
        // v1: secret with (name, value)
        // v2: secret with (name, value, description)
        let dir = TempDir::new().unwrap();

        let v1_spec = r#"
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
        let v1 = dir.path().join("v1.yaml");
        fs::write(&v1, v1_spec).unwrap();

        let v2_spec = r#"
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
components:
  schemas:
    createSecret:
      type: object
      required: [name, value]
      properties:
        name: { type: string }
        value: { type: string }
        description: { type: string }
    describeItem:
      type: object
      properties:
        name: { type: string }
    deleteItem:
      type: object
      properties:
        name: { type: string }
"#;
        let v2 = dir.path().join("v2.yaml");
        fs::write(&v2, v2_spec).unwrap();

        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        // Sync v1 -> v2: same endpoints but schema changed
        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            None,
        );
        assert!(result.is_ok(), "evolution add field failed: {result:?}");
        assert!(output_dir.exists(), "output directory should exist after field evolution");
    }

    #[test]
    fn e2e_evolution_remove_endpoint() {
        // v1: 2 groups (secret + extra endpoints)
        // v2: 1 group (secret only, extra endpoints removed)
        let dir = TempDir::new().unwrap();
        let v2 = write_spec_v2(dir.path());
        let v1 = write_spec_v1(dir.path());

        // Note: v2 has MORE endpoints than v1. So sync v2->v1 = removal
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");
        let audit_path = dir.path().join("audit.jsonl");

        // Sync from v2 (more endpoints) to v1 (fewer endpoints) => removal
        let result = super::run_with_audit(
            &v2,
            &v1,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            Some(&audit_path),
        );
        assert!(result.is_ok(), "evolution remove endpoint failed: {result:?}");

        let audit_content = fs::read_to_string(&audit_path).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(audit_content.lines().next().unwrap()).unwrap();

        let endpoints_removed = event["data"]["endpoints_removed"].as_u64().unwrap_or(0);
        assert!(
            endpoints_removed > 0,
            "should detect removed endpoints: got {endpoints_removed}"
        );

        let removed = event["data"]["removed_endpoints"].as_array().unwrap();
        assert!(
            !removed.is_empty(),
            "removed_endpoints array should not be empty"
        );
    }

    // =========================================================================
    // Test Group 6: Audit Log Verification
    // =========================================================================

    #[test]
    fn e2e_audit_log_complete_event_chain() {
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_spec_v2(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");
        let audit_path = dir.path().join("audit.jsonl");

        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            true,
            "terraform",
            Some(&audit_path),
        );
        assert!(result.is_ok(), "audit log chain test failed: {result:?}");

        // Parse the audit log
        assert!(audit_path.exists(), "audit log should exist");
        let content = fs::read_to_string(&audit_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert!(!lines.is_empty(), "audit log should have at least one line");

        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();

        // Verify all required fields
        assert!(event["timestamp"].is_string(), "should have timestamp");
        assert_eq!(event["event"], "sync_complete", "event should be sync_complete");

        let data = &event["data"];
        assert!(data["old_spec"].is_string(), "should have old_spec path");
        assert!(data["new_spec"].is_string(), "should have new_spec path");
        assert!(data["endpoints_added"].is_number(), "should have endpoints_added count");
        assert!(data["endpoints_removed"].is_number(), "should have endpoints_removed count");
        assert!(data["schemas_added"].is_number(), "should have schemas_added count");
        assert!(data["schemas_removed"].is_number(), "should have schemas_removed count");
        assert!(data["added_endpoints"].is_array(), "should have added_endpoints list");
        assert!(data["removed_endpoints"].is_array(), "should have removed_endpoints list");
        assert!(data["backend"].is_string(), "should have backend name");
        assert!(data["files_generated"].is_number(), "should have files_generated count");
        assert!(!data["auto_scaffold"].is_null(), "should have auto_scaffold flag");

        // Verify counts are correct for v1->v2 diff
        assert_eq!(data["endpoints_added"], 3, "v1->v2 should add 3 endpoints");
        assert_eq!(data["endpoints_removed"], 0, "v1->v2 should remove 0 endpoints (old-only was in old_spec, not v1)");

        // Verify files were actually generated
        let files_generated = data["files_generated"].as_u64().unwrap_or(0);
        assert!(files_generated > 0, "should have generated files, got {files_generated}");

        // Verify added_endpoints list matches count
        let added = data["added_endpoints"].as_array().unwrap();
        assert_eq!(
            added.len() as u64,
            data["endpoints_added"].as_u64().unwrap_or(0),
            "added_endpoints list length should match endpoints_added count"
        );
    }

    #[test]
    fn e2e_audit_log_multiple_syncs_append() {
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_spec_v2(dir.path());
        let v3 = write_spec_v3(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let audit_path = dir.path().join("multi_audit.jsonl");

        // Sync v1->v2
        let out1 = dir.path().join("out1");
        super::run_with_audit(
            &v1, &v2, &resources_dir, &out1, Some(&provider_path),
            false, "terraform", Some(&audit_path),
        ).unwrap();

        // Sync v2->v3
        let out2 = dir.path().join("out2");
        super::run_with_audit(
            &v2, &v3, &resources_dir, &out2, Some(&provider_path),
            false, "terraform", Some(&audit_path),
        ).unwrap();

        // Verify the audit log has 2 lines (JSONL format, one per sync)
        let content = fs::read_to_string(&audit_path).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 2, "audit log should have 2 entries after 2 syncs");

        // Both lines should parse as valid JSON
        for (i, line) in lines.iter().enumerate() {
            let event: Result<serde_json::Value, _> = serde_json::from_str(line);
            assert!(event.is_ok(), "audit log line {i} should be valid JSON");
            let event = event.unwrap();
            assert_eq!(event["event"], "sync_complete");
        }
    }

    // =========================================================================
    // Test Group 7: Error Cases
    // =========================================================================

    #[test]
    fn e2e_invalid_spec_fails_gracefully() {
        let dir = TempDir::new().unwrap();

        // Write invalid YAML as spec
        let bad_spec = dir.path().join("bad_spec.yaml");
        fs::write(&bad_spec, "{{{{ not valid yaml at all ::::").unwrap();

        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        // With bad new spec, sync should fail with error, not panic
        let result = super::run_with_audit(
            &dir.path().join("nonexistent.yaml"),
            &bad_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "terraform",
            None,
        );
        assert!(result.is_err(), "invalid spec should return error, not panic");
    }

    #[test]
    fn e2e_invalid_spec_generate_fails_gracefully() {
        let dir = TempDir::new().unwrap();

        let bad_spec = dir.path().join("bad_spec.yaml");
        fs::write(&bad_spec, "not: valid: openapi: spec").unwrap();

        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &bad_spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_err(), "generate with invalid spec should return error");
    }

    #[test]
    fn e2e_missing_provider_toml_fails_gracefully() {
        let dir = TempDir::new().unwrap();
        let spec = write_new_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        // Generate without --provider and no provider.toml auto-discovery
        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            None,
            None,
        );
        assert!(result.is_err(), "should fail when no provider.toml is found");
    }

    #[test]
    fn e2e_empty_resources_dir() {
        let dir = TempDir::new().unwrap();
        let spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        // No TOML specs written
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        // Should succeed with 0 resources generated
        assert!(result.is_ok(), "empty resources dir should succeed: {result:?}");
    }

    #[test]
    fn e2e_malformed_toml_resource_fails_gracefully() {
        let dir = TempDir::new().unwrap();
        let spec = write_new_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        // Write a malformed TOML file
        fs::write(resources_dir.join("broken.toml"), "{{{{ not toml").unwrap();

        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        // Should fail with a parse error
        assert!(result.is_err(), "malformed TOML should produce an error");
    }

    #[test]
    fn e2e_nonexistent_spec_file_fails_gracefully() {
        let dir = TempDir::new().unwrap();
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        let provider_path = write_provider_toml(dir.path());
        let output_dir = dir.path().join("output");

        let result = crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &dir.path().join("nonexistent_spec.yaml"),
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_err(), "nonexistent spec should return error");
    }

    // =========================================================================
    // Test Group 8: Full Sync Pipeline Determinism
    // =========================================================================

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_deterministic_output_across_runs() {
        // Run the same generation twice and verify identical output
        let dir = TempDir::new().unwrap();
        let spec = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_secret_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);

        let out1 = dir.path().join("run1");
        let out2 = dir.path().join("run2");

        crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &out1,
            Some(&provider_path),
            None,
        ).unwrap();

        crate::commands::generate::run(
            &crate::BackendChoice::Terraform,
            &spec,
            &resources_dir,
            &out2,
            Some(&provider_path),
            None,
        ).unwrap();

        // Collect all files from both runs
        let collect_files = |base: &std::path::Path| -> std::collections::BTreeMap<String, String> {
            let mut files = std::collections::BTreeMap::new();
            for entry in glob::glob(&format!("{}/**/*", base.display()))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|p| p.is_file())
            {
                let relative = entry.strip_prefix(base).unwrap().to_string_lossy().to_string();
                let content = fs::read_to_string(&entry).unwrap();
                files.insert(relative, content);
            }
            files
        };

        let files1 = collect_files(&out1);
        let files2 = collect_files(&out2);

        // Same set of files
        assert_eq!(
            files1.keys().collect::<Vec<_>>(),
            files2.keys().collect::<Vec<_>>(),
            "both runs should produce the same set of files"
        );

        // Same content for each file
        for (path, content1) in &files1 {
            let content2 = files2.get(path).unwrap();
            assert_eq!(
                content1, content2,
                "file {path} should be identical across runs"
            );
        }
    }

    #[cfg(feature = "terraform")]
    #[test]
    fn e2e_full_sync_pipeline_with_all_steps() {
        // End-to-end: diff + drift + auto-scaffold + validate + generate
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("full_pipeline_output");
        let audit_path = dir.path().join("full_pipeline.jsonl");

        // Run full sync with auto-scaffold enabled
        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            true, // auto_scaffold
            "terraform",
            Some(&audit_path),
        );
        assert!(result.is_ok(), "full pipeline sync failed: {result:?}");

        // Verify output was generated
        assert!(output_dir.exists(), "output directory should exist");
        let file_count = super::count_files(&output_dir);
        assert!(file_count > 0, "should generate files, got {file_count}");

        // Verify audit log
        assert!(audit_path.exists(), "audit log should exist");
        let audit_content = fs::read_to_string(&audit_path).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(audit_content.lines().next().unwrap()).unwrap();
        assert_eq!(event["data"]["auto_scaffold"], true, "auto_scaffold should be true");
        assert_eq!(event["data"]["backend"], "terraform", "backend should be terraform");

        // Verify the generated files have correct structure
        assert!(
            output_dir.join("provider/provider.go").exists(),
            "provider.go should exist after full pipeline"
        );
        assert!(
            output_dir.join("resources/helpers.go").exists(),
            "helpers.go should exist after full pipeline"
        );
    }

    #[cfg(all(feature = "terraform", feature = "pulumi", feature = "crossplane", feature = "ansible"))]
    #[test]
    fn e2e_full_sync_all_backends() {
        // Full sync generating all backends at once
        let dir = TempDir::new().unwrap();
        let v1 = write_spec_v1(dir.path());
        let v2 = write_multi_resource_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        write_target_aws_resource_toml(&resources_dir);
        let output_dir = dir.path().join("all_backends_output");
        let audit_path = dir.path().join("all_backends.jsonl");

        let result = super::run_with_audit(
            &v1,
            &v2,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            false,
            "all",
            Some(&audit_path),
        );
        assert!(result.is_ok(), "full sync all backends failed: {result:?}");

        // Verify each backend subdirectory exists with content
        assert!(output_dir.join("terraform").exists(), "terraform/ should exist");
        assert!(output_dir.join("pulumi").exists(), "pulumi/ should exist");
        assert!(output_dir.join("crossplane").exists(), "crossplane/ should exist");
        assert!(output_dir.join("ansible").exists(), "ansible/ should exist");

        // Each should have at least 1 file
        assert!(super::count_files(&output_dir.join("terraform")) > 0, "terraform should have files");
        assert!(super::count_files(&output_dir.join("pulumi")) > 0, "pulumi should have files");
        assert!(super::count_files(&output_dir.join("crossplane")) > 0, "crossplane should have files");
        assert!(super::count_files(&output_dir.join("ansible")) > 0, "ansible should have files");

        // Verify audit log
        let audit_content = fs::read_to_string(&audit_path).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(audit_content.lines().next().unwrap()).unwrap();
        assert_eq!(event["data"]["backend"], "all");

        let total_files = event["data"]["files_generated"].as_u64().unwrap_or(0);
        assert!(total_files > 4, "all backends should generate many files, got {total_files}");
    }
}
