use std::fs;
use std::path::Path;

use colored::Colorize;
use openapi_forge::Spec;

pub fn run(
    spec_path: &Path,
    pattern: Option<&str>,
    output_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{} Loading OpenAPI spec from {}",
        "=>".blue().bold(),
        spec_path.display()
    );
    let api = Spec::load(spec_path)?;

    let groups = api.group_by_crud_pattern();
    println!("{} Found {} CRUD groups", "=>".blue().bold(), groups.len());

    fs::create_dir_all(output_dir)?;

    let mut count = 0;
    for group in &groups {
        // Filter by pattern if provided
        if let Some(pat) = pattern {
            let pat_normalized = pat.replace('*', "");
            if !group.base_name.contains(&pat_normalized) {
                continue;
            }
        }

        // Only scaffold groups with at least create + delete
        if group.create.is_none() || group.delete.is_none() {
            continue;
        }

        let resource_name = format!("akeyless_{}", group.base_name.replace('-', "_"));
        let file_name = format!("{}.toml", group.base_name.replace('-', "_"));

        let create_ep = group.create.as_ref().unwrap();
        let delete_ep = group.delete.as_ref().unwrap();

        let mut toml = String::new();
        toml.push_str(&format!(
            r#"[resource]
name = "{resource_name}"
description = ""
category = ""

[crud]
create_endpoint = "{}"
create_schema = "{}"
"#,
            create_ep.path,
            create_ep.request_schema_ref.as_deref().unwrap_or("TODO"),
        ));

        if let Some(ref update) = group.update {
            toml.push_str(&format!(
                "update_endpoint = \"{}\"\nupdate_schema = \"{}\"\n",
                update.path,
                update.request_schema_ref.as_deref().unwrap_or("TODO"),
            ));
        }

        if let Some(ref read) = group.read {
            toml.push_str(&format!(
                "read_endpoint = \"{}\"\nread_schema = \"{}\"\n",
                read.path,
                read.request_schema_ref.as_deref().unwrap_or("TODO"),
            ));
        } else {
            toml.push_str("read_endpoint = \"TODO\"\nread_schema = \"TODO\"\n");
        }

        toml.push_str(&format!(
            "delete_endpoint = \"{}\"\ndelete_schema = \"{}\"\n",
            delete_ep.path,
            delete_ep.request_schema_ref.as_deref().unwrap_or("TODO"),
        ));

        toml.push_str(
            r#"
[identity]
id_field = "name"
force_new_fields = ["name"]

[fields]
token = { skip = true }
uid_token = { skip = true }
json = { skip = true }
"#,
        );

        let out_path = output_dir.join(&file_name);
        fs::write(&out_path, &toml)?;
        println!("  {} {}", "->".green(), file_name);
        count += 1;
    }

    println!(
        "\n{} Scaffolded {} resource specs in {}",
        "done".green().bold(),
        count,
        output_dir.display()
    );

    Ok(())
}
