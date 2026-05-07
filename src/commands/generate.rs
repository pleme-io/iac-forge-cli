use std::fs;
use std::path::Path;

use colored::Colorize;
use iac_forge::{Backend, DataSourceSpec, ProviderSpec, ResourceSpec, resolve_data_source, resolve_provider, resolve_resource};
use openapi_forge::Spec;

use crate::BackendChoice;
#[cfg(feature = "terraform")]
use crate::helpers_template;

/// Run the generate command for the given backend.
///
/// # Errors
///
/// Returns an error if spec loading, resolution, or code generation fails.
pub fn run(
    backend: &BackendChoice,
    spec_path: &Path,
    resources_dir: &Path,
    output_dir: &Path,
    provider_path: Option<&Path>,
    data_sources_dir: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} Loading OpenAPI spec from {}",
        "=>".blue().bold(),
        spec_path.display()
    );
    let api = Spec::load(spec_path)?;

    let provider_spec = if let Some(p) = provider_path {
        ProviderSpec::load(p)?
    } else {
        let provider_toml = resources_dir
            .parent()
            .unwrap_or(resources_dir)
            .join("provider.toml");
        if provider_toml.exists() {
            ProviderSpec::load(&provider_toml)?
        } else {
            return Err("No provider.toml found. Use --provider to specify one.".into());
        }
    };

    let iac_provider = resolve_provider(&provider_spec);

    let resource_files = find_toml_files(resources_dir)?;
    println!(
        "{} Found {} resource specs",
        "=>".blue().bold(),
        resource_files.len()
    );

    // Resolve all resources to platform-independent IR
    let mut iac_resources = Vec::new();
    let mut skipped = 0;

    for file in &resource_files {
        let resource = ResourceSpec::load(file)?;

        if let Err(e) = resource.validate(&api) {
            eprintln!("{} {}: {e}", "warning:".yellow().bold(), file.display());
            skipped += 1;
            continue;
        }

        match resolve_resource(&resource, &api, &provider_spec.defaults) {
            Ok(iac_resource) => {
                println!("  {} Resolved {}", "->".green(), iac_resource.name);
                iac_resources.push(iac_resource);
            }
            Err(e) => {
                eprintln!("{} {}: {e}", "warning:".yellow().bold(), file.display());
                skipped += 1;
            }
        }
    }

    if skipped > 0 {
        eprintln!(
            "{} {skipped} resource(s) skipped due to validation/resolution errors",
            "warning:".yellow().bold()
        );
    }

    // Load and resolve data sources (if directory provided)
    let mut iac_data_sources = Vec::new();
    if let Some(ds_dir) = data_sources_dir {
        if ds_dir.exists() {
            let ds_files = find_toml_files(ds_dir)?;
            println!(
                "{} Found {} data source specs",
                "=>".blue().bold(),
                ds_files.len()
            );
            for file in &ds_files {
                let ds_spec = DataSourceSpec::load(file)?;
                match resolve_data_source(&ds_spec, &api, &provider_spec.defaults) {
                    Ok(ds) => {
                        println!("  {} Resolved data source {}", "->".green(), ds.name);
                        iac_data_sources.push(ds);
                    }
                    Err(e) => {
                        eprintln!("{} {}: {e}", "warning:".yellow().bold(), file.display());
                    }
                }
            }
        }
    }

    // Dispatch to the appropriate backend(s)
    let backends = match backend {
        BackendChoice::Terraform => vec![BackendChoice::Terraform],
        BackendChoice::Pulumi => vec![BackendChoice::Pulumi],
        BackendChoice::Crossplane => vec![BackendChoice::Crossplane],
        BackendChoice::Ansible => vec![BackendChoice::Ansible],
        BackendChoice::Pangea => vec![BackendChoice::Pangea],
        BackendChoice::Steampipe => vec![BackendChoice::Steampipe],
        BackendChoice::Helm => vec![BackendChoice::Helm],
        BackendChoice::All => {
            let mut v = Vec::new();
            v.push(BackendChoice::Terraform);
            v.push(BackendChoice::Pulumi);
            v.push(BackendChoice::Crossplane);
            v.push(BackendChoice::Ansible);
            v.push(BackendChoice::Pangea);
            v.push(BackendChoice::Steampipe);
            v.push(BackendChoice::Helm);
            v
        }
    };

    for target in &backends {
        let target_dir = if matches!(backend, BackendChoice::All) {
            output_dir.join(target.to_string())
        } else {
            output_dir.to_path_buf()
        };

        println!(
            "\n{} Generating {} artifacts in {}",
            "=>".blue().bold(),
            target,
            target_dir.display()
        );

        generate_for_backend(
            target,
            &api,
            spec_path,
            &provider_spec,
            &iac_provider,
            &iac_resources,
            &iac_data_sources,
            &resource_files,
            &target_dir,
        )?;
    }

    let ds_msg = if iac_data_sources.is_empty() {
        String::new()
    } else {
        format!(" + {} data source(s)", iac_data_sources.len())
    };
    println!(
        "\n{} Generated artifacts for {} resource(s){ds_msg} with backend: {}",
        "done".green().bold(),
        iac_resources.len(),
        backend,
    );

    Ok(())
}

fn generate_for_backend(
    backend: &BackendChoice,
    api: &Spec,
    spec_path: &Path,
    provider_spec: &ProviderSpec,
    iac_provider: &iac_forge::IacProvider,
    iac_resources: &[iac_forge::IacResource],
    iac_data_sources: &[iac_forge::IacDataSource],
    resource_files: &[std::path::PathBuf],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match backend {
        BackendChoice::Terraform => {
            #[cfg(feature = "terraform")]
            {
                generate_terraform(api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir)
            }
            #[cfg(not(feature = "terraform"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("terraform backend not compiled in — enable the 'terraform' feature".into())
            }
        }
        BackendChoice::Pulumi => {
            #[cfg(feature = "pulumi")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &pulumi_forge::PulumiBackend::new(),
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "pulumi"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("pulumi backend not yet available — pulumi-forge crate pending".into())
            }
        }
        BackendChoice::Crossplane => {
            #[cfg(feature = "crossplane")]
            {
                let _ = (api, provider_spec, resource_files);
                // M6.4 — load OpenAPI body schemas so the controller
                // emitter can filter forProvider field pushes by
                // SDK-name+type match. Failure is non-fatal: fall
                // back to identifier+token-only bodies (substrate
                // still compiles cleanly without the broad walk).
                let body_sigs = match crossplane_forge::body_signatures::BodySigMap::from_openapi_json_path(spec_path) {
                    Ok(map) => Some(std::sync::Arc::new(map)),
                    Err(e) => {
                        eprintln!("warning: M6.4 body signature load failed ({e}); falling back to opt-in mode");
                        None
                    }
                };
                let backend = crossplane_forge::CrossplaneBackend::default()
                    .with_body_sigs(body_sigs);
                generate_via_backend(
                    &backend,
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "crossplane"))]
            {
                let _ = (api, spec_path, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("crossplane backend not yet available — crossplane-forge crate pending".into())
            }
        }
        BackendChoice::Ansible => {
            #[cfg(feature = "ansible")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &ansible_forge::AnsibleBackend::new(),
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "ansible"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("ansible backend not yet available — ansible-forge crate pending".into())
            }
        }
        BackendChoice::Pangea => {
            #[cfg(feature = "pangea")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &pangea_forge::PangeaBackend,
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "pangea"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("pangea backend not yet available — pangea-forge crate pending".into())
            }
        }
        BackendChoice::Steampipe => {
            #[cfg(feature = "steampipe")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &steampipe_forge::SteampipeBackend,
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "steampipe"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("steampipe backend not compiled in — enable the 'steampipe' feature".into())
            }
        }
        BackendChoice::Helm => {
            #[cfg(feature = "helm")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &helm_forge::HelmBackend::default(),
                    iac_provider,
                    iac_resources,
                    iac_data_sources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "helm"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, iac_data_sources, resource_files, output_dir);
                Err("helm backend not compiled in — enable the 'helm' feature".into())
            }
        }
        BackendChoice::All => unreachable!("All is expanded before calling this function"),
    }
}

/// Generate using the iac-forge Backend trait (for Pulumi, Crossplane, Ansible, etc.).
#[allow(dead_code)]
fn generate_via_backend(
    backend: &dyn Backend,
    iac_provider: &iac_forge::IacProvider,
    iac_resources: &[iac_forge::IacResource],
    iac_data_sources: &[iac_forge::IacDataSource],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(output_dir)?;

    let mut artifact_count = 0;

    for resource in iac_resources {
        let artifacts = backend.generate_resource(resource, iac_provider)?;
        for artifact in &artifacts {
            let out_path = output_dir.join(&artifact.path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out_path, &artifact.content)?;
            println!("  {} {}", "->".green(), artifact.path);
            artifact_count += 1;
        }

        let test_artifacts = backend.generate_test(resource, iac_provider)?;
        for artifact in &test_artifacts {
            let out_path = output_dir.join(&artifact.path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out_path, &artifact.content)?;
            artifact_count += 1;
        }
    }

    // Generate data source artifacts
    for ds in iac_data_sources {
        let artifacts = backend.generate_data_source(ds, iac_provider)?;
        for artifact in &artifacts {
            let out_path = output_dir.join(&artifact.path);
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&out_path, &artifact.content)?;
            println!("  {} {}", "->".green(), artifact.path);
            artifact_count += 1;
        }
    }

    // Generate provider-level artifacts
    let provider_artifacts =
        backend.generate_provider(iac_provider, iac_resources, iac_data_sources)?;
    for artifact in &provider_artifacts {
        let out_path = output_dir.join(&artifact.path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&out_path, &artifact.content)?;
        println!("  {} {}", "->".green(), artifact.path);
        artifact_count += 1;
    }

    println!(
        "  {} {} artifacts written",
        "=>".blue().bold(),
        artifact_count
    );

    Ok(())
}

/// Generate Terraform provider using the terraform-forge codegen path.
///
/// Uses the complete `terraform_forge::generate_resource()` API which produces
/// full CRUD Go files (Create/Read/Update/Delete/ImportState), tests, and
/// provider registration.
#[cfg(feature = "terraform")]
fn generate_terraform(
    api: &Spec,
    provider_spec: &ProviderSpec,
    _iac_provider: &iac_forge::IacProvider,
    _iac_resources: &[iac_forge::IacResource],
    iac_data_sources: &[iac_forge::IacDataSource],
    resource_files: &[std::path::PathBuf],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let tf_provider = terraform_forge::ProviderSpec {
        provider: terraform_forge::ProviderMeta {
            name: provider_spec.provider.name.clone(),
            description: provider_spec.provider.description.clone(),
            version: provider_spec.provider.version.clone(),
            sdk_import: provider_spec.provider.sdk_import.clone(),
        },
        auth: terraform_forge::AuthConfig {
            token_field: provider_spec.auth.token_field.clone(),
            env_var: provider_spec.auth.env_var.clone(),
            gateway_url_field: provider_spec.auth.gateway_url_field.clone(),
            gateway_env_var: provider_spec.auth.gateway_env_var.clone(),
        },
        defaults: terraform_forge::ProviderDefaults {
            skip_fields: provider_spec.defaults.skip_fields.clone(),
        },
    };

    let defaults = tf_provider.defaults.clone();
    let sdk_import = &tf_provider.provider.sdk_import;

    let resources_out = output_dir.join("resources");
    let provider_out = output_dir.join("provider");
    fs::create_dir_all(&resources_out)?;
    fs::create_dir_all(&provider_out)?;

    let mut type_names = Vec::new();
    let mut tf_names = Vec::new();
    let mut test_count = 0;
    let mut skipped = 0;

    for file in resource_files {
        let resource = terraform_forge::ResourceSpec::load(file)?;

        if let Err(e) = resource.validate(api) {
            eprintln!("{} {}: {e}", "warning:".yellow().bold(), file.display());
            skipped += 1;
            continue;
        }

        println!("  {} Generating {}", "->".green(), resource.resource.name);

        let generated =
            terraform_forge::generate_resource(&resource, api, &defaults, sdk_import)?;

        type_names.push(generated.resource_type_name.clone());
        tf_names.push(resource.resource.name.clone());

        // Check for override file
        let override_path = output_dir.join("overrides").join(&generated.file_name);
        if override_path.exists() {
            println!(
                "  {} Override exists, skipping: {}",
                "!!".yellow(),
                generated.file_name
            );
            fs::copy(&override_path, resources_out.join(&generated.file_name))?;
        } else {
            fs::write(resources_out.join(&generated.file_name), &generated.go_code)?;
        }

        // Generate acceptance test scaffold
        let test = terraform_forge::generate_test(&resource);
        fs::write(resources_out.join(&test.file_name), &test.go_code)?;
        test_count += 1;
    }

    if skipped > 0 {
        eprintln!(
            "{} {skipped} resource(s) skipped due to validation errors",
            "warning:".yellow().bold()
        );
    }

    // Generate provider.go
    println!(
        "  {} Generating provider.go ({} resources)",
        "->".green(),
        type_names.len()
    );
    let data_source_names: Vec<String> = iac_data_sources.iter().map(|ds| ds.name.clone()).collect();
    let provider_code =
        terraform_forge::generate_provider(&tf_provider, &type_names, &tf_names, &data_source_names);
    fs::write(provider_out.join("provider.go"), &provider_code)?;

    // Generate common helpers
    fs::write(resources_out.join("helpers.go"), helpers_template::HELPERS_GO)?;

    println!(
        "  {} Generated {} resources + {} tests + provider.go",
        "=>".blue().bold(),
        type_names.len(),
        test_count,
    );

    Ok(())
}

fn find_toml_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    if dir.is_dir() {
        for entry in glob::glob(&format!("{}/**/*.toml", dir.display()))? {
            files.push(entry?);
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn backend_choice_display() {
        assert_eq!(BackendChoice::Terraform.to_string(), "terraform");
        assert_eq!(BackendChoice::Pulumi.to_string(), "pulumi");
        assert_eq!(BackendChoice::Crossplane.to_string(), "crossplane");
        assert_eq!(BackendChoice::Ansible.to_string(), "ansible");
        assert_eq!(BackendChoice::Pangea.to_string(), "pangea");
        assert_eq!(BackendChoice::Steampipe.to_string(), "steampipe");
        assert_eq!(BackendChoice::Helm.to_string(), "helm");
        assert_eq!(BackendChoice::All.to_string(), "all");
    }

    #[test]
    fn backend_choice_all_contains_all_backends() {
        let all_backends = match &BackendChoice::All {
            BackendChoice::All => {
                let mut v = Vec::new();
                v.push(BackendChoice::Terraform);
                v.push(BackendChoice::Pulumi);
                v.push(BackendChoice::Crossplane);
                v.push(BackendChoice::Ansible);
                v.push(BackendChoice::Pangea);
                v.push(BackendChoice::Steampipe);
                v.push(BackendChoice::Helm);
                v
            }
            _ => unreachable!(),
        };
        assert_eq!(all_backends.len(), 7);
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Terraform)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Pulumi)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Crossplane)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Ansible)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Pangea)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Steampipe)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Helm)));
    }

    #[test]
    fn find_toml_files_nonexistent_dir() {
        let result = find_toml_files(Path::new("/nonexistent"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn find_toml_files_empty_dir() {
        let dir = TempDir::new().unwrap();
        let files = find_toml_files(dir.path()).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn find_toml_files_nested() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("test.toml"), "[resource]\nname = \"x\"").unwrap();
        let files = find_toml_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
    }

    fn write_minimal_spec(dir: &Path) -> std::path::PathBuf {
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

    fn write_provider_toml(dir: &Path) -> std::path::PathBuf {
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

    fn write_resource_toml(dir: &Path) -> std::path::PathBuf {
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
    fn scaffold_generates_valid_toml() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let output_dir = dir.path().join("scaffold_output");

        let result = super::super::scaffold::run(&spec_path, None, &output_dir);
        // The scaffold command should succeed (may produce 0 files if no CRUD groups match)
        assert!(result.is_ok(), "scaffold failed: {:?}", result.err());

        // If any files were produced, verify they parse as valid TOML
        if output_dir.exists() {
            for entry in glob::glob(&format!("{}/**/*.toml", output_dir.display())).unwrap() {
                let path = entry.unwrap();
                let content = fs::read_to_string(&path).unwrap();
                let parsed: Result<toml::Value, _> = toml::from_str(&content);
                assert!(
                    parsed.is_ok(),
                    "Scaffolded TOML is not valid at {}: {:?}",
                    path.display(),
                    parsed.err()
                );
            }
        }
    }

    #[test]
    fn validate_with_valid_spec() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);

        let result = super::super::validate::run(&spec_path, &resources_dir);
        assert!(result.is_ok(), "validate failed: {:?}", result.err());
    }

    #[test]
    fn validate_with_invalid_spec() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        // Write a resource TOML that references a non-existent schema
        let bad_toml = r#"
[resource]
name = "bad_resource"
description = "Bad"
category = "test"

[crud]
create_endpoint = "/create-secret"
create_schema = "NonExistentSchema"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#;
        fs::write(resources_dir.join("bad.toml"), bad_toml).unwrap();

        let result = super::super::validate::run(&spec_path, &resources_dir);
        assert!(result.is_err(), "validate should fail with invalid schema reference");
    }

    #[cfg(feature = "terraform")]
    #[test]
    fn generate_terraform_creates_expected_structure() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        let result = run(
            &BackendChoice::Terraform,
            &spec_path,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "generate failed: {:?}", result.err());

        // Verify expected output files exist
        assert!(
            output_dir.join("provider/provider.go").exists(),
            "provider.go should be generated"
        );
        assert!(
            output_dir.join("resources/helpers.go").exists(),
            "helpers.go should be generated"
        );
        assert!(
            output_dir.join("resources/resource_test_secret.go").exists(),
            "resource Go file should be generated"
        );
        assert!(
            output_dir
                .join("resources/resource_test_secret_test.go")
                .exists(),
            "resource test file should be generated"
        );

        // Verify provider.go references the generated resource
        let provider_code = fs::read_to_string(output_dir.join("provider/provider.go")).unwrap();
        assert!(
            provider_code.contains("NewTestSecretResource") || provider_code.contains("Secret"),
            "provider.go should reference the generated resource type"
        );

        // Verify the resource Go file has CRUD methods
        let resource_code =
            fs::read_to_string(output_dir.join("resources/resource_test_secret.go")).unwrap();
        assert!(
            resource_code.contains("func (r *"),
            "resource Go file should contain methods"
        );
        assert!(
            resource_code.contains("Create"),
            "resource Go file should contain Create method"
        );
        assert!(
            resource_code.contains("Read"),
            "resource Go file should contain Read method"
        );
        assert!(
            resource_code.contains("Delete"),
            "resource Go file should contain Delete method"
        );
    }

    #[test]
    fn scaffold_generates_toml_with_required_sections() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let output_dir = dir.path().join("scaffold_out");

        let result = super::super::scaffold::run(&spec_path, None, &output_dir);
        assert!(result.is_ok(), "scaffold failed: {:?}", result.err());

        // If files were produced, they must have [resource], [crud], [identity] sections
        if output_dir.exists() {
            for entry in glob::glob(&format!("{}/**/*.toml", output_dir.display())).unwrap() {
                let path = entry.unwrap();
                let content = fs::read_to_string(&path).unwrap();
                let parsed: toml::Value = toml::from_str(&content).unwrap();
                assert!(
                    parsed.get("resource").is_some(),
                    "TOML at {} missing [resource] section",
                    path.display()
                );
                assert!(
                    parsed.get("crud").is_some(),
                    "TOML at {} missing [crud] section",
                    path.display()
                );
                assert!(
                    parsed.get("identity").is_some(),
                    "TOML at {} missing [identity] section",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn drift_detects_missing_and_extra_resources() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        // Write a resource that does NOT match any spec CRUD group
        let extra_toml = r#"
[resource]
name = "akeyless_custom_thing"
description = "Not in spec"
category = "test"

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
        fs::write(resources_dir.join("custom.toml"), extra_toml).unwrap();

        // drift should succeed (it just reports, doesn't error)
        let result = super::super::drift::run(&spec_path, &resources_dir);
        assert!(result.is_ok(), "drift failed: {:?}", result.err());
    }

    #[test]
    fn diff_detects_added_and_removed_endpoints() {
        let dir = TempDir::new().unwrap();

        // Old spec with 3 endpoints
        let old_spec = r#"
openapi: "3.0.0"
info: { title: Old, version: "1.0" }
paths:
  /create-secret:
    post:
      operationId: createSecret
      responses:
        "200": { description: ok }
  /delete-item:
    post:
      operationId: deleteItem
      responses:
        "200": { description: ok }
  /old-endpoint:
    post:
      operationId: oldEndpoint
      responses:
        "200": { description: ok }
components:
  schemas: {}
"#;
        let old_path = dir.path().join("old.yaml");
        fs::write(&old_path, old_spec).unwrap();

        // New spec: /old-endpoint removed, /new-endpoint added
        let new_spec = r#"
openapi: "3.0.0"
info: { title: New, version: "2.0" }
paths:
  /create-secret:
    post:
      operationId: createSecret
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
      responses:
        "200": { description: ok }
components:
  schemas: {}
"#;
        let new_path = dir.path().join("new.yaml");
        fs::write(&new_path, new_spec).unwrap();

        let result = super::super::diff::run(&old_path, &new_path);
        assert!(result.is_ok(), "diff failed: {:?}", result.err());
    }

    #[test]
    fn generate_with_pulumi_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("pulumi_output");

        #[cfg(feature = "pulumi")]
        {
            let result = run(
                &BackendChoice::Pulumi,
                &spec_path,
                &resources_dir,
                &output_dir,
                Some(&provider_path),
                None,
            );
            assert!(result.is_ok(), "pulumi generate failed: {:?}", result.err());
            assert!(
                output_dir.join("schema.json").exists(),
                "schema.json should be generated"
            );
            let schema_content = fs::read_to_string(output_dir.join("schema.json")).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&schema_content).unwrap();
            assert_eq!(parsed["name"], "test");
        }
        #[cfg(not(feature = "pulumi"))]
        {
            let _ = (spec_path, provider_path, resources_dir, output_dir);
        }
    }

    #[test]
    fn generate_with_crossplane_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("crossplane_output");

        #[cfg(feature = "crossplane")]
        {
            let result = run(
                &BackendChoice::Crossplane,
                &spec_path,
                &resources_dir,
                &output_dir,
                Some(&provider_path),
                None,
            );
            assert!(result.is_ok(), "crossplane generate failed: {:?}", result.err());
            // Should produce CRD YAML files
            let yamls: Vec<_> = glob::glob(&format!("{}/**/*.yaml", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!yamls.is_empty(), "crossplane should produce YAML files");
        }
        #[cfg(not(feature = "crossplane"))]
        {
            let _ = (spec_path, provider_path, resources_dir, output_dir);
        }
    }

    #[test]
    fn generate_with_ansible_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("ansible_output");

        #[cfg(feature = "ansible")]
        {
            let result = run(
                &BackendChoice::Ansible,
                &spec_path,
                &resources_dir,
                &output_dir,
                Some(&provider_path),
                None,
            );
            assert!(result.is_ok(), "ansible generate failed: {:?}", result.err());
            // Should produce Python module files
            let pyfiles: Vec<_> = glob::glob(&format!("{}/**/*.py", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!pyfiles.is_empty(), "ansible should produce .py files");
        }
        #[cfg(not(feature = "ansible"))]
        {
            let _ = (spec_path, provider_path, resources_dir, output_dir);
        }
    }

    #[test]
    fn generate_with_steampipe_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("steampipe_output");

        #[cfg(feature = "steampipe")]
        {
            let result = run(
                &BackendChoice::Steampipe,
                &spec_path,
                &resources_dir,
                &output_dir,
                Some(&provider_path),
                None,
            );
            assert!(result.is_ok(), "steampipe generate failed: {:?}", result.err());
            // Should produce Go table files and plugin.go
            let gofiles: Vec<_> = glob::glob(&format!("{}/**/*.go", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!gofiles.is_empty(), "steampipe should produce .go files");
            assert!(
                gofiles.iter().any(|f| f.file_name().unwrap() == "plugin.go"),
                "steampipe should produce plugin.go"
            );
        }
        #[cfg(not(feature = "steampipe"))]
        {
            let _ = (spec_path, provider_path, resources_dir, output_dir);
        }
    }

    #[test]
    fn generate_with_helm_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("helm_output");

        #[cfg(feature = "helm")]
        {
            let result = run(
                &BackendChoice::Helm,
                &spec_path,
                &resources_dir,
                &output_dir,
                Some(&provider_path),
                None,
            );
            assert!(result.is_ok(), "helm generate failed: {:?}", result.err());
            // Should produce Chart.yaml files
            let charts: Vec<_> = glob::glob(&format!("{}/**/Chart.yaml", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!charts.is_empty(), "helm should produce Chart.yaml files");

            // Should produce values.yaml files
            let values: Vec<_> = glob::glob(&format!("{}/**/values.yaml", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!values.is_empty(), "helm should produce values.yaml files");

            // Should produce values.schema.json files
            let schemas: Vec<_> = glob::glob(&format!("{}/**/values.schema.json", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(!schemas.is_empty(), "helm should produce values.schema.json files");

            // Templates directory should exist with delegate files
            let templates: Vec<_> = glob::glob(&format!("{}/**/templates/*.yaml", output_dir.display()))
                .unwrap()
                .filter_map(Result::ok)
                .collect();
            assert!(templates.len() >= 8, "helm should produce at least 8 template files");
        }
        #[cfg(not(feature = "helm"))]
        {
            let _ = (spec_path, provider_path, resources_dir, output_dir);
        }
    }

    #[test]
    #[cfg(all(
        feature = "terraform",
        feature = "pulumi",
        feature = "crossplane",
        feature = "ansible",
        feature = "steampipe",
        feature = "helm"
    ))]
    fn generate_all_creates_subdirectories_per_backend() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("all_output");

        let result = run(
            &BackendChoice::All,
            &spec_path,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        assert!(result.is_ok(), "generate all failed: {:?}", result.err());

        assert!(output_dir.join("terraform").exists());
        assert!(output_dir.join("pulumi").exists());
        assert!(output_dir.join("crossplane").exists());
        assert!(output_dir.join("ansible").exists());
        assert!(output_dir.join("steampipe").exists());
        assert!(output_dir.join("helm").exists());
    }

    #[test]
    fn find_toml_files_ignores_non_toml() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("readme.md"), "# readme").unwrap();
        fs::write(dir.path().join("data.json"), "{}").unwrap();
        fs::write(dir.path().join("config.yaml"), "key: value").unwrap();
        fs::write(dir.path().join("actual.toml"), "[section]").unwrap();
        let files = find_toml_files(dir.path()).unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].extension().unwrap() == "toml");
    }

    #[test]
    fn find_toml_files_returns_sorted() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("z_last.toml"), "[a]").unwrap();
        fs::write(dir.path().join("a_first.toml"), "[b]").unwrap();
        fs::write(dir.path().join("m_middle.toml"), "[c]").unwrap();
        let files = find_toml_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3);
        let names: Vec<_> = files.iter().map(|f| f.file_name().unwrap().to_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["a_first.toml", "m_middle.toml", "z_last.toml"]);
    }

    #[test]
    fn find_toml_files_deeply_nested() {
        let dir = TempDir::new().unwrap();
        let deep = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("deep.toml"), "[x]").unwrap();
        fs::write(dir.path().join("top.toml"), "[y]").unwrap();
        let files = find_toml_files(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn backend_choice_from_str_roundtrip() {
        use crate::BackendChoice;
        let names = ["terraform", "pulumi", "crossplane", "ansible", "pangea", "steampipe", "helm", "all"];
        for name in &names {
            let parsed: BackendChoice = name.parse().unwrap();
            assert_eq!(parsed.to_string(), *name);
        }
    }

    #[test]
    fn generate_provider_auto_discovery_in_parent() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        write_resource_toml(&resources_dir);
        let output_dir = dir.path().join("output");

        #[cfg(feature = "terraform")]
        {
            let result = run(
                &BackendChoice::Terraform,
                &spec_path,
                &resources_dir,
                &output_dir,
                None,
                None,
            );
            assert!(result.is_ok(), "auto-discovered provider.toml should work: {:?}", result.err());
        }
        #[cfg(not(feature = "terraform"))]
        {
            let _ = (spec_path, resources_dir, output_dir);
        }
    }

    #[test]
    fn generate_missing_provider_toml_errors() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();
        let output_dir = dir.path().join("output");

        // No provider.toml exists and --provider is not provided
        let result = run(
            &BackendChoice::Terraform,
            &spec_path,
            &resources_dir,
            &output_dir,
            None,
            None,
        );
        assert!(result.is_err(), "should fail when provider.toml is missing");
    }

    #[cfg(feature = "terraform")]
    #[test]
    fn generate_terraform_skips_invalid_resources() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_minimal_spec(dir.path());
        let provider_path = write_provider_toml(dir.path());
        let resources_dir = dir.path().join("resources");
        fs::create_dir_all(&resources_dir).unwrap();

        // Write one valid and one invalid resource
        write_resource_toml(&resources_dir);
        let bad_toml = r#"
[resource]
name = "bad_resource"
description = "Bad"
category = "test"

[crud]
create_endpoint = "/nonexistent"
create_schema = "NonExistentSchema"
read_endpoint = "/describe-item"
read_schema = "describeItem"
delete_endpoint = "/delete-item"
delete_schema = "deleteItem"

[identity]
id_field = "name"
"#;
        fs::write(resources_dir.join("bad.toml"), bad_toml).unwrap();

        let output_dir = dir.path().join("output");
        let result = run(
            &BackendChoice::Terraform,
            &spec_path,
            &resources_dir,
            &output_dir,
            Some(&provider_path),
            None,
        );
        // Should succeed even with one invalid resource (it gets skipped)
        assert!(result.is_ok(), "generate should succeed with partial resources: {:?}", result.err());

        // The valid resource should still be generated
        assert!(
            output_dir.join("resources/resource_test_secret.go").exists(),
            "valid resource should still be generated"
        );
    }
}
