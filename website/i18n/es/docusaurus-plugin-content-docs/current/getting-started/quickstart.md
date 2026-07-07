---
id: quickstart
title: Inicio rápido
sidebar_label: Inicio rápido
sidebar_position: 1
---

# Inicio rápido

Levanta Heldar localmente, incorpora una cámara real y ejecuta el worker de IA de referencia.

## Lo más rápido: Docker en una línea

Si tienes Docker, omite la compilación por completo — descarga las imágenes abiertas preconstruidas e inicia el stack:

```bash
curl -fsSL https://heldar.swmengappdev.workers.dev/install.sh | sh
```

Escribe `~/heldar/{compose.yml,mediamtx.yml,.env}` e inicia MediaMTX + core + web; el dashboard
estará entonces en `http://localhost:8080`. (¿Ya clonaste el repositorio? `docker compose -f deploy/compose.yml up -d`
hace lo mismo.) Agrega el worker de IA de referencia con `docker compose --profile ai up -d`, actualiza con
`docker compose pull`. Para compilar desde el código fuente, continúa a continuación.

## Requisitos previos

- **Rust** (mediante `rustup`) — el proyecto sigue la última versión estable.
- **FFmpeg + ffprobe** en el `PATH` — grabar, recortar, capturar instantáneas y muestrear fotogramas
  requieren que estén disponibles. El servidor realiza una verificación previa de los binarios de medios
  al arrancar y falla rápidamente si no están presentes.
- **curl** para las llamadas a la API que se muestran a continuación.
- **Node.js** para el dashboard React (`apps/web`).
- **Python 3** para el worker de IA (`apps/ai`).

## Compilar y ejecutar

```bash
rustup update
cargo build --workspace
cp .env.example .env                 # defaults work out of the box; never commit .env
scripts/setup_mediamtx.sh            # fetch the MediaMTX live-view gateway
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + Vite dashboard
```

`scripts/run_stack.sh` inicia tres procesos: la pasarela de visualización en vivo MediaMTX,
el servidor Heldar Core en `http://localhost:8000` y el servidor de desarrollo Vite para
el dashboard en `http://localhost:5173`.

### Dos formas de ver el dashboard

- **Binario único (una sola URL).** Compila el dashboard y apunta el servidor hacia él:

  ```bash
  cd apps/web && npm install && npm run build      # writes apps/web/dist
  ```

  Establece `HELDAR_WEB_DIR=./apps/web/dist` en `.env`. El core luego sirve la SPA
  en `http://localhost:8000` junto con la API. Las rutas `/api/*`, `/media/*`,
  `/healthz`, `/readyz` y `/metrics` mantienen la precedencia; todo lo demás
  recurre a la SPA para que los enlaces profundos con enrutamiento del cliente funcionen. Consulta
  [Despliegue](./deploy.md).
- **Servidor de desarrollo Vite (recarga en caliente).** `scripts/run_stack.sh` ejecuta `npm run dev`
  y sirve el dashboard en `http://localhost:5173`, comunicándose con la API en
  `:8000`. Usa esto mientras desarrollas el frontend.

## Agregar una cámara

La URL RTSP se construye a partir de la plantilla del fabricante, por lo que solo necesitas proporcionar la dirección
y las credenciales:

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # uptime, camera/segment counts
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # recorded ranges
```

El grabador genera un proceso FFmpeg sin decodificación por cada cámara grabable y
comienza a escribir segmentos. El indexador convierte los archivos de segmento cerrados en filas
de línea de tiempo unos segundos después.

> No realices ataques de fuerza bruta sobre las credenciales de las cámaras. Los dispositivos HikVision bloquean el acceso tras intentos fallidos.

## Ejecutar el worker de IA

La percepción se ejecuta en un proceso worker separado que extrae fotogramas muestreados a través de
HTTP. Primero habilita una tarea de detección en la cámara (mediante el dashboard o con
la API):

```bash
curl -X POST http://localhost:8000/api/v1/cameras/gate_a/ai-tasks \
  -H 'content-type: application/json' \
  -d '{"task_type":"detection","fps":5,"width":1280,"enabled":true}'
```

Al habilitar la primera tarea se inicia un muestreador de fotogramas para esa cámara. Luego ejecuta el
worker de referencia:

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
```

El worker descubre las tareas, extrae el último fotograma de cada cámara, ejecuta su analizador
y publica las detecciones de vuelta en `POST /api/v1/ai/events`. Consulta
[Construir un worker de IA](../develop/ai-worker.md) para conocer el contrato completo. Para validar
todo el flujo desde el muestreador hasta el worker y los eventos sin modelo ni GPU, crea una
tarea con `task_type: "motion"` en su lugar — el worker de referencia incluye un analizador de
diferencia de fotogramas funcional para ello.

## Configurar detección, zonas y alertas en la interfaz

Con una cámara y una tarea de IA en ejecución, usa el dashboard para:

- **Detección** — crea o ajusta tareas de IA por cámara (`task_type`, `fps` solicitado,
  `width` de muestra y un blob `config` de forma libre que lee el worker).
- **Zonas** — dibuja regiones poligonales en una cámara. Las coordenadas están normalizadas
  entre 0 y 1, correspondiendo a los cuadros de detección. Establece `labels` para filtrar qué detecciones cuentan,
  `dwell_seconds` para activar una alerta de permanencia, y una `severity`. Las detecciones rastreadas
  que cruzan una zona generan eventos de zona `enter` / `exit` / `dwell` con un fotograma de evidencia.
- **Alertas** — apunta el notificador de alertas a un webhook (`HELDAR_ALERT_WEBHOOK_URL`
  o la interfaz). Los eventos `warning` y `critical`, incluidos los eventos de zona y
  los eventos publicados por el worker, se envían a él.

## Siguientes pasos

- [Despliegue](./deploy.md) para la disposición de producción con binario único.
- [Arquitectura](../concepts/architecture.md) para entender cómo encajan el kernel y las apps.
