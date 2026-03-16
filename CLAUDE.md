# iac-forge-cli

Unified CLI for generating IaC providers from OpenAPI specs. Replaces `terraform-forge-cli`
with multi-backend support.

## Commands

```bash
# Generate for a specific backend
iac-forge generate --backend terraform --spec api.yaml --resources resources/ --output ./out/

# Generate for ALL backends at once
iac-forge generate --backend all --spec api.yaml --resources resources/ --output ./out/

# Auto-create resource TOML specs from OpenAPI analysis
iac-forge scaffold --spec api.yaml --output ./specs/

# Detect missing/extra resources vs OpenAPI spec
iac-forge drift --spec api.yaml --resources resources/

# Validate resource specs against OpenAPI spec
iac-forge validate --spec api.yaml --resources resources/

# Diff two OpenAPI spec versions
iac-forge diff --old old-api.yaml --new new-api.yaml
```

## Backend Support

Feature-gated via Cargo features:
- `terraform` (default) — Go code via `terraform-forge`
- `pulumi` — `schema.json` via `pulumi-forge`
- `crossplane` — CRD YAML via `crossplane-forge`
- `ansible` — Python modules via `ansible-forge`

## Dependencies

- `iac-forge` — core IR, resolver, spec types
- `openapi-forge` — OpenAPI 3.0 parsing
- Backend crates (optional features)
