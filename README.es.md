[English](README.md) · [简体中文](README.zh-Hans.md) · **Español**

# Heldar Core

Heldar es un sistema operativo de inteligencia visual de eventos para espacios físicos. Convierte flujos de cámara en eventos estructurados, los eventos en flujos de trabajo y los flujos de trabajo en inteligencia operacional. En lugar de envolver un DVR/NVR existente o partir de funciones de IA, construye primero su propio **núcleo multimedia** (registro de cámaras, ingesta RTSP, grabación, reproducción, vista en vivo) y luego agrega percepción, un motor de eventos y aplicaciones encima como *consumidores*. Poseer el núcleo significa poseer el modelo de metadatos, el motor de eventos y la lógica del producto, sin reimplementar códecs (FFmpeg y MediaMTX se encargan del trabajo multimedia de bajo nivel).

La plataforma es **open-core**: un núcleo Apache-2.0 más aplicaciones de referencia genéricas, con productos verticales y de cliente como crates propietarias separadas. Consulta [LICENSING.md](./LICENSING.md).

## Documentación

La documentación completa se encuentra en **https://heldar.swmengappdev.workers.dev/docs/**. Abarca el inicio rápido, el despliegue, la arquitectura y sus interfaces públicas, el límite open-core y las guías para construir tu propia aplicación o worker de IA sobre el núcleo.

Referencias en el repositorio: [ARCHITECTURE.md](./ARCHITECTURE.md) (las interfaces del núcleo y el diseño de cada etapa), [ROADMAP.md](./ROADMAP.md) (estado de las etapas), [LICENSING.md](./LICENSING.md) (el límite open-core) y las guías para operadores e integradores en [`docs/`](./docs).

## Inicio rápido

**La forma más rápida — Docker (descarga y ejecuta):**

```bash
curl -fsSL https://heldar.swmengappdev.workers.dev/install.sh | sh
# ¿ya tienes el repositorio? simplemente:  docker compose -f deploy/compose.yml up -d
```

Descarga las imágenes **OPEN** preconstruidas (núcleo + aplicaciones genéricas) e inicia MediaMTX + core + web — el panel de control queda disponible en `http://localhost:8080`. Agrega el worker de IA de referencia con `--profile ai`; actualiza con `docker compose pull`. Para producción (imagen completa privada, autenticación, secretos, TLS) usa la superposición `docker compose -f deploy/compose.yml -f deploy/compose.prod.yml up -d` — consulta [`docs/PRODUCTION.md`](docs/PRODUCTION.md). Para un DVR/dispositivo grabado, usa la imagen nativa con systemd (`make appliance-image`, [`infra/systemd/`](infra/systemd/)).

**Compilar desde el código fuente:**

**Requisitos previos:** Rust (vía `rustup`), FFmpeg + ffprobe en el `PATH`, `curl`. Node.js para el panel de control; Python 3 para el worker de IA.

```bash
rustup update                        # el proyecto sigue el último stable
cargo build --workspace
cp .env.example .env                 # los valores predeterminados funcionan de inmediato; nunca confirmes .env
scripts/setup_mediamtx.sh            # descarga la puerta de enlace de vista en vivo MediaMTX
scripts/run_stack.sh                 # MediaMTX + core (http://localhost:8000) + web (Vite en :5173)
```

El core sirve el panel de control compilado en `http://localhost:8000` cuando `HELDAR_WEB_DIR` apunta a `apps/web/dist` (un único binario, una única URL). `scripts/run_stack.sh` también ejecuta el servidor de desarrollo Vite en `http://localhost:5173` para el trabajo de frontend.

**Acceso remoto** (desde cualquier red, sin aplicación, incluso detrás de CGNAT): el dispositivo establece una conexión SALIENTE hacia un punto de encuentro WebRTC y el panel de control completo se ejecuta en un navegador — vista en vivo de múltiples cámaras, reproducción grabada y configuración — con un modelo de autenticación de dos niveles donde el núcleo permanece como la única autoridad RBAC. Configuración opcional y diseño: [`docs/REMOTE-ACCESS.md`](docs/REMOTE-ACCESS.md); refuerzo para internet público (autenticación, TLS, secretos, bloqueo, cifrado de credenciales, Turnstile): [`docs/PRODUCTION.md`](docs/PRODUCTION.md).

**Búsqueda semántica**: encuentra las grabaciones almacenadas por su significado — escribe una descripción («camioneta pickup roja») o suelta una foto y obtén recortes de detección ordenados por similitud, con salto directo a la reproducción. El ranking se basa en embeddings CLIP calculados por el worker de IA (extra opcional `requirements-embed.txt`) — totalmente local, sin nube: [`docs/SEARCH.md`](docs/SEARCH.md).

Añade una cámara (tú proporcionas la dirección y las credenciales; la URL RTSP se construye a partir de la plantilla del fabricante):

```bash
curl -X POST http://localhost:8000/api/v1/cameras -H 'content-type: application/json' -d '{
  "id":"gate_a","name":"Gate A","vendor":"hikvision",
  "address":"192.168.0.2","username":"admin","password":"YOUR_PASSWORD"}'

curl http://localhost:8000/api/v1/system                     # tiempo de actividad, conteos de cámaras/segmentos
curl http://localhost:8000/api/v1/cameras/gate_a/timeline    # rangos grabados
curl http://localhost:8000/api/v1/system/retention           # límite de tamaño de grabación + umbral de disco libre
```

> No fuerces por fuerza bruta las credenciales de la cámara. Los dispositivos HikVision bloquean el acceso tras intentos fallidos.

> **Grabaciones acotadas.** El barredor de retención evita que las grabaciones llenen el disco: un límite de tamaño (`HELDAR_MAX_RECORDINGS_GB`, predeterminado 20) y un umbral mínimo de disco libre (`HELDAR_MIN_FREE_DISK_GB`, predeterminado 5), eliminando primero los más antiguos (los clips bloqueados como evidencia nunca se eliminan). Ambos son configurables en tiempo de ejecución mediante `GET`/`PUT /api/v1/system/retention` (PUT solo para administradores) y la página del Sistema en el panel de control — sin necesidad de reiniciar.

Ejecuta el worker de IA de referencia contra una cámara con IA habilitada:

```bash
cd apps/ai && python3 -m venv .venv && .venv/bin/pip install -r requirements.txt
HELDAR_API=http://localhost:8000 .venv/bin/python worker.py
```

Consulta el [Inicio rápido](https://heldar.swmengappdev.workers.dev/docs/getting-started/quickstart) para habilitar tareas de detección, dibujar zonas y configurar alertas.

### Puertos predeterminados

| Puerto | Servicio |
| --- | --- |
| 8000 | API HTTP de Heldar Core + panel de control |
| 5173 | Panel de control web (servidor de desarrollo Vite) |
| 8554 / 8888 / 8889 | MediaMTX RTSP / HLS / WebRTC |
| 9997 | API de control de MediaMTX (loopback) |
