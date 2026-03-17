use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use colored::Colorize;

/// Build the JSON payload for a failure notification.
///
/// Returns a serialized JSON string with fields: event, timestamp, summary, source.
#[must_use]
pub fn build_failure_payload(summary: &str) -> String {
    let payload = serde_json::json!({
        "event": "iac_forge_failure",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "summary": summary,
        "source": "iac-forge-cli",
    });
    // This should never fail for valid JSON values.
    serde_json::to_string(&payload).unwrap_or_else(|_| String::from("{}"))
}

/// Send a failure notification to a webhook URL via `curl`.
///
/// Uses `std::process::Command` to invoke `curl` so no HTTP client dependency
/// is required. If the notification itself fails (curl not found, network error,
/// non-2xx response), a warning is printed but the error is not propagated.
pub fn notify_failure(webhook_url: &str, summary: &str) {
    let payload = build_failure_payload(summary);

    println!(
        "  {} sending failure notification to webhook...",
        "=>".blue().bold()
    );

    match Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
            &payload,
            "--max-time",
            "10",
            webhook_url,
        ])
        .output()
    {
        Ok(output) => {
            let status_code = String::from_utf8_lossy(&output.stdout);
            if output.status.success()
                && status_code.starts_with('2')
            {
                println!("    {} notification sent (HTTP {status_code})", "ok".green());
            } else {
                eprintln!(
                    "  {} webhook notification returned HTTP {status_code}",
                    "!".yellow()
                );
            }
        }
        Err(e) => {
            eprintln!(
                "  {} failed to send webhook notification: {e} (curl not found?)",
                "!".yellow()
            );
        }
    }
}

/// Append a failure line to a notification log file.
///
/// Each line is a JSON object with event, timestamp, and summary fields.
/// If the write fails, a warning is printed but the error is not propagated.
pub fn notify_log(log_path: &Path, summary: &str) {
    let payload = build_failure_payload(summary);

    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    match OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(mut file) => {
            if writeln!(file, "{payload}").is_ok() {
                println!(
                    "    {} failure logged to {}",
                    "ok".green(),
                    log_path.display()
                );
            } else {
                eprintln!(
                    "  {} failed to write to notify log {}",
                    "!".yellow(),
                    log_path.display()
                );
            }
        }
        Err(e) => {
            eprintln!(
                "  {} failed to open notify log {}: {e}",
                "!".yellow(),
                log_path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn payload_has_required_fields() {
        let json_str = build_failure_payload("something broke");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed["event"], "iac_forge_failure");
        assert_eq!(parsed["summary"], "something broke");
        assert_eq!(parsed["source"], "iac-forge-cli");
        assert!(parsed["timestamp"].is_string());
    }

    #[test]
    fn payload_timestamp_is_rfc3339() {
        let json_str = build_failure_payload("test");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        let ts = parsed["timestamp"].as_str().unwrap();
        // Should parse as a valid RFC 3339 timestamp
        assert!(
            chrono::DateTime::parse_from_rfc3339(ts).is_ok(),
            "timestamp should be valid RFC 3339: {ts}"
        );
    }

    #[test]
    fn payload_is_valid_json() {
        let json_str = build_failure_payload("line1\nline2\t\"quoted\"");
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&json_str);
        assert!(parsed.is_ok(), "payload should be valid JSON even with special chars");
    }

    #[test]
    fn payload_empty_summary() {
        let json_str = build_failure_payload("");
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["summary"], "");
        assert_eq!(parsed["event"], "iac_forge_failure");
    }

    #[test]
    fn notify_log_creates_file() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("notify.jsonl");

        notify_log(&log_path, "test failure");

        assert!(log_path.exists());
        let content = fs::read_to_string(&log_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["event"], "iac_forge_failure");
        assert_eq!(parsed["summary"], "test failure");
    }

    #[test]
    fn notify_log_appends() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("notify.jsonl");

        notify_log(&log_path, "failure one");
        notify_log(&log_path, "failure two");

        let content = fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["summary"], "failure one");
        assert_eq!(second["summary"], "failure two");
    }

    #[test]
    fn notify_log_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let log_path = dir.path().join("nested").join("deep").join("notify.jsonl");

        notify_log(&log_path, "nested test");

        assert!(log_path.exists());
        let content = fs::read_to_string(&log_path).unwrap();
        assert!(content.contains("nested test"));
    }

    #[test]
    fn payload_special_characters() {
        let summary = r#"chart 'my-app': lint failed with "unexpected token""#;
        let json_str = build_failure_payload(summary);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["summary"].as_str().unwrap(), summary);
    }
}
