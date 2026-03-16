use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;

mod commands;
#[cfg(feature = "terraform")]
mod helpers_template;

#[derive(Debug, Clone, ValueEnum)]
pub enum BackendChoice {
    Terraform,
    Pulumi,
    Crossplane,
    Ansible,
    All,
}

impl std::fmt::Display for BackendChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terraform => write!(f, "terraform"),
            Self::Pulumi => write!(f, "pulumi"),
            Self::Crossplane => write!(f, "crossplane"),
            Self::Ansible => write!(f, "ansible"),
            Self::All => write!(f, "all"),
        }
    }
}

impl std::str::FromStr for BackendChoice {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "terraform" => Ok(Self::Terraform),
            "pulumi" => Ok(Self::Pulumi),
            "crossplane" => Ok(Self::Crossplane),
            "ansible" => Ok(Self::Ansible),
            "all" => Ok(Self::All),
            other => Err(format!("unknown backend: {other}")),
        }
    }
}

#[derive(Parser)]
#[command(
    name = "iac-forge",
    about = "Unified CLI for generating IaC providers from OpenAPI specs",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate IaC provider code from resource specs
    Generate {
        /// Target backend platform
        #[arg(long, value_enum)]
        backend: BackendChoice,

        /// Path to OpenAPI spec (YAML or JSON)
        #[arg(long)]
        spec: PathBuf,

        /// Directory containing resource TOML specs
        #[arg(long)]
        resources: PathBuf,

        /// Output directory for generated files
        #[arg(long)]
        output: PathBuf,

        /// Path to provider.toml
        #[arg(long)]
        provider: Option<PathBuf>,
    },

    /// Auto-create resource spec TOMLs from OpenAPI analysis
    Scaffold {
        /// Path to OpenAPI spec
        #[arg(long)]
        spec: PathBuf,

        /// Operation ID pattern to match (e.g. "auth-method-*")
        #[arg(long)]
        pattern: Option<String>,

        /// Output directory for generated TOML files
        #[arg(long)]
        output: PathBuf,
    },

    /// Compare resource specs against OpenAPI spec, flag missing/changed
    Drift {
        /// Path to OpenAPI spec
        #[arg(long)]
        spec: PathBuf,

        /// Directory containing resource TOML specs
        #[arg(long)]
        resources: PathBuf,
    },

    /// Validate resource specs against OpenAPI spec
    Validate {
        /// Path to OpenAPI spec
        #[arg(long)]
        spec: PathBuf,

        /// Directory containing resource TOML specs
        #[arg(long)]
        resources: PathBuf,
    },

    /// Diff two OpenAPI spec versions
    Diff {
        /// Path to old spec
        #[arg(long)]
        old: PathBuf,

        /// Path to new spec
        #[arg(long)]
        new: PathBuf,
    },

    /// Sync resources across API version change (diff + scaffold + generate)
    Sync {
        /// Path to old OpenAPI spec
        #[arg(long)]
        spec_old: PathBuf,

        /// Path to new OpenAPI spec
        #[arg(long)]
        spec_new: PathBuf,

        /// Directory containing resource TOML specs
        #[arg(long)]
        resources: PathBuf,

        /// Output directory for generated files
        #[arg(long)]
        output: PathBuf,

        /// Path to provider.toml
        #[arg(long)]
        provider: Option<PathBuf>,

        /// Auto-scaffold new resources found in diff
        #[arg(long, default_value = "false")]
        auto_scaffold: bool,

        /// Backend to generate (terraform, pulumi, crossplane, ansible, all)
        #[arg(long, default_value = "all")]
        backend: String,

        /// Path to structured JSON audit log (JSONL). Events appended for traceability.
        #[arg(long)]
        audit_log: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Generate {
            backend,
            spec,
            resources,
            output,
            provider,
        } => commands::generate::run(&backend, &spec, &resources, &output, provider.as_deref()),
        Commands::Scaffold {
            spec,
            pattern,
            output,
        } => commands::scaffold::run(&spec, pattern.as_deref(), &output),
        Commands::Drift { spec, resources } => commands::drift::run(&spec, &resources),
        Commands::Validate { spec, resources } => commands::validate::run(&spec, &resources),
        Commands::Diff { old, new } => commands::diff::run(&old, &new),
        Commands::Sync {
            spec_old,
            spec_new,
            resources,
            output,
            provider,
            auto_scaffold,
            backend,
            audit_log,
        } => commands::sync::run_with_audit(
            &spec_old,
            &spec_new,
            &resources,
            &output,
            provider.as_deref(),
            auto_scaffold,
            &backend,
            audit_log.as_deref(),
        ),
    };

    if let Err(e) = result {
        eprintln!("{} {e}", "error:".red().bold());
        process::exit(1);
    }
}
