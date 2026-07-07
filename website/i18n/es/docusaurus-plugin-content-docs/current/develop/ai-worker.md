---
id: ai-worker
title: Crear un AI Worker
sidebar_label: Crear un AI Worker
sidebar_position: 2
---

# Crear un AI worker

Un AI worker de Heldar es cualquier proceso capaz de comunicarse con cuatro endpoints HTTP. El kernel
muestrea fotogramas y almacena los resultados; tu worker es un cliente HTTP puro que
descubre trabajo, obtiene fotogramas, ejecuta un modelo y publica las detecciones de vuelta. Dado que
el contrato es HTTP, un worker que falle o sea lento nunca podrá detener la ingesta ni la
grabación.

La implementación de referencia es
[`apps/ai/worker.py`](https://github.com/Straits-AI/heldar/blob/main/apps/ai/worker.py)
(un pequeño worker en Python con pocas dependencias). La guía de integración completa, incluidos
los analizadores de zonas y ANPR, está en
[docs/AI-WORKERS.md](https://github.com/Straits-AI/heldar/blob/main/docs/AI-WORKERS.md).

## El contrato

Todos los endpoints se encuentran bajo `/api/v1` y devuelven JSON, excepto el fotograma, que es
`image/jpeg`.

### 1. Descubrir trabajo - `GET /api/v1/ai/tasks`

Devuelve todas las tareas habilitadas en una cámara habilitada, cada una con la `frame_url`
para obtener. Esta es toda tu lista de trabajo. Vuelve a consultarla cada pocos segundos para detectar tareas
recién habilitadas o deshabilitadas.

```json
[
  {
    "id": "ai_3f2a9c1b...",
    "camera_id": "gate_a",
    "task_type": "detection",
    "stream_profile": "sub",
    "fps": 5.0,
    "width": 1280,
    "config": { "classes": ["person", "car"], "min_confidence": 0.4 },
    "frame_url": "/api/v1/cameras/gate_a/frame"
  }
]
```

`task_type` es una cadena de formato libre que tú defines; el kernel utiliza únicamente `fps` / `width` /
`enabled` para controlar el muestreador, y `config` es un blob opaco que interpreta tu worker.
El valor de `fps` aquí es la tasa solicitada; la tasa de muestreo efectiva
tras el presupuesto se informa mediante `GET /api/v1/ai/samplers`.

### 2. Obtener un fotograma - `GET /api/v1/cameras/{id}/frame`

Sirve el último JPEG muestreado para una cámara. Obténlo a tu propio ritmo,
normalmente a la tasa `fps` de la tarea o ligeramente por debajo.

```
200 OK
Content-Type: image/jpeg
Cache-Control: no-store
x-frame-age-ms: 142
x-frame-captured-at: 2026-06-13T08:15:31.120+00:00

<JPEG bytes>
```

- `x-frame-age-ms` - milisegundos transcurridos desde que se escribió el fotograma. Úsalo para omitir
  fotogramas obsoletos si el muestreador ha quedado sin conexión.
- `x-frame-captured-at` - la marca de tiempo de escritura. Deduplica sobre este valor para no
  reanalizar un fotograma sin cambios, y opcionalmente devuélvelo como `timestamp` de la detección
  para que las detecciones se alineen con el tiempo de captura.

Un `404` significa que aún no existe ningún fotograma (no hay ninguna tarea de IA habilitada para la cámara, o el
muestreador no ha producido su primer fotograma). Trátalo como un ciclo omitido, no como un
error.

### 3. Publicar resultados - `POST /api/v1/ai/events`

Publica un lote de detecciones para una cámara y tarea, opcionalmente con un único
evento derivado en la misma llamada.

```json
{
  "camera_id": "gate_a",
  "task_type": "detection",
  "timestamp": "2026-06-13T08:15:31.120Z",
  "detections": [
    {
      "label": "person",
      "confidence": 0.92,
      "bbox": [0.41, 0.30, 0.08, 0.22],
      "track_id": "t-17",
      "attributes": { "zone": "entry_lane_a" }
    }
  ],
  "event": {
    "event_type": "person_in_red_zone",
    "severity": "warning",
    "payload": { "zone": "red_a", "track_id": "t-17" }
  }
}
```

Reglas de los campos:

- `camera_id` es obligatorio y debe existir, en caso contrario devuelve `404`.
- `task_type` es obligatorio y se almacena en cada fila de detección.
- `timestamp` es RFC3339 opcional y se aplica a todo el lote; si se omite o no puede
  analizarse, el valor predeterminado es `now()` del servidor.
- `detections` es opcional (por defecto `[]`); envía `[]` para publicar solo un evento.
  Todos los campos dentro de una detección son opcionales.
- `event` es opcional. `event_type` es obligatorio cuando está presente; `severity` tiene
  como valor predeterminado `info` (usa `warning` o `critical` para activar el webhook de alerta); `payload`
  tiene como valor predeterminado un objeto vacío.

La respuesta es `{ "detections_ingested": N }`.

### 4. Estado del muestreador - `GET /api/v1/ai/samplers`

Estado del muestreador por cámara (`connecting` / `sampling` / `offline` / `error` /
`stopped`) y los fps efectivos presupuestados. Útil para paneles de control y para
confirmar que el kernel está produciendo fotogramas de forma efectiva.

## La convención bbox

`bbox` es `[x, y, w, h]` **normalizado a 0..1**, con origen en la esquina superior izquierda. La normalización
mantiene las detecciones independientes de la resolución, por lo que sobreviven a cualquier cambio posterior en el
`width` muestreado y se mapean directamente sobre polígonos de zona normalizados. El kernel almacena
el cuadro como JSON sin formato y no valida su forma, por lo que tu worker es responsable de
la corrección.

Una detección con `track_id` y `bbox` activa el motor de zonas del kernel
(su punto de suelo es el centro inferior del cuadro); las detecciones sin ellos se siguen
almacenando pero no pueden generar eventos de zona.

## El bucle del worker

```text
tasks = GET /api/v1/ai/tasks                 # refresh every few seconds
for each task (own thread / async task):
    loop at ~task.fps:
        resp = GET task.frame_url
        if resp is 404:        sleep, continue          # no frame yet
        if x-frame-captured-at == last_seen: continue   # unchanged frame; skip
        dets, event = analyze(task, resp.body)
        if dets or event:
            POST /api/v1/ai/events { camera_id, task_type, timestamp,
                                     detections: dets, event: event }
```

Dado que el fotograma servido es del último valor, obtenerlo más rápido de lo que escribe el muestreador
devuelve los mismos bytes; deduplica en `x-frame-captured-at`. Obtenerlo más lento simplemente
descarta fotogramas intermedios, lo cual está bien para la detección y el seguimiento a estas
tasas.

## Conectar un modelo

El worker de referencia define una clase base `Analyzer` y crea una instancia
por hilo de tarea, de modo que el estado por cámara (un fotograma anterior, un rastreador) vive en
`self`. Los analizadores se registran por `task_type`; un tipo desconocido recurre a un
marcador de posición que ejercita la ruta de fotogramas pero nunca fabrica detecciones. Un
`MotionAnalyzer` funcional y sin modelo está registrado para `task_type: "motion"`, por lo que
puedes validar la ruta completa del muestreador al worker y a los eventos sin ningún modelo ni
GPU.

Un detector real encaja como una subclase y una llamada `register(...)`, sin ningún
cambio en el kernel ni en el contrato HTTP:

```python
from worker import Analyzer, AnalysisResult, Detection, FrameContext, register

class YoloAnalyzer(Analyzer):
    name = "yolo"
    def __init__(self, config, log):
        super().__init__(config, log)
        from ultralytics import YOLO
        self.model = YOLO(config.get("weights", "yolov8n.pt"))
        self.conf = float(config.get("threshold", 0.25))

    def analyze(self, frame: FrameContext) -> AnalysisResult:
        img = frame.image(); w, h = img.size
        dets = []
        for r in self.model(img, conf=self.conf, verbose=False):
            for b in r.boxes:
                x1, y1, x2, y2 = b.xyxy[0].tolist()
                dets.append(Detection(
                    label=self.model.names[int(b.cls)],
                    confidence=float(b.conf),
                    bbox=[x1/w, y1/h, (x2-x1)/w, (y2-y1)/h]))  # normalized 0..1
        return AnalysisResult(detections=dets)

register("detection", YoloAnalyzer)   # replaces the placeholder for "detection"
```

El kernel nunca toca tu modelo. Solo enruta los resultados a los consumidores por
`task_type`: los resultados de `detection` con IDs de seguimiento activan el motor de zonas, los resultados de `anpr`
alimentan el motor de control de acceso, y así sucesivamente. Para añadir una nueva tubería, elige un
nuevo `task_type`, publica sus resultados y escribe un consumidor para él (consulta
[Crear un módulo](./build-a-module.md)).

## Ejecutar el worker de referencia

```bash
cd apps/ai
python3 -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
HELDAR_API=http://localhost:8000 python worker.py
# or: python worker.py --api http://localhost:8000 --log-format json
```

La configuración del lado del worker (indicador CLI o variable de entorno) cubre la URL base de la API
(`--api` / `HELDAR_API`), el intervalo de sondeo de tareas
(`--poll-interval` / `HELDAR_AI_POLL_INTERVAL`), y los parámetros de HTTP, retroceso exponencial y registro.
La tabla completa está en
[apps/ai/README.md](https://github.com/Straits-AI/heldar/blob/main/apps/ai/README.md).
