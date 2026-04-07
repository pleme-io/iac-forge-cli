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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_spec_with_crud(dir: &Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: Test, version: "1.0" }
paths:
  /create-auth-method:
    post:
      operationId: createAuthMethod
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/createAuthMethod'
      responses:
        "200": { description: ok }
  /update-auth-method:
    post:
      operationId: updateAuthMethod
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/updateAuthMethod'
      responses:
        "200": { description: ok }
  /get-auth-method:
    post:
      operationId: getAuthMethod
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/getAuthMethod'
      responses:
        "200": { description: ok }
  /delete-auth-method:
    post:
      operationId: deleteAuthMethod
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/deleteAuthMethod'
      responses:
        "200": { description: ok }
components:
  schemas:
    createAuthMethod:
      type: object
      required: [name]
      properties:
        name: { type: string }
        token: { type: string }
    updateAuthMethod:
      type: object
      properties:
        name: { type: string }
    getAuthMethod:
      type: object
      properties:
        name: { type: string }
    deleteAuthMethod:
      type: object
      properties:
        name: { type: string }
"#;
        let path = dir.join("spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    fn write_no_crud_spec(dir: &Path) -> std::path::PathBuf {
        let spec = r#"
openapi: "3.0.0"
info: { title: Test, version: "1.0" }
paths:
  /list-items:
    post:
      operationId: listItems
      responses:
        "200": { description: ok }
  /describe-item:
    post:
      operationId: describeItem
      responses:
        "200": { description: ok }
components:
  schemas: {}
"#;
        let path = dir.join("spec.yaml");
        fs::write(&path, spec).unwrap();
        path
    }

    #[test]
    fn scaffold_creates_output_dir() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("new_dir");
        assert!(!output_dir.exists());
        let result = run(&spec_path, None, &output_dir);
        assert!(result.is_ok());
        assert!(output_dir.exists());
    }

    #[test]
    fn scaffold_produces_valid_toml() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, None, &output_dir).unwrap();

        for entry in glob::glob(&format!("{}/**/*.toml", output_dir.display())).unwrap() {
            let path = entry.unwrap();
            let content = fs::read_to_string(&path).unwrap();
            let parsed: Result<toml::Value, _> = toml::from_str(&content);
            assert!(parsed.is_ok(), "Invalid TOML at {}: {:?}", path.display(), parsed.err());
        }
    }

    #[test]
    fn scaffold_toml_has_required_sections() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, None, &output_dir).unwrap();

        let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!files.is_empty(), "should produce at least one TOML file");
        for path in &files {
            let content = fs::read_to_string(path).unwrap();
            assert!(content.contains("[resource]"), "missing [resource]");
            assert!(content.contains("[crud]"), "missing [crud]");
            assert!(content.contains("[identity]"), "missing [identity]");
            assert!(content.contains("[fields]"), "missing [fields]");
        }
    }

    #[test]
    fn scaffold_with_pattern_filters_results() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, Some("auth-method*"), &output_dir).unwrap();

        let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        for path in &files {
            let name = path.file_stem().unwrap().to_str().unwrap();
            assert!(
                name.contains("auth_method"),
                "pattern filter should only include matching files, got: {name}"
            );
        }
    }

    #[test]
    fn scaffold_with_non_matching_pattern_produces_no_files() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, Some("nonexistent-pattern"), &output_dir).unwrap();

        let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(files.is_empty(), "non-matching pattern should produce no files");
    }

    #[test]
    fn scaffold_no_crud_groups_produces_no_files() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_no_crud_spec(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, None, &output_dir).unwrap();

        let files: Vec<_> = glob::glob(&format!("{}/**/*.toml", output_dir.display()))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(files.is_empty(), "no CRUD groups => no TOML files");
    }

    #[test]
    fn scaffold_invalid_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("bad.yaml");
        fs::write(&spec, "invalid yaml content ][").unwrap();
        let output_dir = dir.path().join("out");
        assert!(run(&spec, None, &output_dir).is_err());
    }

    #[test]
    fn scaffold_nonexistent_spec_returns_error() {
        let dir = TempDir::new().unwrap();
        let spec = dir.path().join("missing.yaml");
        let output_dir = dir.path().join("out");
        assert!(run(&spec, None, &output_dir).is_err());
    }

    #[test]
    fn scaffold_resource_name_has_akeyless_prefix() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, None, &output_dir).unwrap();

        for entry in glob::glob(&format!("{}/**/*.toml", output_dir.display())).unwrap() {
            let path = entry.unwrap();
            let content = fs::read_to_string(&path).unwrap();
            let parsed: toml::Value = toml::from_str(&content).unwrap();
            let name = parsed["resource"]["name"].as_str().unwrap();
            assert!(
                name.starts_with("akeyless_"),
                "resource name should start with akeyless_ prefix, got: {name}"
            );
        }
    }

    #[test]
    fn scaffold_skip_fields_present() {
        let dir = TempDir::new().unwrap();
        let spec_path = write_spec_with_crud(dir.path());
        let output_dir = dir.path().join("out");
        run(&spec_path, None, &output_dir).unwrap();

        for entry in glob::glob(&format!("{}/**/*.toml", output_dir.display())).unwrap() {
            let path = entry.unwrap();
            let content = fs::read_to_string(&path).unwrap();
            assert!(
                content.contains("token = { skip = true }"),
                "should include token skip field"
            );
        }
    }
}
