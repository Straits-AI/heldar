---
id: build-a-module
title: Crear un módulo
sidebar_label: Crear un módulo
sidebar_position: 1
---

# Crear un módulo

Un módulo extiende Heldar con sus propias tablas, rutas, interfaz de usuario y lógica de percepción. Hay **dos maneras
de crearlo**, y la elección depende de qué tan profundamente te integres y en qué lenguaje trabajes:

- **Crate de aplicación compilado en proceso (esta guía).** Un crate de Rust que depende de `heldar-kernel` y es
  enlazado por un binario compositor. Comparte el pool de SQLite del kernel y se conecta a la juntura de ingesta
  `DetectionConsumer` — la integración más estrecha, para aplicaciones de primera parte. Los crates abiertos
  `heldar-entry`/`movement`/`search` se construyen de esta manera.
- **Plugin sidecar fuera de proceso** ([guía aparte](./sidecar-plugins.md)). Cualquier servicio HTTP en cualquier
  lenguaje al que Heldar hace proxy inverso y al que envía eventos, **instalado en tiempo de ejecución** sin
  necesidad de recompilar. Aislado por proceso/contenedor, con mínimos privilegios. Este es el camino para
  plugins de terceros y propios, y lo que instala la página de **Plugins**.

El resto de esta página describe el camino en proceso. Una aplicación añade tablas, rutas y lógica de percepción
sin que el kernel sepa que existe. Dependes de `heldar-kernel`; un binario compositor te enlaza.
(Su página del panel de control se sigue entregando por separado como un bundle cargado en tiempo de ejecución — ver paso 6.)

A lo largo de esta guía, la referencia trabajada es la aplicación de control de acceso abierta,
[`heldar-entry`](https://github.com/Straits-AI/heldar/tree/main/crates/heldar-entry).
Es un ejemplo real y compilable de cada paso a continuación. Una aplicación en proceso también declara un
`manifest()` para aparecer en la navegación del panel de control (ver la sección "Manifest" de la guía de sidecar —
la forma es la misma; los módulos en proceso simplemente lo devuelven desde código en lugar de registrarlo en tiempo
de ejecución).

## El modelo mental

El kernel **no tiene ninguna dependencia en tu crate**. Las dependencias apuntan en un solo sentido: tu
crate depende de `heldar-kernel`, y el servidor compositor
([`heldar-server`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs))
depende de ambos y los conecta entre sí. Te conectas a través de cuatro junturas públicas:
un `DetectionConsumer`, un `Router<AppState>`, un esquema autoinstalado y el
primitivo de autenticación. Agregar tu aplicación es una adición en algunos puntos de composición del
servidor, nunca una edición al manejador de ingesta del kernel ni al router.

## 1. Un nuevo crate dependiendo del kernel

Crea un crate de biblioteca y añade la dependencia en `heldar-kernel`:

```toml
# crates/heldar-dwell/Cargo.toml
[package]
name = "heldar-dwell"
version = "0.1.0"
edition = "2021"

[dependencies]
heldar-kernel = "0.1"          # or a path/git dep during local development
axum = "0.8"
sqlx = { version = "0.8", features = ["sqlite", "runtime-tokio"] }
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
```

Tu `lib.rs` expone los módulos que el servidor utilizará: un consumidor, un
inicializador de esquema, un router y una configuración opcional. Sigue el modelo de
[`heldar-entry/src/lib.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/lib.rs).

## 2. Implementar un `DetectionConsumer`

Después de que se persiste un lote de detecciones del worker, el kernel lo distribuye a cada
consumidor registrado cuyo `interested_in(task_type)` devuelve `true`. Este es el
trait, de `heldar_kernel::services::consumer`:

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

Un consumidor mínimo que cuenta detecciones por cámara:

```rust
use std::sync::Arc;
use sqlx::SqlitePool;
use heldar_kernel::services::consumer::{DetectionBatch, DetectionConsumer};

pub struct DwellCounter {
    pool: SqlitePool,
}

impl DwellCounter {
    pub fn new(pool: SqlitePool) -> Arc<Self> {
        Arc::new(Self { pool })
    }
}

#[async_trait::async_trait]
impl DetectionConsumer for DwellCounter {
    fn name(&self) -> &'static str {
        "dwell_counter"
    }

    // Self-select on task_type. Return true for all types only if you are
    // genuinely task-agnostic. heldar-entry returns true only for "anpr".
    fn interested_in(&self, task_type: &str) -> bool {
        task_type.eq_ignore_ascii_case("detection")
    }

    async fn consume(&self, batch: &DetectionBatch<'_>) {
        // batch.camera_id / .site_id / .task_type / .timestamp are the context;
        // each det carries label, confidence, bbox ([x,y,w,h] 0..1), track_id,
        // and a free-form attributes blob. Write to YOUR tables on self.pool.
        for det in batch.detections {
            let _ = sqlx::query(
                "INSERT INTO dwell_counts (camera_id, label, ts)
                 VALUES (?, ?, ?)",
            )
            .bind(batch.camera_id)
            .bind(det.label.as_deref())
            .bind(batch.timestamp)
            .execute(&self.pool)
            .await;
        }
    }
}
```

`consume` no debe entrar en pánico; los errores son tuya responsabilidad registrarlos o ignorarlos. El kernel lo llama
de forma sincrónica después de que el lote es confirmado, así que mantenlo liviano, o envía el trabajo a una
cola o tu propio bucle de fondo. Para un consumidor completo de ejemplo, consulta el motor ANPR en
[`heldar-entry/src/anpr.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/anpr.rs)
(`impl DetectionConsumer for AnprEngine`).

## 3. Autoinstalar tu esquema con migraciones versionadas

Tu aplicación es dueña de sus tablas. Aplícalas contra el pool compartido al inicio con el
runner de migraciones de aplicación del kernel — versionado y de solo-añadir —, exactamente
como
[`heldar-entry/src/schema.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/schema.rs):

```rust
// src/schema.rs
use heldar_kernel::db::{run_app_migrations, AppMigration};
use sqlx::SqlitePool;

const MIGRATIONS: &[AppMigration] = &[AppMigration {
    version: 1,
    name: "init",
    sql: include_str!("../migrations/0001_init.sql"),
}];

pub async fn init(pool: &SqlitePool) -> anyhow::Result<()> {
    run_app_migrations(pool, "dwell", MIGRATIONS).await
}
```

```sql
-- migrations/0001_init.sql
CREATE TABLE IF NOT EXISTS dwell_counts (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    camera_id  TEXT NOT NULL,
    label      TEXT,
    ts         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dwell_cam_ts ON dwell_counts (camera_id, ts);
```

Cada migración se registra en `_heldar_app_migrations` bajo el nombre de tu componente
(`"dwell"` aquí) y se aplica exactamente una vez, de modo que las aplicaciones nunca
colisionan con el kernel ni entre sí en la BD compartida. Para cambiar el esquema más
adelante, añade un nuevo `migrations/NNNN_*.sql` y una línea a `MIGRATIONS` — **nunca
edites una migración ya publicada** (la guarda de checksum del runner la rechaza). El
kernel nunca define tus tablas de dominio; compartes su `SqlitePool` pero eres dueño
de tu esquema.

## 4. Exponer un `Router<AppState>` y fusionarlo

Tus manejadores se ejecutan contra el `AppState` del kernel, que te da el pool compartido
(`st.pool`), la configuración del kernel (`st.cfg`) y el recorder/sampler/cliente HTTP.
Usa rutas absolutas `/api/v1/...`; el servidor monta tu router en la raíz.
Reutiliza el primitivo de autenticación para autorización y auditoría, como
[`heldar-entry/src/routes.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-entry/src/routes.rs):

```rust
// src/routes.rs
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use heldar_kernel::auth::Principal;
use heldar_kernel::error::AppResult;
use heldar_kernel::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/dwell/summary", get(summary))
}

async fn summary(
    State(st): State<AppState>,
    principal: Principal,
) -> AppResult<Json<Value>> {
    // Capability check + audit come from the kernel auth primitive. When
    // HELDAR_AUTH_ENABLED is false this is a no-op (open appliance).
    principal.require(principal.can_view(), "view dwell summary")?;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dwell_counts")
        .fetch_one(&st.pool)
        .await?;
    Ok(Json(json!({ "total": total })))
}
```

## 5. Registrar tu consumidor y lanzar bucles en el servidor

Todo se une en el servidor compositor. En
[`heldar-server/src/main.rs`](https://github.com/Straits-AI/heldar/blob/main/crates/heldar-server/src/main.rs)
realizas cuatro pequeñas adiciones:

```rust
// (a) apply your schema, after the kernel migrations have run
heldar_dwell::schema::init(&pool).await.context("dwell schema init")?;

// (b) add your consumer to the consumer vector that goes into AppState
let consumers: Arc<Vec<Arc<dyn DetectionConsumer>>> = Arc::new(vec![
    services::zones::ZoneEngine::new(pool.clone(), cfg.clone(), recorder.clone()),
    heldar_entry::anpr::AnprEngine::new(pool.clone(), cfg.clone(), entry_cfg.clone()),
    heldar_dwell::DwellCounter::new(pool.clone()),   // <- your consumer
]);

// (c) merge your router next to the kernel and the other apps
let app = Router::new()
    .merge(routes::api_router())
    .merge(heldar_entry::routes::router())
    .merge(heldar_dwell::routes::router());          // <- your router

// (d) if you have a background loop, supervise it (respawns on panic)
let p = pool.clone();
spawn_supervised("dwell_rollup", move || heldar_dwell::rollup::run(p.clone()));
```

Eso es toda la integración. El vector de consumidores es distribuido por la ruta de ingesta del kernel
sin nombrar a ningún consumidor; la fusión del router es invisible para el router del kernel;
el esquema es propio tuyo; y `spawn_supervised` reinicia un bucle que entra en pánico después de 5 segundos.

## 6. Entregar una interfaz de panel de control (cargada en tiempo de ejecución)

La página del panel de control de tu módulo **no** se compila en la SPA — es un bundle ES cargado en tiempo de
ejecución que tu crate sirve, de modo que la imagen del panel permanece agnóstica al módulo (un único `heldar-web`
para las versiones abierta y completa). Ver
[Sistema de módulos → Interfaces de módulo cargadas en tiempo de ejecución](./module-system.md#runtime-loaded-module-uis) para el razonamiento.
Los pasos, siguiendo el modelo de `heldar-entry`/`search`:

1. **Crea la página** bajo `apps/web/src/modules/{id}/` (`page.tsx` + un `entry.tsx` que
   haga `export default` de ella). Importa React normalmente, e importa la superficie del shell — cliente API,
   autenticación, kit de UI, formateadores — desde **`@heldar/shell`** (nunca desde `../lib/*`); se resuelve a
   las instancias compartidas del shell en tiempo de ejecución. Un módulo que posee rutas de backend las llama con
   `request` + `qs` del SDK.
2. **Apunta el manifest hacia ella:** agrega `.with_runtime_ui("/api/v1/modules/{id}/ui/index.js")` a tu
   cadena de construcción de `manifest()`.
3. **Sirve el bundle construido** desde el router de tu crate: `const UI: &str = include_str!("../ui/{id}.js");`
   y un manejador `serve_ui` con permisos de visor en `GET /api/v1/modules/{id}/ui/index.js` (cópialo desde
   `heldar-search/src/routes.rs`, incluyendo la cabecera `Cache-Control: no-cache`).
4. **Construye e incrusta** con `make module-bundles` (Vite construye el bundle de biblioteca tratando `react` +
   `@heldar/shell` como externos, luego lo copia en el directorio `ui/` de tu crate). Vuelve a ejecutarlo tras
   cualquier edición de la interfaz.

Los módulos sin cabecera (sin página) simplemente omiten esto — omite `ui_url` y el almacén los lista con
un tratamiento de cómputo y sin opción de apertura.

## El kernel no tiene dependencia en tu crate

Este es el objetivo del diseño. Los bordes de dependencia son: `heldar-dwell` hacia
`heldar-kernel`, y `heldar-server` hacia ambos. El kernel nunca importa tu crate
y no adquiere ninguna rama para tu `task_type`. Eso es lo que permite que el kernel abierto y
las aplicaciones genéricas se distribuyan como crates separados, y lo que permite que los verticales
propietarios se compongan de la misma manera en una compilación privada sin tocar el código público.

## No toda aplicación es un consumidor

`heldar-movement` y `heldar-search` se construyen sobre las mismas junturas pero no están en
la ruta de ingesta caliente: movement son dos bucles de fondo supervisados (un proponente de
candidatos ReID y un motor de reglas de brecha), y search es una capa de consulta de solo lectura
sobre hechos del kernel ya almacenados. Ambos siguen siendo dueños de un esquema, exponen un router,
y (en el caso de movement) lanzan bucles, compuestos en el servidor exactamente como se describe arriba,
menos el registro del consumidor. Elige el patrón que se ajuste: un consumidor para la interpretación
por lote, un bucle para resúmenes periódicos, o un router simple para consultas de solo lectura.

¿Necesitas leer los hechos de otra aplicación? Consulta su vista de contrato `*_read` publicada
(`entry_events_read`, `breach_alerts_read`, …), nunca sus tablas base — la vista es el contrato
estable entre aplicaciones, de modo que la aplicación dueña puede evolucionar sus tablas sin
romperte. Y publica una vista `*_read` propia (como una migración) si otras aplicaciones deben
leerte.
