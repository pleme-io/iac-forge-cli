use std::fmt;
use std::path::Path;
use std::process::Command;

use colored::Colorize;

/// Result of validating a single Helm chart.
#[derive(Debug)]
pub struct ChartResult {
    /// Chart directory name (used as chart name).
    pub name: String,
    /// Whether `helm lint` passed.
    pub lint_passed: bool,
    /// Whether `helm template` rendered successfully.
    pub template_passed: bool,
    /// Error details if any check failed.
    pub errors: Vec<String>,
}

/// Aggregated result of validating all discovered Helm charts.
#[derive(Debug)]
pub struct ValidationReport {
    /// Per-chart results.
    pub charts: Vec<ChartResult>,
    /// True if helm binary was found on PATH.
    pub helm_available: bool,
}

impl ValidationReport {
    /// Returns `true` when every chart passed all checks (or no charts were found).
    #[must_use]
    pub fn all_passed(&self) -> bool {
        self.charts
            .iter()
            .all(|c| c.lint_passed && c.template_passed)
    }

    /// Returns a human-readable summary of failures (empty if all passed).
    #[must_use]
    pub fn failure_summary(&self) -> String {
        let failures: Vec<&ChartResult> = self
            .charts
            .iter()
            .filter(|c| !c.lint_passed || !c.template_passed)
            .collect();
        if failures.is_empty() {
            return String::new();
        }
        let mut lines = Vec::new();
        for f in &failures {
            let checks: Vec<&str> = [
                (!f.lint_passed).then_some("lint"),
                (!f.template_passed).then_some("template"),
            ]
            .into_iter()
            .flatten()
            .collect();
            lines.push(format!("  chart '{}': failed {}", f.name, checks.join(", ")));
            for err in &f.errors {
                lines.push(format!("    {err}"));
            }
        }
        format!(
            "{} of {} chart(s) failed validation:\n{}",
            failures.len(),
            self.charts.len(),
            lines.join("\n")
        )
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.helm_available {
            return write!(f, "helm not found on PATH, skipping chart validation");
        }
        if self.charts.is_empty() {
            return write!(f, "no helm charts found to validate");
        }
        if self.all_passed() {
            write!(
                f,
                "all {} helm chart(s) passed validation",
                self.charts.len()
            )
        } else {
            write!(f, "{}", self.failure_summary())
        }
    }
}

/// Check whether the `helm` binary is available on `PATH`.
fn helm_available() -> bool {
    Command::new("helm")
        .arg("version")
        .arg("--short")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Discover chart directories under `<output>/helm/charts/*/`.
fn discover_chart_dirs(output_dir: &Path) -> Vec<std::path::PathBuf> {
    let charts_root = output_dir.join("helm").join("charts");
    let Ok(entries) = std::fs::read_dir(&charts_root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// Run `helm lint <chart_dir>` and return `(passed, error_output)`.
fn run_helm_lint(chart_dir: &Path) -> (bool, String) {
    match Command::new("helm")
        .arg("lint")
        .arg(chart_dir)
        .output()
    {
        Ok(output) => {
            let passed = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let combined = if passed {
                String::new()
            } else {
                format!("{stdout}{stderr}")
            };
            (passed, combined)
        }
        Err(e) => (false, format!("failed to execute helm lint: {e}")),
    }
}

/// Run `helm template <chart_name> <chart_dir>` and return `(passed, error_output)`.
fn run_helm_template(chart_name: &str, chart_dir: &Path) -> (bool, String) {
    match Command::new("helm")
        .arg("template")
        .arg(chart_name)
        .arg(chart_dir)
        .output()
    {
        Ok(output) => {
            let passed = output.status.success();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let combined = if passed {
                String::new()
            } else {
                format!("{stdout}{stderr}")
            };
            (passed, combined)
        }
        Err(e) => (false, format!("failed to execute helm template: {e}")),
    }
}

/// Validate all Helm charts under `<output_dir>/helm/charts/*/`.
///
/// Runs `helm lint` and `helm template` on each discovered chart directory.
/// If the `helm` binary is not found on PATH, returns a report with
/// `helm_available = false` and no chart results (skip with warning).
///
/// # Errors
///
/// Returns `Err` with a summary string when one or more charts fail validation.
pub fn validate_helm_charts(
    output_dir: &Path,
) -> Result<ValidationReport, Box<dyn std::error::Error>> {
    if !helm_available() {
        println!(
            "  {} helm binary not found on PATH, skipping chart validation",
            "!".yellow()
        );
        return Ok(ValidationReport {
            charts: Vec::new(),
            helm_available: false,
        });
    }

    let chart_dirs = discover_chart_dirs(output_dir);
    if chart_dirs.is_empty() {
        println!(
            "  {} no helm charts found under {}/helm/charts/",
            "i".blue(),
            output_dir.display()
        );
        return Ok(ValidationReport {
            charts: Vec::new(),
            helm_available: true,
        });
    }

    let mut results = Vec::new();

    for chart_dir in &chart_dirs {
        let chart_name = chart_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        println!(
            "  {} validating chart '{chart_name}'...",
            "=>".blue().bold()
        );

        let mut errors = Vec::new();

        let (lint_passed, lint_err) = run_helm_lint(chart_dir);
        if lint_passed {
            println!("    {} helm lint passed", "ok".green());
        } else {
            println!("    {} helm lint failed", "FAIL".red().bold());
            errors.push(format!("lint: {lint_err}"));
        }

        let (template_passed, template_err) = run_helm_template(chart_name, chart_dir);
        if template_passed {
            println!("    {} helm template passed", "ok".green());
        } else {
            println!("    {} helm template failed", "FAIL".red().bold());
            errors.push(format!("template: {template_err}"));
        }

        results.push(ChartResult {
            name: chart_name.to_string(),
            lint_passed,
            template_passed,
            errors,
        });
    }

    let report = ValidationReport {
        charts: results,
        helm_available: true,
    };

    if report.all_passed() {
        Ok(report)
    } else {
        Err(report.failure_summary().into())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn report_all_passed_empty() {
        let report = ValidationReport {
            charts: Vec::new(),
            helm_available: true,
        };
        assert!(report.all_passed());
        assert!(report.failure_summary().is_empty());
    }

    #[test]
    fn report_all_passed_with_charts() {
        let report = ValidationReport {
            charts: vec![
                ChartResult {
                    name: "chart-a".to_string(),
                    lint_passed: true,
                    template_passed: true,
                    errors: Vec::new(),
                },
                ChartResult {
                    name: "chart-b".to_string(),
                    lint_passed: true,
                    template_passed: true,
                    errors: Vec::new(),
                },
            ],
            helm_available: true,
        };
        assert!(report.all_passed());
        assert!(report.failure_summary().is_empty());
    }

    #[test]
    fn report_detects_lint_failure() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "bad-chart".to_string(),
                lint_passed: false,
                template_passed: true,
                errors: vec!["lint: missing Chart.yaml".to_string()],
            }],
            helm_available: true,
        };
        assert!(!report.all_passed());
        let summary = report.failure_summary();
        assert!(summary.contains("bad-chart"));
        assert!(summary.contains("lint"));
        assert!(summary.contains("1 of 1"));
    }

    #[test]
    fn report_detects_template_failure() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "broken-tmpl".to_string(),
                lint_passed: true,
                template_passed: false,
                errors: vec!["template: render error".to_string()],
            }],
            helm_available: true,
        };
        assert!(!report.all_passed());
        let summary = report.failure_summary();
        assert!(summary.contains("broken-tmpl"));
        assert!(summary.contains("template"));
    }

    #[test]
    fn report_detects_both_failures() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "total-fail".to_string(),
                lint_passed: false,
                template_passed: false,
                errors: vec![
                    "lint: bad".to_string(),
                    "template: broken".to_string(),
                ],
            }],
            helm_available: true,
        };
        assert!(!report.all_passed());
        let summary = report.failure_summary();
        assert!(summary.contains("lint"));
        assert!(summary.contains("template"));
    }

    #[test]
    fn report_mixed_charts() {
        let report = ValidationReport {
            charts: vec![
                ChartResult {
                    name: "good".to_string(),
                    lint_passed: true,
                    template_passed: true,
                    errors: Vec::new(),
                },
                ChartResult {
                    name: "bad".to_string(),
                    lint_passed: false,
                    template_passed: true,
                    errors: vec!["lint: issue".to_string()],
                },
            ],
            helm_available: true,
        };
        assert!(!report.all_passed());
        let summary = report.failure_summary();
        assert!(summary.contains("1 of 2"));
        assert!(summary.contains("bad"));
        assert!(!summary.contains("good"));
    }

    #[test]
    fn display_helm_not_available() {
        let report = ValidationReport {
            charts: Vec::new(),
            helm_available: false,
        };
        let display = format!("{report}");
        assert!(display.contains("helm not found"));
    }

    #[test]
    fn display_no_charts() {
        let report = ValidationReport {
            charts: Vec::new(),
            helm_available: true,
        };
        let display = format!("{report}");
        assert!(display.contains("no helm charts found"));
    }

    #[test]
    fn display_all_passed() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "ok-chart".to_string(),
                lint_passed: true,
                template_passed: true,
                errors: Vec::new(),
            }],
            helm_available: true,
        };
        let display = format!("{report}");
        assert!(display.contains("passed validation"));
    }

    #[test]
    fn discover_chart_dirs_empty_dir() {
        let dir = TempDir::new().unwrap();
        let charts = discover_chart_dirs(dir.path());
        assert!(charts.is_empty());
    }

    #[test]
    fn discover_chart_dirs_finds_charts() {
        let dir = TempDir::new().unwrap();
        let charts_root = dir.path().join("helm").join("charts");
        fs::create_dir_all(charts_root.join("my-app")).unwrap();
        fs::create_dir_all(charts_root.join("my-lib")).unwrap();
        // Also create a file that should be ignored
        fs::write(charts_root.join("README.md"), "ignored").unwrap();

        let charts = discover_chart_dirs(dir.path());
        assert_eq!(charts.len(), 2);
        let names: Vec<String> = charts
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(String::from))
            .collect();
        assert!(names.contains(&"my-app".to_string()));
        assert!(names.contains(&"my-lib".to_string()));
    }

    #[test]
    fn discover_chart_dirs_nonexistent() {
        let charts = discover_chart_dirs(Path::new("/nonexistent/output"));
        assert!(charts.is_empty());
    }

    #[test]
    fn validate_no_charts_returns_ok() {
        let dir = TempDir::new().unwrap();
        // No helm/charts subdirectory at all
        let result = validate_helm_charts(dir.path());
        assert!(result.is_ok());
        let report = result.unwrap();
        assert!(report.all_passed());
    }

    #[test]
    fn display_shows_failure_details() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "broken".to_string(),
                lint_passed: false,
                template_passed: false,
                errors: vec!["lint: missing".to_string(), "template: bad".to_string()],
            }],
            helm_available: true,
        };
        let display = format!("{report}");
        assert!(display.contains("broken"), "should mention chart name");
        assert!(display.contains("lint"), "should mention lint failure");
        assert!(display.contains("template"), "should mention template failure");
    }

    #[test]
    fn all_passed_single_chart_both_pass() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "good".to_string(),
                lint_passed: true,
                template_passed: true,
                errors: Vec::new(),
            }],
            helm_available: true,
        };
        assert!(report.all_passed());
    }

    #[test]
    fn all_passed_false_when_lint_fails() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "chart".to_string(),
                lint_passed: false,
                template_passed: true,
                errors: Vec::new(),
            }],
            helm_available: true,
        };
        assert!(!report.all_passed());
    }

    #[test]
    fn all_passed_false_when_template_fails() {
        let report = ValidationReport {
            charts: vec![ChartResult {
                name: "chart".to_string(),
                lint_passed: true,
                template_passed: false,
                errors: Vec::new(),
            }],
            helm_available: true,
        };
        assert!(!report.all_passed());
    }

    #[test]
    fn failure_summary_multiple_failures() {
        let report = ValidationReport {
            charts: vec![
                ChartResult {
                    name: "a".to_string(),
                    lint_passed: false,
                    template_passed: true,
                    errors: vec!["lint: issue-a".to_string()],
                },
                ChartResult {
                    name: "b".to_string(),
                    lint_passed: true,
                    template_passed: false,
                    errors: vec!["template: issue-b".to_string()],
                },
            ],
            helm_available: true,
        };
        let summary = report.failure_summary();
        assert!(summary.contains("2 of 2"));
        assert!(summary.contains("issue-a"));
        assert!(summary.contains("issue-b"));
    }

    #[test]
    fn discover_chart_dirs_ignores_files() {
        let dir = TempDir::new().unwrap();
        let charts_root = dir.path().join("helm").join("charts");
        fs::create_dir_all(&charts_root).unwrap();
        fs::write(charts_root.join("not-a-dir.txt"), "hello").unwrap();
        fs::create_dir_all(charts_root.join("real-chart")).unwrap();

        let charts = discover_chart_dirs(dir.path());
        assert_eq!(charts.len(), 1);
        assert!(charts[0].ends_with("real-chart"));
    }

    #[test]
    fn chart_result_errors_empty_when_passing() {
        let result = ChartResult {
            name: "pass".to_string(),
            lint_passed: true,
            template_passed: true,
            errors: Vec::new(),
        };
        assert!(result.errors.is_empty());
        assert!(result.lint_passed);
        assert!(result.template_passed);
    }

    #[test]
    fn failure_summary_format() {
        let report = ValidationReport {
            charts: vec![
                ChartResult {
                    name: "alpha".to_string(),
                    lint_passed: false,
                    template_passed: false,
                    errors: vec!["lint: x".to_string(), "template: y".to_string()],
                },
                ChartResult {
                    name: "beta".to_string(),
                    lint_passed: true,
                    template_passed: true,
                    errors: Vec::new(),
                },
            ],
            helm_available: true,
        };
        let summary = report.failure_summary();
        // Should mention 1 of 2 failed
        assert!(summary.contains("1 of 2"));
        // Should list both error details
        assert!(summary.contains("lint: x"));
        assert!(summary.contains("template: y"));
    }
}
