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
