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
