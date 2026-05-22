# OpenAPI Spec

The gateway publishes an OpenAPI 3.0 specification that describes every REST endpoint, request schema, response schema, and security requirement.

## Viewing the Spec

The generated spec is committed to the repository at `docs/src/api/openapi.json`. It is regenerated automatically by `cargo xtask docs`.

To browse it interactively, run a local Swagger UI:

```bash
# Using Docker
docker run -p 8888:8080 \
  -e SWAGGER_JSON_URL=http://localhost:3000/api/openapi.json \
  swaggerapi/swagger-ui

# Then open: http://localhost:8888
```

Or use the [Swagger Editor](https://editor.swagger.io/) and paste the JSON.

## Interactive Swagger UI (in this book)

<div id="swagger-ui"></div>

<link rel="stylesheet" type="text/css" href="https://unpkg.com/swagger-ui-dist@5/swagger-ui.css">
<script src="https://unpkg.com/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
<script>
window.addEventListener('load', function() {
  SwaggerUIBundle({
    url: "../api/openapi.json",
    dom_id: '#swagger-ui',
    presets: [
      SwaggerUIBundle.presets.apis,
      SwaggerUIBundle.SwaggerUIStandalonePreset
    ],
    layout: "BaseLayout",
    deepLinking: true,
    tryItOutEnabled: false
  });
});
</script>

> **Note:** The interactive spec above renders the generated `openapi.json` directly inside the mdBook.

## Regenerating the Spec

```bash
cargo xtask docs
```

This command:

1. Compiles `stitchd-gateway` in debug mode
2. Runs `stitchd-gateway --export-openapi docs/src/api/openapi.json`
3. The binary serialises the `utoipa`-derived `ApiDoc` struct to JSON and exits

The spec is derived from `#[utoipa::path]` annotations on every route handler in `crates/stitchd-gateway/src/routes/`. To add a new endpoint to the spec:

1. Annotate the handler with `#[utoipa::path(...)]`
2. Add the handler path to `ApiDoc` in `crates/stitchd-gateway/src/openapi.rs`
3. Register any new request/response types in the `components(schemas(...))` block
4. Run `cargo xtask docs` to regenerate

## Contract Checking

The repository includes a contract-check script that verifies the spec covers all registered gateway routes:

```bash
python3 scripts/check_openapi_contract.py
```

This script:
- Parses `crates/stitchd-gateway/src/routes/` for registered Axum routes
- Reads `docs/src/api/openapi.json`
- Reports any route without a matching OpenAPI path entry

Run this in CI to catch annotation gaps before they reach main.

## Security Schemes

The spec defines two security schemes:

| Scheme | Type | Header |
|--------|------|--------|
| `sdk_key` | API Key | `x-sdk-key` |
| `bearer_jwt` | HTTP Bearer | `Authorization: Bearer <jwt>` |

Every endpoint declares which scheme it uses via the `security` field. Endpoints that require no auth (e.g., login) declare an empty security array.
