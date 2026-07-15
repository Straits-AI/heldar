---
id: architecture
title: Arquitectura
sidebar_label: Arquitectura
sidebar_position: 1
---

# Arquitectura

Heldar es un plano de control HTTP ligero (rutas Axum) sobre un conjunto de
servicios en segundo plano de larga duración, todos compartiendo un único almacén
SQLite y una única configuración. El núcleo (`heldar-kernel`) es
**agnóstico al dominio**: gestiona cámaras, ingesta y graba RTSP, muestrea
fotogramas para IA, acepta detecciones de los trabajadores, evalúa zonas
espaciales, y proporciona autenticación, retención y observabilidad. No sabe
nada sobre control de acceso, inteligencia de movimiento o búsqueda; esas son
aplicaciones.

Dos reglas dan forma a todo el sistema:

- **El núcleo es lo único que se comunica con las cámaras.** El grabador
  continuo (24/7) copia el flujo de bits comprimido al disco sin decodificar.
  Un muestreador con presupuesto fijo es el único componente que decodifica,
  escribiendo un JPEG actual por cámara. Un trabajador de IA lento o ausente
  nunca puede bloquear la ingesta ni la grabación.
- **El núcleo no tiene dependencia de ninguna aplicación.** Las aplicaciones
  dependen del núcleo y se enlazan mediante un binario compositor. Añadir una
  aplicación es una inserción en unos pocos puntos de composición, nunca una
  edición al núcleo.

Para el diseño completo por etapa (supervisor del grabador, indexador, barredor
de retención, motor de zonas, métricas y más), consulte
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
en el repositorio.

## Las cuatro juntas públicas

Las aplicaciones se conectan al núcleo únicamente a través de estas juntas.
Juntas permiten que una nueva aplicación añada tablas, rutas, lógica de
percepción y autorización sin que el núcleo la nombre en ningún momento.

### 1. El trait `DetectionConsumer`

Tras persistir un lote de detecciones de un trabajador, la ruta de ingesta del
núcleo lo distribuye a un registro de consumidores. Un consumidor declara de
forma autónoma qué `task_type`s le interesan, por lo que el núcleo nunca
acumula una rama `if task_type == "..."`.

```rust
pub struct DetectionBatch<'a> {
    pub camera_id: &'a str,
    pub site_id: Option<&'a str>,
    pub task_type: &'a str,
    pub detections: &'a [DetectionIngest],
    pub timestamp: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait DetectionConsumer: Send + Sync {
    fn name(&self) -> &'static str;
    fn interested_in(&self, task_type: &str) -> bool;
    async fn consume(&self, batch: &DetectionBatch<'_>);
}
```

El motor de zonas abierto (una primitiva espacial) y el motor de control de
acceso abierto (autorización de matrículas) son ambos consumidores. El motor de
zonas devuelve `true` para cualquier tipo de tarea con detecciones rastreadas;
el motor de control de acceso devuelve `true` solo para `anpr`.

### 2. Fusión de `Router<AppState>`

Cada aplicación expone su propio `Router<AppState>` de Axum con rutas absolutas
`/api/v1/...`. El servidor compositor fusiona esos enrutadores junto al
enrutador del núcleo; el enrutador del núcleo no los conoce. Desde `AppState`
un manejador de aplicación accede al pool SQLite compartido, la configuración
del núcleo, y el grabador/muestreador/cliente HTTP.

### 3. Esquema de autoinstalación

Cada aplicación posee sus propias tablas y las instala ella misma como migraciones
versionadas y de solo-añadir contra el pool compartido al iniciar
(`db::run_app_migrations`, registradas por componente en `_heldar_app_migrations`).
El núcleo no define tablas de dominio. El patrón es un `schema::init(pool)` que
ejecuta el array `MIGRATIONS` de la aplicación con sus archivos
`migrations/NNNN_*.sql` — para evolucionar un esquema se añade una migración
nueva, nunca se edita una ya publicada.

### 4. La primitiva de autenticación

El núcleo proporciona un extractor `Principal` más comprobaciones de capacidad
RBAC (`can_view`, `can_manage_registry`, `can_operate_gate`, entre otros) y un
ayudante de auditoría. Las aplicaciones reutilizan estos elementos para
autorización y auditoría en lugar de inventar los propios, de modo que un único
interruptor `HELDAR_AUTH_ENABLED` gobierna toda la superficie compuesta.

## Cómo se compone un despliegue

El servidor compositor (`heldar-server`) es donde el núcleo y un conjunto
elegido de aplicaciones se integran. Para cada aplicación: aplica el esquema de
la aplicación, construye los consumidores de detección de la aplicación y los
añade al vector de consumidores en `AppState`, fusiona el enrutador de la
aplicación, y lanza cualquier bucle en segundo plano bajo un supervisor que
relanza en caso de pánico. La compilación de referencia abierta compone solo el
núcleo más las aplicaciones genéricas Apache-2.0; un despliegue diferente enlaza
un conjunto diferente de crates aquí.

No todas las aplicaciones son un `DetectionConsumer`. Algunas aplicaciones son
bucles periódicos en segundo plano o capas de consulta de solo lectura sobre
hechos del núcleo ya almacenados; utilizan la misma composición de esquema +
enrutador + bucle sin situarse en la ruta caliente de ingesta.

Consulte [Construir un módulo](../develop/build-a-module.md) para un recorrido
paso a paso sobre cómo escribir uno, y [Open-core](./open-core.md) para el
límite entre abierto y propietario.
