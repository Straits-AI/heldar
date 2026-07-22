---
id: deploy
title: Despliegue
sidebar_label: Despliegue
sidebar_position: 2
---

# Despliegue

Heldar está diseñado para ejecutarse como **un único binario en una única URL**. El servidor compositor
(`heldar-server`, el binario `heldar-core`) sirve la API JSON, los medios grabados,
los endpoints de métricas/salud y el panel de control integrado desde un único proceso.

Tres formas de ejecutarlo, en orden de esfuerzo:

- **Docker (descargar y ejecutar):** `docker compose -f deploy/compose.yml up -d` — o el
  [inicio rápido en una línea](quickstart#fastest-docker-one-liner). Imágenes abiertas preconstruidas, sin necesidad de cadena de herramientas.
- **Binario nativo** — compilar desde el código fuente y ejecutarlo (esta página).
- **Appliance flasheado** — una imagen de SO con systemd nativo para un equipo DVR dedicado (sin Docker en el
  appliance; véase `infra/systemd/` en el repositorio).

## Un binario, una URL

Compila el panel de control y apunta el servidor hacia él con `HELDAR_WEB_DIR`:

```bash
cd apps/web && npm install && npm run build      # writes apps/web/dist
# in .env:
HELDAR_WEB_DIR=./apps/web/dist
```

Cuando `HELDAR_WEB_DIR` está definido y el directorio existe, el servidor sirve la
SPA como fallback. Las rutas explícitas tienen prioridad y la SPA solo actúa como
fallback para todo lo demás:

- `/api/*` - la API JSON.
- `/media/recordings`, `/media/clips`, `/media/snapshots`, `/media/playback`,
  `/media/archives` - archivos multimedia servidos estáticamente desde el directorio de datos.
- `/healthz` (liveness), `/readyz` (readiness, ejecuta `SELECT 1`), `/metrics`
  (exposición de Prometheus).
- todo lo demás - el panel de control; las rutas desconocidas enrutadas por el cliente vuelven a
  `index.html` para que los enlaces directos devuelvan `200`.

Si `HELDAR_WEB_DIR` no está definido, el valor predeterminado es `apps/web/dist` relativo al
directorio de trabajo del binario. Si ninguno de los dos existe, el servidor opera solo con la API y
registra que el panel de control no está siendo servido.

## Puertos

| Puerto | Servicio |
| --- | --- |
| 8000 | Heldar Core HTTP API + panel de control (`HELDAR_API_HOST` / `HELDAR_API_PORT`) |
| 5173 | Servidor de desarrollo Vite (solo desarrollo; no se usa en el despliegue de binario único) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | API de control MediaMTX (loopback) |

La vista en vivo se publica en MediaMTX mediante un ffmpeg supervisado y propiedad del kernel por
cámara; las credenciales de las cámaras permanecen en el kernel y nunca llegan a MediaMTX ni al
navegador, que únicamente ve las URLs de HLS/WebRTC/RTSP sin credenciales. El motor de transcodificación
en vivo toma su valor por defecto de `HELDAR_LIVE_TRANSCODE_ENGINE` (`software`) y puede cambiarse en
tiempo de ejecución (software / VAAPI / NVENC) desde la página Sistema o mediante
`GET`/`PUT /api/v1/system/transcode`.

## Autenticación

La autenticación y el RBAC son **opcionales** mediante `HELDAR_AUTH_ENABLED` (por defecto `false`).

- **`false`** - API abierta, adecuada para un appliance LAN de un único inquilino. La superficie de
  administración es accesible sin token y actúa como administrador.
- **`true`** - cada solicitud requiere una sesión (inicio de sesión) o una `X-API-Key`. Se aplican cinco
  roles (`admin` / `manager` / `guard` / `viewer` / `integration`)
  sobre las capacidades, y cada mutación se escribe en un registro de auditoría inmutable.
  En el primer arranque sin usuarios, se crea un administrador a partir del entorno de arranque.

Las sesiones utilizan una cookie HttpOnly, SameSite=Strict. Establece
`HELDAR_AUTH_COOKIE_SECURE=true` detrás de TLS (mantén `false` para acceso LAN en HTTP plano o
acceso mediante overlay). Ajusta el tiempo de vida de la sesión con `HELDAR_SESSION_TTL_HOURS`
(por defecto 12), y opcionalmente expira las sesiones inactivas con
`HELDAR_SESSION_IDLE_TIMEOUT_MIN` (por defecto `0`, es decir, sin tiempo de inactividad).

El bloqueo por fuerza bruta está activo por defecto: una cuenta queda bloqueada tras
`HELDAR_LOGIN_MAX_FAILURES` (5) intentos consecutivos fallidos durante
`HELDAR_LOGIN_LOCKOUT_MIN` (15) minutos — se rechaza incluso con la contraseña correcta,
desbloqueándose automáticamente cuando vence el periodo (un administrador puede desbloquearlo antes mediante
`POST /api/v1/users/{id}/unlock`).

Establece `HELDAR_AUTH_ENABLED=true` para cualquier despliegue multiusuario o en red.

:::tip ¿Expuesto a Internet? Refuérzalo primero.
Para un despliegue accesible desde la Internet pública, el kernel **falla ruidosamente** ante una
configuración insegura — rechaza arrancar con la autenticación desactivada detrás de un rendezvous, y
advierte (o rechaza, bajo `HELDAR_STRICT_PROD=true`) ante una cookie no `Secure`, sin tiempo de inactividad,
o credenciales de cámara en texto plano. Sigue la
[lista de verificación de hardening en producción](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)
— autenticación, TLS, cifrado de credenciales de cámara (`HELDAR_SECRET_KEY`), y un desafío de inicio de sesión
opcional con Cloudflare Turnstile — antes de poner en marcha.
:::

## Almacenamiento y el directorio de datos

Heldar usa únicamente SQLite (journal WAL, migraciones integradas). La URL por defecto es
`sqlite://./data/heldar.db`.

| Variable | Por defecto | Significado |
| --- | --- | --- |
| `HELDAR_DATABASE_URL` | `sqlite://./data/heldar.db` | Solo SQLite; una URL que no sea `sqlite` es rechazada al arrancar |
| `HELDAR_DATA_DIR` | `./data` | raíz para la BD y los subdirectorios de medios |
| `HELDAR_RECORDINGS_DIR` / `CLIPS_DIR` / `SNAPSHOTS_DIR` / `FRAMES_DIR` | bajo `./data` | raíces de medios (creadas al arrancar) |
| `HELDAR_MAX_RECORDINGS_GB` | `20` | límite suave de espacio; los segmentos más antiguos no bloqueados se eliminan al superarlo |
| `HELDAR_MIN_FREE_DISK_GB` | `5` | piso duro de protección del host; elimina segmentos no bloqueados mientras el espacio libre esté por debajo |
| `HELDAR_MAX_DB_GB` | `4` | limita la propia BD de metadatos `heldar.db`; el espacio se recupera en línea mediante auto_vacuum incremental |

Las grabaciones permanecen en el disco local y se sirven desde ahí; por defecto nada se sube
a la nube. Los segmentos bloqueados como evidencia nunca son eliminados por la retención.
Los tres límites también son **configurables en tiempo de ejecución sin reiniciar** — desde la página
Sistema del panel de control (paneles "Límite de grabación" y "Límite de base de datos") o mediante
`GET`/`PUT /api/v1/system/retention` y `GET`/`PUT /api/v1/system/db`
(PUT es solo para administradores); un valor almacenado tiene prioridad sobre el valor del entorno, que sigue siendo el predeterminado.
Una BD creada antes de que existiera el límite se convierte a auto_vacuum incremental en
segundo plano al arrancar (`HELDAR_DB_AUTOVACUUM_CONVERT`, por defecto `true`; nunca bloquea el
arranque) o bajo demanda mediante `POST /api/v1/system/db/convert`.

Las credenciales de las cámaras residen en esta BD SQLite. Establece `HELDAR_SECRET_KEY` (base64 de 32
bytes, p. ej. `openssl rand -base64 32`) para cifrarlas en reposo con AES-256-GCM;
si no se define, se mantienen en texto plano para un appliance LAN de confianza. Las credenciales en texto plano existentes se
cifran en el siguiente arranque — véase la
[guía de hardening en producción](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md).

## CORS

`HELDAR_CORS_ORIGINS` controla el acceso de origen cruzado. Vacío o `*` permite todos los
orígenes; de lo contrario, restringe a la lista configurada (el valor por defecto permite el
servidor de desarrollo Vite). En un despliegue de binario único el panel de control es del mismo origen, por lo que
CORS es relevante principalmente cuando un frontend separado o una integración llama a la API.

## Operación de un despliegue

Para dimensionamiento, puesta en marcha, observabilidad y acceso remoto — incluida la configuración de
tu propio STUN/TURN mediante `HELDAR_WEBRTC_ICE_SERVERS` — véase el
hub de [Operación](../operate/index.md).
