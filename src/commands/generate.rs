use std::fs;
use std::path::Path;

use colored::Colorize;
use iac_forge::{Backend, ProviderSpec, ResourceSpec, resolve_provider, resolve_resource};
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

    // Dispatch to the appropriate backend(s)
    let backends = match backend {
        BackendChoice::Terraform => vec![BackendChoice::Terraform],
        BackendChoice::Pulumi => vec![BackendChoice::Pulumi],
        BackendChoice::Crossplane => vec![BackendChoice::Crossplane],
        BackendChoice::Ansible => vec![BackendChoice::Ansible],
        BackendChoice::All => {
            let mut v = Vec::new();
            v.push(BackendChoice::Terraform);
            v.push(BackendChoice::Pulumi);
            v.push(BackendChoice::Crossplane);
            v.push(BackendChoice::Ansible);
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
            &provider_spec,
            &iac_provider,
            &iac_resources,
            &resource_files,
            &target_dir,
        )?;
    }

    println!(
        "\n{} Generated artifacts for {} resource(s) with backend: {}",
        "done".green().bold(),
        iac_resources.len(),
        backend,
    );

    Ok(())
}

fn generate_for_backend(
    backend: &BackendChoice,
    api: &Spec,
    provider_spec: &ProviderSpec,
    iac_provider: &iac_forge::IacProvider,
    iac_resources: &[iac_forge::IacResource],
    resource_files: &[std::path::PathBuf],
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match backend {
        BackendChoice::Terraform => {
            #[cfg(feature = "terraform")]
            {
                generate_terraform(api, provider_spec, iac_provider, iac_resources, resource_files, output_dir)
            }
            #[cfg(not(feature = "terraform"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, resource_files, output_dir);
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
                    output_dir,
                )
            }
            #[cfg(not(feature = "pulumi"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, resource_files, output_dir);
                Err("pulumi backend not yet available — pulumi-forge crate pending".into())
            }
        }
        BackendChoice::Crossplane => {
            #[cfg(feature = "crossplane")]
            {
                let _ = (api, provider_spec, resource_files);
                generate_via_backend(
                    &crossplane_forge::CrossplaneBackend,
                    iac_provider,
                    iac_resources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "crossplane"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, resource_files, output_dir);
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
                    output_dir,
                )
            }
            #[cfg(not(feature = "ansible"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, resource_files, output_dir);
                Err("ansible backend not yet available — ansible-forge crate pending".into())
            }
        }
        BackendChoice::All => unreachable!("All is expanded before calling this function"),
    }
}

/// Generate using the iac-forge Backend trait (for Pulumi, Crossplane, Ansible).
#[allow(dead_code)]
fn generate_via_backend(
    backend: &dyn Backend,
    iac_provider: &iac_forge::IacProvider,
    iac_resources: &[iac_forge::IacResource],
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

    // Generate provider-level artifacts
    let data_sources = Vec::new();
    let provider_artifacts =
        backend.generate_provider(iac_provider, iac_resources, &data_sources)?;
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
    let data_source_names: Vec<String> = Vec::new();
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
                v
            }
            _ => unreachable!(),
        };
        assert_eq!(all_backends.len(), 4);
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Terraform)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Pulumi)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Crossplane)));
        assert!(all_backends.iter().any(|b| matches!(b, BackendChoice::Ansible)));
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
