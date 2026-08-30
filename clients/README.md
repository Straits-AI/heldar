# Generated API clients

**Do not edit these by hand.** They are generated from the OpenAPI contract:

```bash
cargo test -p heldar-server --test openapi_contract write_the_served_document
python3 scripts/gen_clients.py target/openapi.json clients
```

CI regenerates them and fails if the committed files differ, so a contract change that is not
reflected here cannot merge.

## Why they exist

Not to ship an SDK. **A client that compiles is a stronger statement about the contract than any
prose.** A `$ref` to a schema that does not exist, an operation with no id, a duplicate
`operationId`, a field named after a language keyword — all become build failures here instead of a
surprise for whoever generates a client later.

Both defects the first run found were of exactly that kind:

- `ExportRequest` has a field literally named `from`, a keyword in two of the three languages.
- Three `operationId`s collided (`create`, `list`, `get_one` — utoipa derives them from function
  names, and `sites` and `evidence` both have all three). The document was **invalid**, TypeScript
  refused to compile it, and Python silently produced **13 methods for 14 operations**.

Neither was visible by reading the document.

## Scope

The contract currently covers 14 of 151 routes ([#120](https://github.com/Straits-AI/heldar/issues/120)),
so these clients cover 14 routes. They grow as the contract does.

The dashboard does **not** consume `typescript/heldar.ts` — its own `types.ts` covers routes the
contract has not reached yet, so swapping it wholesale would lose coverage. Instead a test asserts
the two agree wherever they overlap; it found `CameraView.priority`, which the server has always
returned and the dashboard had never modelled.

## Per language

| | build | notes |
|---|---|---|
| `typescript/heldar.ts` | `tsc --strict` | `fetch`-based, no dependencies |
| `python/heldar_client.py` | `import heldar_client` | stdlib only (`urllib`); the dataclasses **describe** wire shapes — the client returns parsed JSON |
| `rust/` | `cargo build` | `serde` only; standalone workspace |

Every endpoint returns the same error shape, so each client raises/throws one error type carrying
`code` and `retryable` — branch on `code`, never on the message.
