# Example plugin registry

A signed catalog you can serve to populate the **Community** and **Proprietary** shelves of the Heldar
Plugin Store. It demonstrates the registry format and the signature trust model end to end.

- `example-catalog.json` — the catalog document (`heldar-catalog/v1`).
- `example-catalog.json.sig` — the detached Ed25519 signature over the exact catalog bytes, produced by
  [`scripts/sign-catalog.sh`](../../scripts/sign-catalog.sh).

The catalog is signed by an **example key** (`key_id: example-registry-key`). Its public key is:

```
example-registry-key:ADIHkj3Z0gm/o04gofyC8xO2WPsrCx3kMaoDL9ZBQX8=
```

## Serve + pin it

```bash
# 1. serve this directory over HTTP (the .sig must sit next to the catalog)
cd examples/registry && python3 -m http.server 9400

# 2. run Heldar Core pointed at it, pinning the example key
HELDAR_REGISTRY_URLS=http://127.0.0.1:9400/example-catalog.json \
HELDAR_REGISTRY_TRUSTED_KEYS=example-registry-key:ADIHkj3Z0gm/o04gofyC8xO2WPsrCx3kMaoDL9ZBQX8= \
HELDAR_REGISTRY_ALLOW_PRIVATE=true \
  heldar-core
```

Open the dashboard → **Plugins**. The **Community** shelf now shows *Weather Overlay* and the
**Proprietary** shelf shows *Advanced Analytics*, both badged **Verified** (the catalog signature
checked out against the pinned key). Tamper with `example-catalog.json` and the source drops to
unverified with zero entries — fail-closed.

`HELDAR_REGISTRY_ALLOW_PRIVATE=true` is only needed here because the demo registry is on loopback over
HTTP; a real registry is served over HTTPS from a public host and needs neither flag.

## Publish your own

1. `openssl genpkey -algorithm ed25519 -out registry.pem` (keep the private key in a secret store).
2. Pin the public key: `openssl pkey -in registry.pem -pubout -outform DER | tail -c 32 | base64`,
   then set `HELDAR_REGISTRY_TRUSTED_KEYS=my-key:<that-base64>` (or add it to `TRUSTED_KEYS` in a fork).
3. Author `catalog.json`, then `./scripts/sign-catalog.sh catalog.json registry.pem my-key`.
4. Host both files (HTTPS) and set `HELDAR_REGISTRY_URLS` to the catalog URL.

See [the registry guide](../../website/docs/develop/registry.md) for the full format.
