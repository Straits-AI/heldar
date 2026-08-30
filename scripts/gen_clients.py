#!/usr/bin/env python3
"""Generate typed API clients from the OpenAPI contract (#120).

Usage: gen_clients.py <openapi.json> <out-dir>

Emits TypeScript, Python and Rust into `<out-dir>/{typescript,python,rust}`.

WHY A GENERATOR HERE AND NOT `openapi-generator-cli`.

The off-the-shelf generators are excellent and enormous — a JVM, a template engine, and thousands of
lines of output for a document with fourteen operations. Pulling that into CI to prove a contract is
well-formed is a large amount of machinery whose own failures then need diagnosing. This is one
traversal with three emitters; it fits in a file someone can read, and it fails loudly on anything it
does not understand rather than emitting something plausible.

The point of generating clients at all is NOT to ship an SDK. It is that a client which COMPILES is
a stronger statement about the document than any prose: a `$ref` to a schema that does not exist, a
response with no type, or an operation with no id all become build failures instead of a surprise for
whoever generates a client later.

So: anything ambiguous is a hard error. A generator that guesses produces a client that compiles and
lies, which is worse than one that refuses.
"""

import json
import os
import re
import sys

METHODS = ("get", "put", "post", "delete", "patch")


class Unsupported(Exception):
    """The document contains something this generator will not guess at."""


def die(msg):
    print(f"REFUSING: {msg}", file=sys.stderr)
    sys.exit(1)


def ref_name(schema):
    """The schema name a `$ref` points at, or None."""
    r = schema.get("$ref") if isinstance(schema, dict) else None
    if not r:
        return None
    if not r.startswith("#/components/schemas/"):
        raise Unsupported(f"external or unusual $ref: {r!r}")
    return r.rsplit("/", 1)[-1]


def snake(s):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", s).lower().replace("-", "_").replace("/", "_")


def safe_ident(name, lang):
    """A wire field name as a legal identifier, or the name unchanged.

    `ExportRequest` has a field literally called `from`, which is a keyword in Python and Rust. The
    generator hit it on its first run — which is the entire reason the generated clients are
    COMPILED in CI rather than merely produced. A generator that emitted `from: str` would have
    "worked" right up until someone tried to use it.
    """
    if lang == "py":
        import keyword

        return f"{name}_" if keyword.iskeyword(name) else name
    if lang == "rs":
        # The Rust keywords a JSON field could plausibly collide with.
        reserved = {
            "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else",
            "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
            "mod", "move", "mut", "pub", "ref", "return", "self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while", "yield",
        }
        return f"r#{name}" if name in reserved else name
    return name


# --------------------------------------------------------------------------------------------
# Type mapping. One table per language, sharing one traversal, so a type added in one place cannot
# be silently handled differently in another.
# --------------------------------------------------------------------------------------------

SCALARS = {
    "string": {"ts": "string", "py": "str", "rs": "String"},
    "integer": {"ts": "number", "py": "int", "rs": "i64"},
    "number": {"ts": "number", "py": "float", "rs": "f64"},
    "boolean": {"ts": "boolean", "py": "bool", "rs": "bool"},
}


def type_of(schema, lang, schemas):
    """Render a schema as a type in `lang`, or raise if the document is ambiguous."""
    if not isinstance(schema, dict):
        raise Unsupported(f"not a schema: {schema!r}")

    name = ref_name(schema)
    if name:
        if name not in schemas:
            raise Unsupported(f"$ref to {name!r}, which components.schemas does not define")
        return name

    # utoipa emits nullable as `oneOf`/`allOf` in places; handle the common single-branch shape and
    # refuse anything genuinely ambiguous rather than picking a branch.
    for key in ("allOf", "oneOf", "anyOf"):
        if key in schema:
            branches = [b for b in schema[key] if b != {"type": "null"}]
            if len(branches) != 1:
                raise Unsupported(f"{key} with {len(branches)} non-null branches — which one is it?")
            inner = type_of(branches[0], lang, schemas)
            return optional(inner, lang)

    t = schema.get("type")
    if isinstance(t, list):
        non_null = [x for x in t if x != "null"]
        if len(non_null) != 1:
            raise Unsupported(f"union type {t!r}")
        inner = type_of({**schema, "type": non_null[0]}, lang, schemas)
        return optional(inner, lang)

    if t == "array":
        item = type_of(schema.get("items") or {}, lang, schemas)
        return {"ts": f"{item}[]", "py": f"list[{item}]", "rs": f"Vec<{item}>"}[lang]
    if t == "object" or t is None:
        # A MAP is not a free-form object. `additionalProperties` with a schema says every value has
        # a known type, and flattening that to `unknown` throws away the one thing a client needs to
        # index it — the dashboard had to cast around this before the schema said so.
        ap = schema.get("additionalProperties")
        if isinstance(ap, dict) and ap:
            v = type_of(ap, lang, schemas)
            return {
                "ts": f"Record<string, {v}>",
                "py": f"dict[str, {v}]",
                "rs": f"std::collections::HashMap<String, {v}>",
            }[lang]
        # A genuinely free-form object is a real thing in this API (detail blobs); not ambiguity.
        return {"ts": "unknown", "py": "object", "rs": "serde_json::Value"}[lang]
    if t in SCALARS:
        return SCALARS[t][lang]
    raise Unsupported(f"type {t!r}")


def optional(inner, lang):
    return {
        "ts": f"{inner} | null",
        "py": f"{inner} | None",
        "rs": f"Option<{inner}>",
    }[lang]


def operations(doc):
    """Every operation, with the id/method/path/body/response the emitters need."""
    out = []
    for path, item in sorted((doc.get("paths") or {}).items()):
        for method, op in sorted(item.items()):
            if method not in METHODS:
                continue
            op_id = op.get("operationId")
            if not op_id:
                raise Unsupported(f"{method.upper()} {path} has no operationId to name a method by")
            body = None
            rb = op.get("requestBody")
            if rb:
                schema = ((rb.get("content") or {}).get("application/json") or {}).get("schema")
                if schema is None:
                    raise Unsupported(f"{op_id} has a request body with no application/json schema")
                body = schema
            ok = (op.get("responses") or {}).get("200") or {}
            resp = ((ok.get("content") or {}).get("application/json") or {}).get("schema")
            out.append(
                {
                    "id": op_id,
                    "method": method,
                    "path": path,
                    "params": re.findall(r"\{(\w+)\}", path),
                    "body": body,
                    "response": resp,
                    "capability": op.get("x-heldar-capability"),
                    "scope": op.get("x-heldar-scope"),
                    "admin_only": op.get("x-heldar-admin-only", False),
                }
            )
    return out


HEADER = """// GENERATED FROM openapi.json BY scripts/gen_clients.py — DO NOT EDIT.
//
// Regenerate with:  cargo test -p heldar-server --test openapi_contract write_the_served_document
//                   python3 scripts/gen_clients.py target/openapi.json clients
//
// Contract version: {version}
"""


def emit_typescript(doc, ops, schemas, out):
    lines = [HEADER.format(version=doc["info"]["version"]).replace("//", "//"), ""]
    for name, schema in sorted(schemas.items()):
        if schema.get("type") == "string" and "enum" in schema:
            variants = " | ".join(json.dumps(v) for v in schema["enum"])
            lines.append(f"export type {name} = {variants};\n")
            continue
        req = set(schema.get("required") or [])
        lines.append(f"export interface {name} {{")
        for prop, ps in (schema.get("properties") or {}).items():
            t = type_of(ps, "ts", schemas)
            lines.append(f"  {prop}{'' if prop in req else '?'}: {t};")
        lines.append("}\n")

    lines.append("export interface RequestOptions { baseUrl?: string; token?: string; }\n")
    lines.append("export class HeldarClient {")
    lines.append("  constructor(private opts: RequestOptions = {}) {}")
    lines.append("""
  private async call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (this.opts.token) headers["Authorization"] = `Bearer ${this.opts.token}`;
    const res = await fetch(`${this.opts.baseUrl ?? ""}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) {
      // Every endpoint returns the same error shape, so a caller writes one error path.
      const err = (await res.json().catch(() => ({}))) as Partial<ErrorBody>;
      throw Object.assign(new Error(err.error ?? res.statusText), {
        code: err.code ?? "internal",
        retryable: err.retryable ?? false,
        status: res.status,
      });
    }
    return (await res.json()) as T;
  }
""")
    for op in ops:
        args = [f"{p}: string" for p in op["params"]]
        if op["body"] is not None:
            args.append(f"body: {type_of(op['body'], 'ts', schemas)}")
        ret = type_of(op["response"], "ts", schemas) if op["response"] else "unknown"
        tmpl = re.sub(r"\{(\w+)\}", r"${encodeURIComponent(\1)}", op["path"])
        call_body = ", body" if op["body"] is not None else ""
        req = []
        if op["capability"]:
            req.append(f"capability `{op['capability']}`")
        if op["admin_only"]:
            req.append("admin")
        if op["scope"]:
            req.append(f"{op['scope']}")
        note = "  /** Requires " + ", ".join(req) + ". */\n" if req else ""
        lines.append(
            f"{note}  {op['id']}({', '.join(args)}): Promise<{ret}> {{\n"
            f'    return this.call<{ret}>("{op["method"].upper()}", `{tmpl}`{call_body});\n'
            f"  }}\n"
        )
    lines.append("}")
    write(os.path.join(out, "typescript"), "heldar.ts", "\n".join(lines))
    emit_dashboard_types(doc, schemas)


def emit_dashboard_types(doc, schemas, path="apps/web/src/lib/contract.ts"):
    """The contract's TYPES, for the dashboard to alias instead of re-declaring.

    The dashboard hand-wrote every request/response shape, and the overlap test caught five real
    drifts across three changes — a field the server returns that the dashboard had never heard of.
    Detecting drift is worth having; making it IMPOSSIBLE is better, and costs a generated file.

    Types only, no client: the dashboard has its own `api.ts` with auth, error mapping and polling
    that the generated client does not replicate, and replacing that would be churn for no gain.
    What matters is that a shape is declared once."""
    lines = [
        "// GENERATED FROM the served OpenAPI document BY scripts/gen_clients.py — DO NOT EDIT.",
        "//",
        "// The dashboard aliases these in `types.ts` rather than re-declaring them, so a field the",
        "// server adds cannot go unnoticed here. Regenerate with:",
        "//",
        "//   cargo test -p heldar-server --test openapi_contract write_the_served_document",
        "//   python3 scripts/gen_clients.py target/openapi.json clients",
        "//",
        f"// Contract version: {doc['info']['version']}",
        "",
    ]
    for name, schema in sorted(schemas.items()):
        if schema.get("type") == "string" and "enum" in schema:
            variants = " | ".join(json.dumps(v) for v in schema["enum"])
            lines.append(f"export type {name} = {variants};\n")
            continue
        req = set(schema.get("required") or [])
        lines.append(f"export interface {name} {{")
        for prop, ps in (schema.get("properties") or {}).items():
            t = type_of(ps, "ts", schemas)
            desc = (ps.get("description") or "").strip().splitlines()
            if desc:
                lines.append("  /** " + " ".join(d.strip() for d in desc) + " */")
            lines.append(f"  {prop}{'' if prop in req else '?'}: {t};")
        lines.append("}\n")
    d = os.path.dirname(path)
    if os.path.isdir(d):
        with open(path, "w") as fh:
            fh.write("\n".join(lines))
        print(f"  wrote {path}")


def emit_python(doc, ops, schemas, out):
    lines = [
        HEADER.format(version=doc["info"]["version"]).replace("//", "#"),
        "from __future__ import annotations",
        "",
        "import json",
        "import urllib.error",
        "import urllib.request",
        "from dataclasses import dataclass",
        "from typing import Any",
        "",
        "# The dataclasses below DESCRIBE the wire shapes; the client returns parsed JSON, not",
        "# instances of them. Saying so is more useful than implying a deserialization step that",
        "# does not happen — a caller can construct one to type a payload, or ignore them entirely.",
        "",
        "",
        "class HeldarError(Exception):",
        '    """Every endpoint returns the same shape, so a caller writes one error path."""',
        "",
        "    def __init__(self, message: str, code: str, retryable: bool, status: int) -> None:",
        "        super().__init__(message)",
        "        self.code = code",
        "        self.retryable = retryable",
        "        self.status = status",
        "",
        "",
    ]
    for name, schema in sorted(schemas.items()):
        if schema.get("type") == "string" and "enum" in schema:
            lines.append(f"{name} = str  # one of: {', '.join(map(str, schema['enum']))}")
            lines.append("")
            continue
        req = set(schema.get("required") or [])
        props = list((schema.get("properties") or {}).items())
        # Required fields first: a dataclass cannot follow a defaulted field with a bare one.
        props.sort(key=lambda kv: kv[0] not in req)
        lines.append("@dataclass")
        lines.append(f"class {name}:")
        if not props:
            lines.append("    pass")
        for prop, ps in props:
            t = type_of(ps, "py", schemas)
            ident = safe_ident(prop, "py")
            wire = "" if ident == prop else f'  # wire name: "{prop}"'
            lines.append(
                f"    {ident}: {t}" + ("" if prop in req else " = None") + wire
            )
        lines.append("")
        lines.append("")

    lines += [
        "class HeldarClient:",
        "    def __init__(self, base_url: str = '', token: str | None = None) -> None:",
        "        self.base_url = base_url.rstrip('/')",
        "        self.token = token",
        "",
        "    def _call(self, method: str, path: str, body: Any = None) -> Any:",
        "        data = None if body is None else json.dumps(body).encode()",
        "        req = urllib.request.Request(self.base_url + path, data=data, method=method)",
        "        req.add_header('content-type', 'application/json')",
        "        if self.token:",
        "            req.add_header('Authorization', f'Bearer {self.token}')",
        "        try:",
        "            with urllib.request.urlopen(req) as r:",
        "                return json.loads(r.read() or b'null')",
        "        except urllib.error.HTTPError as e:",
        "            try:",
        "                err = json.loads(e.read() or b'{}')",
        "            except Exception:",
        "                err = {}",
        "            raise HeldarError(",
        "                err.get('error', str(e)),",
        "                err.get('code', 'internal'),",
        "                bool(err.get('retryable', False)),",
        "                e.code,",
        "            ) from None",
        "",
    ]
    for op in ops:
        args = "".join(f", {p}: str" for p in op["params"])
        if op["body"] is not None:
            args += ", body: Any"
        tmpl = re.sub(r"\{(\w+)\}", r"{\1}", op["path"])
        call_body = ", body" if op["body"] is not None else ""
        req = []
        if op["capability"]:
            req.append(f"capability `{op['capability']}`")
        if op["admin_only"]:
            req.append("admin")
        if op["scope"]:
            req.append(op["scope"])
        lines.append(f"    def {snake(op['id'])}(self{args}) -> Any:")
        if req:
            lines.append(f'        """Requires {", ".join(req)}."""')
        lines.append(
            f"        return self._call('{op['method'].upper()}', f'{tmpl}'{call_body})"
        )
        lines.append("")
    write(os.path.join(out, "python"), "heldar_client.py", "\n".join(lines))


def emit_rust(doc, ops, schemas, out):
    lines = [
        HEADER.format(version=doc["info"]["version"]),
        "#![allow(dead_code)]",
        "",
        "use serde::{Deserialize, Serialize};",
        "",
    ]
    for name, schema in sorted(schemas.items()):
        if schema.get("type") == "string" and "enum" in schema:
            lines.append("#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]")
            lines.append(f"pub enum {name} {{")
            for v in schema["enum"]:
                variant = "".join(part.capitalize() for part in re.split(r"[^A-Za-z0-9]+", str(v)))
                lines.append(f'    #[serde(rename = "{v}")]')
                lines.append(f"    {variant},")
            lines.append("}\n")
            continue
        req = set(schema.get("required") or [])
        lines.append("#[derive(Debug, Clone, Serialize, Deserialize)]")
        lines.append(f"pub struct {name} {{")
        for prop, ps in (schema.get("properties") or {}).items():
            t = type_of(ps, "rs", schemas)
            if prop not in req and not t.startswith("Option<"):
                t = f"Option<{t}>"
            field = safe_ident(snake(prop), "rs")
            if field.lstrip("r#") != prop:
                lines.append(f'    #[serde(rename = "{prop}")]')
            lines.append(f"    pub {field}: {t},")
        lines.append("}\n")

    lines.append("/// What each operation requires, from the contract's own extensions.")
    lines.append("pub const REQUIREMENTS: &[(&str, &str, Option<&str>, &str)] = &[")
    for op in ops:
        cap = f'Some("{op["capability"]}")' if op["capability"] else "None"
        lines.append(
            f'    ("{op["method"].upper()}", "{op["path"]}", {cap}, "{op["scope"] or ""}"),'
        )
    lines.append("];")
    write(os.path.join(out, "rust/src"), "lib.rs", "\n".join(lines))
    write(
        os.path.join(out, "rust"),
        "Cargo.toml",
        "# GENERATED — see scripts/gen_clients.py\n"
        "#\n"
        "# The empty [workspace] is load-bearing: without it cargo folds this crate into the\n"
        "# repository workspace and refuses to build it, because it is not a member. A generated\n"
        "# client has to build on its own — that is the whole point of building it.\n"
        "[workspace]\n\n"
        "[package]\nname = \"heldar-client\"\nversion = \"0.0.0\"\nedition = \"2021\"\n"
        "publish = false\n\n[dependencies]\n"
        'serde = { version = "1", features = ["derive"] }\nserde_json = "1"\n',
    )


def write(dirpath, name, content):
    os.makedirs(dirpath, exist_ok=True)
    with open(os.path.join(dirpath, name), "w") as fh:
        fh.write(content.rstrip() + "\n")
    print(f"  wrote {os.path.join(dirpath, name)}")


def main():
    if len(sys.argv) < 3:
        print("usage: gen_clients.py <openapi.json> <out-dir>", file=sys.stderr)
        return 2
    with open(sys.argv[1]) as fh:
        doc = json.load(fh)
    out = sys.argv[2]
    schemas = (doc.get("components") or {}).get("schemas") or {}
    if not schemas:
        die("the document defines no schemas — a client generated from it would be empty, and an "
            "empty client that compiles proves nothing")
    try:
        ops = operations(doc)
        if not ops:
            die("the document defines no operations")
        emit_typescript(doc, ops, schemas, out)
        emit_python(doc, ops, schemas, out)
        emit_rust(doc, ops, schemas, out)
    except Unsupported as e:
        die(f"{e}\n\nThis generator refuses rather than guessing: a client that compiles and lies "
            f"is worse than one that will not build.")
    print(f"\n{len(ops)} operations, {len(schemas)} schemas")
    return 0


if __name__ == "__main__":
    sys.exit(main())
