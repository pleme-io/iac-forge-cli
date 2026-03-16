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
