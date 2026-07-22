---
id: sidecar-plugins
title: Plugins Sidecar
sidebar_label: Plugins Sidecar
sidebar_position: 2
---

# Plugins sidecar

Un **plugin sidecar** extiende Heldar sin necesidad de compilarlo en el binario. Es un servicio HTTP
fuera del proceso — en cualquier lenguaje, como proceso o contenedor — que Heldar **instala en tiempo
de ejecución**: sin recompilación, con aislamiento de proceso/contenedor y acceso de mínimos
privilegios. Este es el camino para módulos de terceros y de elaboración propia. (Para una aplicación
Rust de primera parte, estrechamente integrada, que comparte la base de datos del kernel y la ruta de
ingesta en caliente, utiliza en su lugar un [crate de aplicación compilado](./build-a-module.md).)

La referencia completa y ejecutable se encuentra en
[`examples/hello-module`](https://github.com/Straits-AI/heldar/tree/main/examples/hello-module) — un
sidecar Python sin dependencias que puedes registrar y ver recibir eventos en minutos.

## Cómo encaja todo

Cuando instalas un sidecar, Heldar realiza tres acciones reversibles:

1. **Acuña una clave de API con ámbito** que el sidecar usa para llamar de vuelta a las APIs del
   kernel. La clave es de mínimos privilegios: `viewer` (lectura) o `integration` (lectura + ingesta).
   Los roles `admin`/`manager` nunca se conceden a un plugin.
2. **Crea una suscripción de webhook** que firma y entrega los eventos a los que te suscribes.
3. **Realiza un proxy inverso de `/m/{id}/*`** hacia tu servicio, de modo que tu interfaz y tu API
   comparten el mismo origen que la consola (montado como micro-frontend — tu interfaz no se incluye
   en el paquete de Heldar).

La desinstalación revierte las tres acciones: la clave se revoca, la suscripción se elimina y la ruta
se retira.

![Heldar Core y un plugin sidecar](/img/diagrams/sidecar.svg)

## Los cuatro endpoints

Tu sidecar sirve estos. Solo los dos primeros son obligatorios.

| Endpoint | Llamante | Contrato |
| --- | --- | --- |
| `GET /heldar/health` | kernel (cada 30 s) | devuelve cualquier `2xx` para ser marcado como **saludable** |
| `POST /heldar/events` | kernel | entregas de eventos; verificar `X-Heldar-Signature` (ver más abajo) |
| `GET /` y tus recursos | iframe del panel | tu interfaz de plugin, servida en `/m/{id}/` |
| `GET /api/...` | tu interfaz | la API de datos de tu interfaz, accesible a través de `/m/{id}/api/...` |

Dado que la interfaz se monta en `/m/{id}/`, haz que sus peticiones de recursos y API sean
**relativas** (`fetch("api/events")`, no `fetch("/api/events")`) para que se resuelvan a través del
proxy.

## El manifiesto

Te registras presentando un manifiesto. La misma estructura describe un módulo en proceso (que lo
devuelve desde código); un sidecar lo envía a `POST /api/v1/modules`:

```json
{
  "id": "visitor-portal",
  "name": "Visitor Portal",
  "version": "1.0.0",
  "publisher": "ACME Corp",
  "description": "Self-service visitor pre-registration",
  "base_url": "http://127.0.0.1:9123",
  "nav": [{ "path": "/visitor-portal", "label": "Visitors", "icon": "module" }],
  "subscribes": ["entry_matched", "entry_unmatched"],
  "role": "integration"
}
```

| Campo | Significado |
| --- | --- |
| `id` | slug estable; el punto de montaje `/m/{id}/` y la clave de navegación. No debe colisionar con un módulo integrado. |
| `base_url` | el origen al que Heldar hace proxy inverso (http/https). |
| `nav` | entradas de navegación a mostrar. Omitir para una entrada predeterminada única en `/{id}`. `icon` recurre a un glifo genérico. |
| `subscribes` | tipos de eventos a recibir (`["*"]` = todos). Consulta la [taxonomía de eventos](./webhooks.md). |
| `role` | el rol de la clave acuñada: `viewer` o `integration`. |

## Registrar

Desde el panel: **Plugins → Instalar un plugin sidecar**. O a través de la API (admin):

```bash
curl -sX POST http://localhost:8000/api/v1/modules \
  -H 'authorization: Bearer <ADMIN_API_KEY>' \
  -H 'content-type: application/json' \
  -d @manifest.json
```

La respuesta devuelve — **una sola vez** — las credenciales con las que configurar tu sidecar:

```json
{
  "module": { "id": "visitor-portal", "base_url": "http://127.0.0.1:9123", ... },
  "api_key": "vok_…",          // -> tu HELDAR_API_KEY (llamadas a la API del kernel)
  "webhook_secret": "whsec_…"  // -> tu HELDAR_WEBHOOK_SECRET (verificar entregas)
}
```

Guarda ambas inmediatamente; nunca se vuelven a mostrar. Desinstala con
`DELETE /api/v1/modules/{id}` (o el botón **Desinstalar**).

## Recibir eventos

El kernel hace `POST` de cada evento suscrito a `{base_url}/heldar/events` con las cabeceras:

- `X-Heldar-Event` — el tipo de evento
- `X-Heldar-Delivery` — un id de entrega único
- `X-Heldar-Timestamp` — segundos Unix
- `X-Heldar-Signature` — `sha256=<hex HMAC-SHA256(webhook_secret, raw_body)>`

**Verifica siempre la firma** sobre los bytes exactos de la petición:

```python
import hashlib, hmac
def verify(raw: bytes, header: str, secret: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), raw, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

Devuelve `2xx` para confirmar la recepción. Las respuestas no-2xx (o un tiempo de espera agotado) son
reintentadas por el motor de entrega al-menos-una-vez, así que haz tu manejador idempotente en
`X-Heldar-Delivery`.

## Llamar al kernel de vuelta

Usa la clave acuñada como token bearer contra cualquier API del kernel que tu rol permita:

```bash
curl http://localhost:8000/api/v1/events \
  -H "authorization: Bearer $HELDAR_API_KEY"
```

Las claves `integration` también pueden hacer POST de detecciones en el pipeline de ingesta; las
claves `viewer` son de solo lectura.

## Modelo de seguridad

- Los plugins son **instalados por un administrador** y se ejecutan **fuera del proceso** — aislarlos
  como lo harías con cualquier servicio (contenedor, política de red, un `base_url` no-loopback solo
  cuando confíes en la ruta).
- La consola **nunca reenvía tu cookie de sesión** a un sidecar; el sidecar se autentica en el kernel
  únicamente con su propia clave acuñada.
- El iframe de la interfaz del plugin está en sandbox (`allow-scripts allow-same-origin allow-forms allow-popups`);
  no puede navegar ni actuar sobre el frame de la consola principal.
- La desinstalación revoca completamente la clave y la suscripción, por lo que un plugin eliminado no
  conserva ningún acceso activo.
