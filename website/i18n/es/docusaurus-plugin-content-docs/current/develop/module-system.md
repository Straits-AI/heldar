---
id: module-system
title: Sistema de Módulos
sidebar_label: Sistema de Módulos
sidebar_position: 1
---

# Sistema de Módulos

Heldar está compuesto, no es monolítico: el kernel es agnóstico al dominio, y cada superficie de producto — control de acceso, movimiento, búsqueda, tus propias aplicaciones — es un **módulo** que se conecta a través de un pequeño conjunto de puntos de integración.
La navegación del panel de control y las rutas se construyen **en tiempo de ejecución** a partir de los módulos que expone el binario en ejecución, por lo que añadir uno nunca bifurca el núcleo.

Esta página es el mapa; las guías por tipo — [Construir un módulo](./build-a-module.md),
[Plugins sidecar](./sidecar-plugins.md), [Plugins Wasm](./wasm-plugins.md), [Registro](./registry.md) —
son el detalle.

## Los tres tipos

| Tipo | Proceso | UI (montaje) | Añadir sin… | Usar para |
|---|---|---|---|---|
| **En proceso** | crate Rust enlazado al kernel | un bundle ES de React que el crate sirve en `/api/v1/modules/{id}/ui`, importado por el panel **en tiempo de ejecución** (`mount: runtime`) | reconstruir el panel | aplicaciones de primer nivel que necesitan la ruta crítica + base de datos compartida (entry, movement, search, verticals) |
| **Sidecar** | servicio fuera de proceso (cualquier lenguaje) | un iframe en sandbox, con proxy inverso en `/m/{id}/*` (`mount: iframe`) | recompilar el kernel o el panel | aplicaciones de terceros / desplegadas de forma independiente |
| **Wasm** | en proceso, en sandbox (wasmi) | sin cabeza — sin página (`mount: headless`) | recompilar el kernel | cómputo no confiable sobre el flujo de detección |

- Los módulos **en proceso** se registran a través de los puntos de integración del kernel — un `DetectionConsumer`, una fusión `Router<AppState>`
  y un esquema auto-instalado versionado (`schema::init` sobre `db::run_app_migrations`) — y exponen un `manifest()` que contiene
  `mount: runtime` + una `ui_url`. Su UI **no** está compilada en el panel (ver más abajo).
  Ver [Construir un módulo](./build-a-module.md).
- Los plugins **sidecar** se registran en tiempo de ejecución vía `POST /api/v1/modules`: el kernel genera una clave de API con privilegios mínimos + una suscripción webhook para el plugin y realiza un proxy inverso de su UI + API bajo `/m/{id}/*`.
  Ver [Plugins sidecar](./sidecar-plugins.md).
- Los plugins **Wasm** se cargan desde un directorio (detrás de la característica `wasm`, desactivada por defecto) como
  `DetectionConsumer`s en sandbox. Ver [Plugins Wasm](./wasm-plugins.md).

## UIs de módulos cargadas en tiempo de ejecución

La página del panel de un módulo en proceso **no** está empaquetada en la SPA. Cada crate construye su página como un
bundle **de biblioteca** de Vite independiente (un módulo ES) y lo incrusta vía `include_str!`; el kernel lo sirve
en `GET /api/v1/modules/{id}/ui/index.js` (con acceso restringido a espectadores). El componente `ModuleHost` del panel lee la
`ui_url` del manifiesto, importa dinámicamente ese bundle con `import()`, y monta su componente React exportado por defecto.

El bundle **no** incluye su propio React ni kit de UI. Importa `react` y el SDK del shell
(**`@heldar/shell`** — el cliente de API, auth/sesión, sistema de diseño y formateadores) como *externos*; un
mapa de importación en el panel los resuelve a las instancias únicas del shell en tiempo de ejecución. Así, un módulo comparte
el React y el sistema de diseño del shell en lugar de duplicarlos — los bundles compilados son pequeños
(~10–50 KB) y siempre coinciden con el host.

Por qué importa: dado que ninguna UI de módulo está compilada en el panel, la SPA es **byte-idéntica para las
compilaciones open y full**. Existe una sola imagen `heldar-web` para ambas, y el generador del repositorio open elimina
la UI de un vertical propietario borrando su único directorio autocontenido — sin parcheo de código fuente por archivo. Un
módulo que no incluye página (p. ej., un plugin de cómputo sin cabeza) simplemente omite `ui_url`.

## Un manifiesto, compuesto en el arranque y en tiempo de ejecución

El panel renderiza su navegación de Módulos desde un único endpoint — **`GET /api/v1/modules`** — que fusiona
los tres tipos en una lista:

- **En el arranque**, el servidor compositor recopila los manifiestos de los módulos en proceso (cada uno con su
  `ui_url`) — y, en una compilación privada, cualquier vertical propietario a través de un punto de integración sin operación en open — más cualquier
  módulo wasm, y los almacena en el estado de la aplicación.
- **En tiempo de ejecución**, el manejador de la lista los une con los registros de **sidecar** de la base de datos
  (cada uno proyectado como manifiesto, con un campo de salud en vivo).

El panel **consulta `GET /api/v1/modules` cada 30 segundos**, por lo que instalar o eliminar un sidecar
aparece en la navegación sin recargar ni reiniciar. Un icono de módulo desconocido vuelve a un glifo genérico
— un icono faltante nunca es un error.

## El punto de integración de composición

Añadir una aplicación en proceso es un *push* en un solo lugar — el servidor compositor — no una edición al kernel:

```rust
// crates/heldar-server/src/main.rs (sketch)
let mut modules = vec![
    heldar_entry::manifest(),
    heldar_movement::manifest(),
    heldar_search::manifest(),
];
modules.extend(verticals::manifests());          // proprietary verticals — a no-op stub in the open build
let (wasm_consumers, wasm_modules) = wasm_plugins::load(/* … */);  // no-op when the `wasm` feature is off
```

Los puntos de integración `verticals` y `wasm_plugins` son la forma en que el código *opcional* se compone sin que el kernel lo referencie nunca: en la compilación open ambos son stubs que no devuelven nada; una compilación privada (o
`--features wasm`) intercambia la implementación real. `main.rs` es byte-idéntico entre los repositorios open y privado
— ver [Open-core](../concepts/open-core.md).

## Salud y estado

Los sidecars reportan salud en `GET /heldar/health`, que el kernel sondea cada 30s; la tienda muestra cada uno
como `healthy` / `unreachable` / `unknown`. El [registro](./registry.md) calcula el
**shelf** (core / proprietary / community / compute) y el **state** (`included` / `available` /
`installed` / `unreachable` / `not-in-build`) de cada entrada del catálogo cruzando el conjunto en proceso (enlazado al kernel)
con los registros en vivo — así la tienda refleja lo que este binario realmente enlaza más lo que está
instalado en este momento.

## Módulos sobre acceso remoto

El [panel remoto](../getting-started/remote-access.md) ejecuta la SPA completa sobre el relay, por lo que los módulos
funcionan de forma remota — con un matiz por tipo:

- Los módulos **en proceso** sirven su bundle de UI en `/api/v1/modules/{id}/ui` y realizan sus llamadas a la API bajo
  `/api/v1/*` — ambas viajan por el relay, por lo que el panel los carga *y* ejecuta de forma remota sin infraestructura adicional
  (así es como un módulo **propietario** llega a un operador remoto sin incluirse nunca en la imagen open). ✅
- Los iframes **sidecar** realizan proxy inverso en `/m/{id}/*`, que el relay reenvía al box (el kernel entonces
  llega al sidecar con su propia clave generada — nunca la del usuario). ✅
- Los módulos **Wasm** son sin cabeza + en proceso — nada que transmitir por relay. ✅

El relay es un tubo con lista de permitidos (`/api/v1/*`, `/media/*`, `/m/*`; el traversal de rutas y las rutas internas del Worker
se rechazan) y el box ejecuta su **propio** RBAC en cada solicitud reenviada, por lo que el acceso remoto nunca
amplía lo que un rol puede hacer.
