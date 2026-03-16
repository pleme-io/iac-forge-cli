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
- `pangea` — Ruby DSL via `pangea-forge`
- `steampipe` — Go Steampipe plugin tables via `steampipe-forge`

## Sync Command

The `sync` command runs the full API evolution pipeline in a single invocation.
Use it when an upstream OpenAPI spec changes to automatically update all
generated IaC code.

### Pipeline Steps

1. **Diff** -- compare old vs new spec (endpoints added/removed, schemas added/removed)
2. **Drift** -- detect missing/extra resources against the new spec
3. **Scaffold** -- auto-create TOML specs for new endpoints (if `--auto-scaffold`)
4. **Validate** -- check all resource specs against the new spec (warnings, non-fatal)
5. **Generate** -- produce backend artifacts from all resource specs
6. **Summary** -- report endpoints/schemas/files changed

### Usage

```bash
iac-forge sync \
  --spec-old old-api.yaml \
  --spec-new new-api.yaml \
  --resources resources/ \
  --output ./out/ \
  --provider provider.toml \
  --auto-scaffold \
  --backend all \
  --audit-log ./audit.jsonl
```

### Audit Log

When `--audit-log <path>` is provided, a `sync_complete` event is appended
in JSONL format with:

```json
{
  "timestamp": "...",
  "event": "sync_complete",
  "data": {
    "old_spec": "...",
    "new_spec": "...",
    "endpoints_added": 5,
    "endpoints_removed": 1,
    "schemas_added": 3,
    "schemas_removed": 0,
    "added_endpoints": ["/new-endpoint", ...],
    "removed_endpoints": ["/old-endpoint"],
    "backend": "all",
    "files_generated": 42,
    "auto_scaffold": true
  }
}
```

This integrates with `tend watch` file-watch post-hooks to create a full
traceability chain: spec change detected -> sync executed -> audit logged.

## Dependencies

- `iac-forge` -- core IR, resolver, spec types
- `openapi-forge` -- OpenAPI 3.0 parsing
- Backend crates (optional features)
