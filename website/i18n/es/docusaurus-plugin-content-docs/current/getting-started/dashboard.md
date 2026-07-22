---
id: dashboard
title: Uso del panel de control
sidebar_label: Uso del panel de control
sidebar_position: 4
---

# Uso del panel de control

El panel de control es la interfaz web React (`apps/web`) que el servidor aloja junto a la API — un solo binario, una sola URL
(consulte [Despliegue](./deploy.md)). Es la misma interfaz localmente y [de forma remota](./remote-access.md); el RBAC controla
lo que cada rol puede hacer.

## Inicio de sesión

Con `HELDAR_AUTH_ENABLED=false` (el valor predeterminado del dispositivo en LAN) el panel de control se abre directamente en la
cuadrícula de cámaras con acceso completo. Con la autenticación activada aparece una pantalla de **inicio de sesión** (usuario + contraseña, más un desafío opcional de Cloudflare Turnstile); su sesión es una cookie HttpOnly y aparece un control de cierre de sesión una vez que ha entrado. Cinco roles controlan los controles que ve — **admin**, **manager**, **guard**, **viewer**,
**integration** — que se corresponden con el RBAC del lado del servidor (el bloqueo de evidencias y los cambios de configuración son de manager+,
la administración de usuarios/webhooks es solo para administradores, los viewers son de solo lectura).

Una **barra de telemetría** en la parte superior es siempre visible: estado del núcleo (en línea/fuera de línea), recuento de grabaciones,
conteos de cámaras/grabadores/segmentos, un medidor de almacenamiento, tiempo de actividad y el reloj.

## Las páginas

**Vista de cámaras** (`/`) — la cuadrícula en vivo de múltiples cámaras y su pantalla de inicio. Diseño adaptativo automático o fijo
(1×1 … 4×4), estadísticas agregadas (total / grabando / fuera de línea / error) y acceso con un clic a cualquier cámara.

**Detalle de cámara** (`/cameras/:id`) — la vista del operador para una cámara: vídeo **en directo** de baja latencia
(WebRTC/WHEP, fallback a HLS) y una **línea de tiempo grabada** que puede desplazar con un clic (1h / 6h / 24h / 3d). Desde
aquí puede capturar instantáneas, exportar clips de evidencia (MP4), bloquear segmentos como evidencia, activar una grabación manual
(modo de evento), alternar **directo en caliente** (mantener el flujo en directo de esta cámara siempre activo para una reproducción
instantánea; el valor por defecto es arranque bajo demanda con cierre por inactividad) y gestionar las **tareas de IA** y las **zonas** de la cámara (a continuación). Los paneles muestran segmentos y eventos recientes, telemetría de estado (FPS, tasa de bits, reconexiones), PTZ, el horario de grabación y el relleno de huecos ANR.

**Reproducción** (`/playback`) — revisión sincronizada de múltiples cámaras: elija cámaras y una ventana de tiempo, luego
desplácese por todas ellas con un reloj maestro compartido de reproducir/pausar/buscar y velocidad (0.5× … 4×).

**Añadir cámara** (`/cameras/new`) — registrar una cámara. Elija un proveedor (Hikvision / Dahua / Genérico); la
URL RTSP se construye a partir de la plantilla del proveedor (usted proporciona la dirección y las credenciales) o se indica explícitamente para
Genérico. Establezca el modo de grabación (continuo / programado / evento / programado+evento), el flujo, la duración del segmento,
la retención, la cuota de almacenamiento por cámara, el directo en caliente y la pre/post-grabación para los modos de evento.

**Descubrir** (`/discover`) — escanear un rango de IP en busca de cámaras RTSP (y ONVIF), identificar proveedor/modelo a partir del
banner, verificar credenciales y saltar a Añadir cámara con los campos prellenados (manager+).

**IA** (`/ai`) — la consola de percepción: una vista de solo lectura de los **muestreadores** de fotogramas por cámara (estado +
FPS efectivo) y las **tareas de IA** habilitadas, además del modelo compartido de presupuesto de FPS / contrapresión. Las tareas se
crean y ajustan en la página de detalle de la cámara.

**Incidentes** (`/incidents`) — gestión de casos de evidencia: segmentos agrupados por un ID de incidente de forma libre,
con reproducción, bloqueo/desbloqueo de evidencia y reetiquetado a otro caso (manager+). Las grabaciones bloqueadas como evidencia nunca
son eliminadas por la retención.

**Copia de seguridad** (`/backup`) — **destinos** de copia de seguridad (local / SFTP / FTP / S3) y **políticas** (cámaras ×
destino × intervalo), un registro de trabajos con estado y **exportación de archivo** bajo demanda (elija cámaras + un
rango de fechas, descargue un `.zip`).

**Complementos** (`/plugins`) — la tienda de complementos: examine el registro combinado (integrado + remoto), instale o
desinstale módulos y sidecars (manager+), y consulte las insignias verificadas/firmadas y las credenciales de webhook por complemento.

**Sistema** (`/system`) — estado y observabilidad: el panel de almacenamiento (medidor de disco, horizonte de llenado proyectado,
tasa de escritura), los controles de **límite de grabación** (límite de tamaño + umbral de disco libre, ajustable en vivo — admin+),
los controles de **límite de base de datos** (el límite de la BD de metadatos `heldar.db` + la conversión única de auto_vacuum — admin+),
el selector del motor de **transcodificación en vivo** (software / VAAPI / NVENC, con los codificadores hardware detectados;
se aplica de inmediato a las nuevas sesiones en directo — admin+),
estado por cámara, el feed de eventos recientes, el **registro de auditoría** inmutable (manager+), configuración masiva de cámaras
y el panel de webhooks (admin+).

## Módulos de dominio

Más allá de las páginas de la plataforma, el panel de control muestra una sección de **Módulos** a partir de las aplicaciones que el
binario en ejecución compone (`GET /api/v1/modules`) — la navegación se actualiza automáticamente, sin necesidad de recompilar. Las aplicaciones genéricas de código abierto añaden:

- **Entry** — la consola de control de acceso (autorización de matrículas, el registro de visitantes/vehículos/listas de vigilancia,
  el flujo de trabajo de confirmación/rechazo del guardia, informes).
- **Movement** — inteligencia de movimiento entre cámaras (candidatos de ReID revisados por humanos —con una señal `appearance_score` de similitud CLIP opcional y desactivada por defecto, secundaria al ancla de matrícula—, incidentes de incursión en zonas restringidas).
- **Search** — consulta en lenguaje natural sobre los hechos de eventos almacenados, con una cadena de pruebas para cada respuesta. La búsqueda también incluye recuperación semántica (texto e imagen) sobre los recortes de detección almacenados, presentada como una pestaña **Semantic** (`POST /api/v1/search/semantic`).

Cada uno tiene una guía de operador en el centro [Operate](../operate/index.md). Los complementos sidecar se montan como
iframes en sandbox bajo `/m/{id}/` (consulte [Complementos sidecar](../develop/sidecar-plugins.md)).

## Zonas y tareas de IA

En la página de detalle de una cámara:

- **Tareas de IA** — cree tareas de detección por cámara (`task_type`, `fps` solicitado, `width` de muestra y una
  `config` de forma libre que lee el trabajador). Habilitar la primera tarea inicia el muestreador de fotogramas de esa cámara; el
  [trabajador de IA](../develop/ai-worker.md) de referencia extrae fotogramas y publica detecciones de vuelta.
- **Zonas** — dibuje regiones poligonales (normalizadas 0..1, que coinciden con los cuadros de detección), establezca las `labels` que
  cuentan, un umbral `dwell_seconds` y una `severity`. Las detecciones rastreadas que cruzan una zona generan
  eventos `enter` / `exit` / `dwell` con un fotograma de evidencia, que alimentan las alertas.

## Relacionado

- [Inicio rápido](./quickstart.md) — ponga en marcha el sistema y añada su primera cámara.
- [Acceso remoto](./remote-access.md) — el mismo panel de control desde cualquier lugar, en un navegador.
- [Despliegue](./deploy.md) — cómo el servidor sirve la SPA (un solo binario, una sola URL).
