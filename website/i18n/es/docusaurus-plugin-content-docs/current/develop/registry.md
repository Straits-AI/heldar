---
id: registry
title: Registro de plugins
sidebar_label: Registro de plugins
sidebar_position: 3
---

# Registro de plugins

La **Tienda de plugins** navega por un *catálogo* de plugins disponibles y lo cruza con lo que está
cargado. Un catálogo proviene de dos tipos de fuentes:

- el catálogo **integrado** de primera parte, compilado en el binario — siempre disponible, incluso sin
  conexión, y de confianza por construcción;
- **registros remotos** opcionales (documentos JSON firmados en URLs configuradas por el administrador)
  — así es como se pueblan las estanterías propietarias y de la comunidad, sin compilar nada en el
  binario.

Instalar una entrada de sidecar pasa por el [flujo de registro de sidecar](./sidecar-plugins.md); el
catálogo solo realiza el descubrimiento. Los módulos en proceso se muestran como *Incluido* / *Contacto*
— están enlazados al kernel en tiempo de compilación, no son instalables en tiempo de ejecución.

## Formato del catálogo (`heldar-catalog/v1`)

```json
{
  "format": "heldar-catalog/v1",
  "name": "Acme Registry",
  "issued_at": "2026-06-16T00:00:00Z",
  "expires_at": "2026-12-16T00:00:00Z",
  "entries": [
    {
      "id": "weather-overlay",
      "name": "Weather Overlay",
      "publisher": "Acme Plugins",
      "kind": "community",
      "summary": "Overlay local weather on the wall.",
      "description": "Longer copy shown in the detail drawer.",
      "version": "1.0.0",
      "icon": "module",
      "homepage": "https://example.com/weather-overlay",
      "categories": ["overlay"],
      "install": {
        "type": "sidecar",
        "image": "ghcr.io/acme/weather-overlay:1.0.0",
        "default_base_url": "http://127.0.0.1:9300",
        "subscribes": ["*"],
        "role": "viewer"
      }
    }
  ]
}
```

| Campo | Significado |
| --- | --- |
| `kind` | `core` / `proprietary` / `community` — selecciona la estantería y la insignia. |
| `install.type` | `sidecar` (instalable en tiempo de ejecución, rellena previamente el formulario de registro) o `builtin` (compilado; solo CTA). |
| `install.default_base_url` | solo sidecar: la URL que el formulario de instalación rellena previamente (editable por el operador). |
| `install.subscribes` / `role` | solo sidecar: tipos de eventos a recibir + el rol de la clave generada. |
| `install.image` | solo sidecar: una sugerencia informativa de despliegue — el kernel nunca la descarga ni la ejecuta. |
| `install.availability` / `contact` | solo builtin: `open` / `commercial` + un contacto para el CTA. |

El panel cruza cada entrada con el estado en vivo y muestra uno de: **Disponible**,
**Instalado**, **Incluido**, **No en la compilación**, **Inalcanzable**.

## Modelo de confianza

Un catálogo remoto solo es de confianza si su **firma Ed25519 independiente** verifica contra una
**clave pública anclada**. La firma cubre los bytes *exactos* del catálogo (sin canonicalización JSON),
siguiendo el mismo patrón que el firmante de webhooks.

- El artefacto `<catalog-url>.sig` se ubica junto al catálogo: `{ "alg": "ed25519", "key_id": "...",
  "signature": "<base64 raw 64-byte sig>" }`.
- La verificación se ejecuta **del lado del servidor** contra las claves ancladas en tiempo de
  compilación más las claves del operador en `HELDAR_REGISTRY_TRUSTED_KEYS`. El navegador nunca ve una
  clave y nunca verifica — por lo tanto, un catálogo falsificado nunca puede mostrar una insignia
  **Verificado** falsa.
- Es **fail-closed**: una fuente remota no verificada contribuye **cero** entradas (configure
  `HELDAR_REGISTRY_ALLOW_UNVERIFIED=true` para relajar esto en un registro interno de confianza).
- El catálogo integrado es de confianza por construcción (es *el* binario), por lo que sus entradas
  siempre están verificadas — la insignia es honesta incluso sin conexión.

Una insignia **Verificado** significa que *el listado fue firmado por una clave de editor anclada* — no
que el código del plugin sea seguro. Un sidecar sigue ejecutándose fuera del proceso con una clave
generada de mínimo privilegio.

## Firmar + publicar

```bash
openssl genpkey -algorithm ed25519 -out registry.pem            # once; keep the private key secret
openssl pkey -in registry.pem -pubout -outform DER | tail -c 32 | base64   # the pinnable public key
./scripts/sign-catalog.sh catalog.json registry.pem my-key      # -> catalog.json.sig
```

Aloje `catalog.json` + `catalog.json.sig` sobre HTTPS, ancle la clave pública
(`HELDAR_REGISTRY_TRUSTED_KEYS=my-key:<base64>`), y configure `HELDAR_REGISTRY_URLS` con la URL del
catálogo. Un ejemplo ejecutable de extremo a extremo se encuentra en
[`examples/registry`](https://github.com/Straits-AI/heldar/tree/main/examples/registry).

## Configuración

| Variable de entorno | Predeterminado | Propósito |
| --- | --- | --- |
| `HELDAR_REGISTRY_ENABLED` | `true` | Interruptor principal para la obtención del registro remoto (el catálogo integrado siempre se carga). |
| `HELDAR_REGISTRY_URLS` | *(vacío)* | URLs de catálogo separadas por comas. Vacío = sin conexión externa. |
| `HELDAR_REGISTRY_REFRESH_S` | `900` | Cadencia de actualización en segundo plano. |
| `HELDAR_REGISTRY_FETCH_TIMEOUT_S` | `10` | Tiempo de espera por obtención. |
| `HELDAR_REGISTRY_TRUSTED_KEYS` | *(vacío)* | Claves ancladas adicionales, `key_id:base64pubkey,...`. |
| `HELDAR_REGISTRY_ALLOW_UNVERIFIED` | `false` | Mostrar entradas remotas no verificadas (marcadas como no verificadas). |
| `HELDAR_REGISTRY_ALLOW_PRIVATE` | `false` | Permitir URLs de registro http / privadas/loopback (protección contra SSRF). |

Las obtenciones remotas usan un cliente dedicado con redirecciones deshabilitadas, un valor
predeterminado de solo HTTPS, un límite de cuerpo de 2 MiB y rechazo literal de IP privadas/loopback.
El reenlace de hostname a IP privada está fuera del alcance de v1 (las URLs son configuradas por el
administrador y las redirecciones están desactivadas).
