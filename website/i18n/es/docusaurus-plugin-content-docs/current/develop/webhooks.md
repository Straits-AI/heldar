---
id: webhooks
title: Webhooks e integración
sidebar_label: Webhooks e integración
sidebar_position: 3
---

# Webhooks y el sustrato de integración

Los webhooks son el mecanismo por el que una aplicación externa o **padre** recibe eventos de Heldar
en tiempo casi real. Una *suscripción webhook* registra una URL, un filtro de tipos de evento,
una severidad mínima y un secreto de firma opcional; el kernel entonces realiza un POST de cada
evento coincidente a esa URL como JSON firmado, con entrega al menos una vez y
reintentos.

Esta es la maquinaria de integración **genérica** que vive en el kernel abierto.
Los verticales se construyen sobre el mismo sustrato — declaran sus propios tipos de evento de dominio
y exponen sus propios endpoints REST — sin que el kernel sepa que existen
(véase [Verticales sobre el mismo sustrato](#verticales-sobre-el-mismo-sustrato)).

Todas las rutas a continuación viven bajo `/api/v1`. Gestionar suscripciones requiere el
rol `manager` (o `admin`); las lecturas requieren cualquier principal autenticado. Cuando
`HELDAR_AUTH_ENABLED=false` (el modo predeterminado de dispositivo LAN monoinquilino), cada
llamante es un principal permisivo, por lo que los endpoints están abiertos. Los despliegues autenticados
pasan una clave como `Authorization: Bearer <key>` o `X-API-Key: <key>`.

## Registrar un webhook

Crea una suscripción con `POST /api/v1/webhooks`:

```bash
curl -sS -X POST http://localhost:8000/api/v1/webhooks \
  -H 'Authorization: Bearer <api-key>' \
  -H 'Content-Type: application/json' \
  -d '{
        "name": "Ops Slack bridge",
        "url": "https://example.com/heldar/webhook",
        "event_types": ["zone_enter", "disk_pressure"],
        "min_severity": "warning",
        "secret": "whsec_a5f3…"
      }'
```

| Campo          | Predeterminado | Significado                                                                                       |
| -------------- | -------------- | ------------------------------------------------------------------------------------------------- |
| `name`         | —              | Etiqueta legible (obligatorio).                                                                    |
| `url`          | —              | El destino POST `http(s)` (obligatorio).                                                           |
| `event_types`  | `["*"]`        | Conjunto de tipos de evento a entregar. `["*"]` (u omitido) coincide con **todos** los tipos.     |
| `min_severity` | `info`         | `info` (todos), `warning` (warning + critical), o `critical` (solo critical).                     |
| `secret`       | none           | Clave de firma HMAC-SHA256. Cuando se establece, cada entrega lleva una cabecera `X-Heldar-Signature`. |
| `enabled`      | `true`         | Pausar la entrega sin eliminar la suscripción.                                                    |

El secreto es de **solo escritura**: nunca se devuelve. Las lecturas exponen únicamente un
booleano `has_secret`. Al actualizar (`PATCH /api/v1/webhooks/{id}`), el campo `secret`
tiene tres estados — omítelo para conservar el secreto actual, envía `null`/`""` para
borrarlo, o envía un valor para reemplazarlo.

Otros endpoints:

- `GET /api/v1/webhooks` — listar suscripciones.
- `PATCH /api/v1/webhooks/{id}` — actualización parcial (cualquier campo ausente no cambia).
- `DELETE /api/v1/webhooks/{id}` — eliminar una suscripción.
- `POST /api/v1/webhooks/{id}/test` — entregar un evento firmado sintético a la
  URL y devolver `{ ok, status, error }`.
- `GET /api/v1/webhooks/{id}/deliveries?limit=` — los intentos de entrega recientes
  (estado, código de respuesta, marcas de tiempo).

Los operadores pueden hacer todo esto sin la API desde el panel:
**Sistema → Webhooks**.

## El payload entregado

Cada entrega es un único objeto JSON — el sobre del evento — enviado por POST con estas
cabeceras:

| Cabecera             | Valor                                                                |
| -------------------- | -------------------------------------------------------------------- |
| `Content-Type`       | `application/json`                                                   |
| `X-Heldar-Event`     | El tipo de evento (p. ej. `zone_enter`).                             |
| `X-Heldar-Delivery`  | Un id único para este intento de entrega (úsalo para deduplicar).   |
| `X-Heldar-Timestamp` | Segundos Unix en que se envió la solicitud.                          |
| `X-Heldar-Signature` | `sha256=<hex>` HMAC-SHA256 del **cuerpo en bruto** — solo cuando se ha configurado un secreto. |

El cuerpo:

```json
{
  "id": "evt_9c1f…",
  "camera_id": "gate_a",
  "site_id": "hq",
  "event_type": "zone_enter",
  "severity": "warning",
  "timestamp": "2026-01-12T09:14:33.102Z",
  "payload": { "zone_id": "zone_7", "zone_name": "Loading bay", "track_id": "t-42", "label": "person" }
}
```

`camera_id` y `site_id` pueden ser `null` para eventos a nivel de sistema. `payload` es un
objeto específico del tipo de evento — su forma la define quien emite el evento
(el kernel, una app o un worker de IA).

## Verificar la firma

Cuando se configura un secreto, verifica `X-Heldar-Signature` antes de confiar en una
solicitud. Calcula HMAC-SHA256 sobre los **bytes exactos en bruto de la solicitud** — no
vuelvas a serializar el JSON analizado, ya que el orden de las claves y los espacios en blanco diferirían y
la firma no coincidiría. Compara siempre en tiempo constante.

```python
import hashlib
import hmac

def verify(secret: str, raw_body: bytes, signature_header: str | None) -> bool:
    if not signature_header:
        return False
    expected = "sha256=" + hmac.new(secret.encode(), raw_body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature_header)
```

```js
// Node.js
import { createHmac, timingSafeEqual } from "node:crypto";

function verify(secret, rawBody, signatureHeader) {
  if (!signatureHeader) return false;
  const expected = "sha256=" + createHmac("sha256", secret).update(rawBody).digest("hex");
  const a = Buffer.from(expected);
  const b = Buffer.from(signatureHeader);
  return a.length === b.length && timingSafeEqual(a, b);
}
```

## Semántica de entrega

- **Al menos una vez.** Cada suscripción mantiene su propio cursor de entrega (una marca
  de tiempo de evento). Planifica para duplicados: haz que tu manejador sea idempotente deduplicando
  sobre el `id` del evento (o `X-Heldar-Delivery`).
- **Sin reproducción de historial.** Una nueva suscripción comienza en "ahora", por lo que añadir una nunca
  te inunda con eventos históricos.
- **Acusar recibo con 2xx.** Cualquier respuesta `2xx` cuenta como entregado. Una respuesta no-2xx,
  un timeout o un error de conexión es un fallo y se reintenta en el
  siguiente ciclo (el intervalo de sondeo, mínimo 5s).
- **Reintentos acotados.** Un evento se reintenta hasta 5 veces. Después de eso, el kernel
  desiste de ese evento y avanza el cursor más allá de él, de modo que un único endpoint defectuoso
  nunca puede bloquear la cola. Cada intento — éxito o fallo — se
  registra en el log de entregas (`GET /api/v1/webhooks/{id}/deliveries`).
- **Responde rápido.** Devuelve rápidamente (acusa recibo primero, procesa de forma asíncrona). Los manejadores lentos
  cuentan contra el timeout por solicitud y parecen fallos.

## Taxonomía de tipos de evento

`GET /api/v1/events/types` devuelve los tipos de evento incorporados con una descripción de una línea
cada uno — la misma lista que puebla el selector de tipos de evento del panel. Úsala para
construir una interfaz o para validar un filtro. Los tipos incorporados del kernel y de las apps
de referencia incluyen:

| `event_type`         | Descripción                                                          |
| -------------------- | -------------------------------------------------------------------- |
| `camera_offline`     | El grabador de una cámara perdió su conexión RTSP.                   |
| `recorder_error`     | Un proceso grabador produjo un error o sus segmentos quedaron obsoletos. |
| `recording_gap`      | Se detectó un hueco entre segmentos grabados consecutivos.           |
| `sampler_offline`    | Un muestreador de fotogramas de IA para una cámara se desconectó.    |
| `retention_delete`   | El barredor de retención eliminó segmentos antiguos.                  |
| `disk_pressure`      | El almacenamiento de grabación está bajo presión (cuota, límite de tamaño o umbral de espacio libre). |
| `disk_smart_warning` | Una autoevaluación SMART reportó una advertencia de salud del disco.  |
| `raid_degraded`      | Un array md/RAID de Linux reportó un miembro degradado o caído.      |
| `zone_enter`         | Una detección rastreada entró en una zona configurada.               |
| `zone_exit`          | Una detección rastreada salió de una zona configurada.               |
| `zone_dwell`         | Una detección rastreada permaneció dentro de una zona más allá de su umbral. |
| `entry_matched`      | Control de acceso: una entrada coincidió con el registro y fue autorizada. |
| `entry_exception`    | Control de acceso: una entrada requiere revisión del operador.       |
| `entry_unmatched`    | Control de acceso: una entrada no coincidió con ningún registro.     |
| `entry_blocked`      | Control de acceso: una entrada coincidió con una lista de vigilancia/bloqueo y fue denegada. |

Esta lista es **descriptiva, no exhaustiva**. Las apps y los workers de IA emiten sus propias
cadenas `event_type` personalizadas en el mismo log de eventos, y un webhook con
`event_types: ["*"]` también las entrega.

## Verticales sobre el mismo sustrato

Un vertical (una app de dominio construida sobre el kernel) reutiliza esta maquinaria en lugar de
reinventarla. Declara sus propias cadenas `event_type` de dominio — escritas en el
log de eventos canónico a través del kernel — y expone sus propios endpoints REST; la
auth genérica, el log de eventos, el outbox transaccional y la entrega de webhooks se heredan
del kernel. Véase [Construir un módulo](./build-a-module.md) para los
puntos de integración.

Tomemos un vertical de portal de visitantes como patrón de trabajo. Se integra en dos
direcciones sobre el kernel:

- **Entrante** — el portal llama a los propios endpoints REST del vertical (por ejemplo,
  para pre-registrar un visitante), autenticado con una **clave API** de Heldar con alcance al
  rol `integration`. Los endpoints son del vertical; la clave API, RBAC
  y el log de auditoría son del kernel.
- **Saliente** — la app padre suscribe un webhook a los eventos de dominio del vertical (por ejemplo,
  un evento `portal.*` que emite el vertical), filtrado por tipo de evento y severidad y verificado
  con el mismo HMAC `X-Heldar-Signature`. No se necesita código de entrega
  específico del vertical — es el mismo motor documentado anteriormente.

Por tanto, la historia de integración de un vertical es simplemente: *declarar tipos de evento de dominio + exponer
endpoints de dominio*, y la auth genérica de clave API (entrante) y las suscripciones webhook
(saliente) vienen de forma gratuita.
