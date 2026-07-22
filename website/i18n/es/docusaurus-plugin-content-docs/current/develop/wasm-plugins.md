---
id: wasm-plugins
title: Plugins Wasm
sidebar_label: Plugins Wasm
sidebar_position: 4
---

# Plugins Wasm

Un **plugin Wasm** es un [`DetectionConsumer`](./build-a-module.md) sin cabeza y con aislamiento de sandbox: después de que el kernel persiste un lote de detecciones, lo entrega al plugin (como JSON), el cual se ejecuta en un sandbox de WebAssembly con **autoridad ambiental cero** — sin sistema de archivos, red, reloj ni aleatoriedad — y emite eventos derivados de vuelta. Los eventos emitidos tienen espacio de nombres, están delimitados por cámara, tienen un límite máximo y se persisten a través del camino de eventos normal del kernel, por lo que fluyen hacia webhooks y sidecars de forma automática.

Es la herramienta especializada para **lógica de reglas/transformación ligera, fuertemente aislada y en proceso** en la ruta de ingesta activa. Para cualquier cosa con interfaz de usuario, múltiples lenguajes, o trabajo pesado/con estado, utiliza un [plugin sidecar](./sidecar-plugins.md) en su lugar — los sidecars obtienen una red y una clave con ámbito; un invitado Wasm no obtiene ninguno de los dos.

## Cómo encaja

| | Sidecar (Fase B) | Plugin Wasm (Fase D) |
| --- | --- | --- |
| Proceso | separado (cualquier lenguaje) | en proceso (sandbox) |
| UI | sí (iframe en `/m/{id}/`) | ninguna (sin cabeza) |
| Capacidad | clave API con ámbito + red | **ninguna** — cómputo puro |
| Ideal para | apps, interfaces, integraciones | reglas, filtros, eventos derivados |

El entorno de ejecución ([wasmi](https://github.com/wasmi-labs/wasmi), un intérprete de Rust puro) se distribuye detrás de una **`wasm` cargo feature desactivada por defecto** — el binario del dispositivo predeterminado nunca lo enlaza. Compila el servidor con `--features wasm` para habilitar la carga de plugins.

## El plugin

Un invitado es un módulo central `wasm32-unknown-unknown`. Exporta una ABI mínima y puede importar exactamente dos funciones del host (`heldar.log`, `heldar.emit_event`) — importar cualquier otra cosa (p. ej., WASI) falla al cargar, por lo que el sandbox está cerrado por construcción. La plantilla completa y lista para copiar es
[`examples/wasm-plugin`](https://github.com/Straits-AI/heldar/tree/main/examples/wasm-plugin); la única
parte que cambias es la función `rule()`:

```rust
fn rule(input: &Input) {
    let threshold = input.config.get("threshold").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
    let persons = input.detections.iter().filter(|d| d.label.as_deref() == Some("person")).count();
    if persons > threshold {
        emit(&Event {
            event_type: "occupancy.high".into(),
            severity: "warning".into(),
            payload: json!({ "persons": persons }),
        });
    }
}
```

El host llama a `heldar_describe()` una vez al cargar para leer `{ id, name, version, publisher, description,
interested_in }`, y luego `heldar_handle(ptr, len)` por lote (entrada JSON escrita en la memoria del invitado). Los eventos
que el invitado pasa a `emit_event` se almacenan en buffer y se persisten después de la llamada como
`wasm.{plugin_id}.{event_type}`, **siempre delimitados a la cámara del lote** (un invitado no puede falsificar eventos
para otra cámara) con la severidad limitada a `info`/`warning`/`critical`.

## Compilar + cargar

```bash
# 1. build the guest to wasm32 (the example)
cd examples/wasm-plugin
cargo build --release --target wasm32-unknown-unknown

# 2. drop it into the plugins directory
cp target/wasm32-unknown-unknown/release/heldar_occupancy_plugin.wasm \
   <data>/wasm-plugins/occupancy.wasm

# 3. run the server with the wasm feature
cargo run -p heldar-server --features wasm
```

Los plugins cargados aparecen en `GET /api/v1/modules` (montaje `headless`, sin ruta de navegación) y en la tienda de **Plugins**
con un tratamiento de *cómputo en sandbox*. La v1 carga al arrancar; cambiar plugins requiere un reinicio.

## Sandbox + límites

Cada invitado se ejecuta con límites estrictos, configurados mediante variables de entorno (leídas por el host del plugin):

| Variable de entorno | Predeterminado | Límites |
| --- | --- | --- |
| `HELDAR_WASM_ENABLED` | `true` | interruptor principal (con la feature `wasm` activada) |
| `HELDAR_WASM_PLUGINS_DIR` | `<data>/wasm-plugins` | de dónde se cargan los `*.wasm` |
| `HELDAR_WASM_FUEL` | `50000000` | presupuesto de instrucciones por llamada (límite de DoS de CPU — un bucle infinito provoca una trampa) |
| `HELDAR_WASM_MAX_MEMORY_MB` | `64` | límite de memoria lineal por llamada |
| `HELDAR_WASM_MAX_TABLE_ELEMENTS` | `100000` | límite de elementos de tabla por llamada (las tablas son RAM del host, no cubiertas por el límite de memoria) |
| `HELDAR_WASM_MAX_EVENTS` | `64` | eventos que un invitado puede emitir por llamada |
| `HELDAR_WASM_MAX_EVENT_BYTES` | `16384` | límite de bytes por evento |
| `HELDAR_WASM_MAX_LOG_CALLS` | `256` | llamadas a `heldar.log` por lote (limita una avalancha de registros) |
| `HELDAR_WASM_MAX_FAILURES` | `5` | fallos consecutivos antes de que el plugin se deshabilite automáticamente |

Una trampa del invitado, agotamiento de combustible, OOM o pánico quedan aislados — se registran y nunca bloquean el kernel,
y un plugin con fallos repetidos se desconecta automáticamente (deshabilitado + un evento `wasm_plugin_disabled`). Los invitados
se ejecutan en `spawn_blocking` para que el CPU de Wasm nunca bloquee el reactor asíncrono.

## Confianza + ámbito

La v1 carga plugins desde un **directorio local controlado por el operador** (de confianza del operador). El kernel nunca
descarga ni ejecuta `.wasm` remoto, y aún no existe firma por artefacto — esos aspectos, el
[Component Model](https://component-model.bytecodealliance.org/), WASI, el estado proporcionado por el host y los
SDKs multi-lenguaje son no-objetivos deliberados para la v1. Si más adelante ejecutas Wasm de terceros no confiable, la
ruta de actualización es el entorno de ejecución [wasmtime](https://wasmtime.dev/) (interrupción por época + un sandbox más reforzado)
detrás de la misma interfaz.
