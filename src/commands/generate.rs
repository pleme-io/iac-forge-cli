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
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    match backend {
        BackendChoice::Terraform => {
            #[cfg(feature = "terraform")]
            {
                generate_terraform(api, provider_spec, iac_provider, iac_resources, output_dir)
            }
            #[cfg(not(feature = "terraform"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, output_dir);
                Err("terraform backend not compiled in — enable the 'terraform' feature".into())
            }
        }
        BackendChoice::Pulumi => {
            #[cfg(feature = "pulumi")]
            {
                let _ = (api, provider_spec);
                generate_via_backend(
                    &pulumi_forge::PulumiBackend::new(),
                    iac_provider,
                    iac_resources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "pulumi"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, output_dir);
                Err("pulumi backend not yet available — pulumi-forge crate pending".into())
            }
        }
        BackendChoice::Crossplane => {
            #[cfg(feature = "crossplane")]
            {
                let _ = (api, provider_spec);
                generate_via_backend(
                    &crossplane_forge::CrossplaneBackend,
                    iac_provider,
                    iac_resources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "crossplane"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, output_dir);
                Err("crossplane backend not yet available — crossplane-forge crate pending".into())
            }
        }
        BackendChoice::Ansible => {
            #[cfg(feature = "ansible")]
            {
                let _ = (api, provider_spec);
                generate_via_backend(
                    &ansible_forge::AnsibleBackend::new(),
                    iac_provider,
                    iac_resources,
                    output_dir,
                )
            }
            #[cfg(not(feature = "ansible"))]
            {
                let _ = (api, provider_spec, iac_provider, iac_resources, output_dir);
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
#[cfg(feature = "terraform")]
fn generate_terraform(
    api: &Spec,
    provider_spec: &ProviderSpec,
    _iac_provider: &iac_forge::IacProvider,
    _iac_resources: &[iac_forge::IacResource],
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

    let resources_out = output_dir.join("resources");
    let provider_out = output_dir.join("provider");
    fs::create_dir_all(&resources_out)?;
    fs::create_dir_all(&provider_out)?;

    let type_names: Vec<String> = Vec::new();
    let tf_names: Vec<String> = Vec::new();

    // Generate provider.go
    println!("  {} Generating provider.go", "->".green(),);
    let data_source_names: Vec<String> = Vec::new();
    let provider_code =
        terraform_forge::generate_provider(&tf_provider, &type_names, &tf_names, &data_source_names);
    fs::write(provider_out.join("provider.go"), &provider_code)?;

    // Generate common helpers
    fs::write(resources_out.join("helpers.go"), helpers_template::HELPERS_GO)?;

    println!(
        "  {} Generated {} resources + provider.go",
        "=>".blue().bold(),
        type_names.len(),
    );

    // Suppress unused variable warnings in the transitional code
    let _ = api;

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

    #[test]
    fn backend_choice_display() {
        assert_eq!(BackendChoice::Terraform.to_string(), "terraform");
        assert_eq!(BackendChoice::Pulumi.to_string(), "pulumi");
        assert_eq!(BackendChoice::Crossplane.to_string(), "crossplane");
        assert_eq!(BackendChoice::Ansible.to_string(), "ansible");
        assert_eq!(BackendChoice::All.to_string(), "all");
    }

    #[test]
    fn find_toml_files_nonexistent_dir() {
        let result = find_toml_files(Path::new("/nonexistent"));
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
