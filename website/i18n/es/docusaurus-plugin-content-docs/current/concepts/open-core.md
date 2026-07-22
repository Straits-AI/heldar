---
id: open-core
title: Open-core
sidebar_label: Open-core
sidebar_position: 2
---

# Open-core

Heldar se distribuye como una plataforma open-core: un kernel Apache-2.0 y un
conjunto de aplicaciones de referencia genéricas son públicos, mientras que los
productos verticales y específicos para clientes son crates propietarios
separados en un repositorio privado. La división se aplica en el límite del
crate, no mediante indicadores de características dentro de una única base de
código.

## Qué es abierto (Apache-2.0)

El repositorio público `heldar` contiene:

- **`heldar-kernel`** - la plataforma agnóstica al dominio: media/DVR, ingesta
  de percepción y el muestreador de fotogramas, el motor de zonas, auth/RBAC,
  observabilidad, retención y las interfaces públicas (el trait
  `DetectionConsumer`, la combinación `Router<AppState>`, el patrón de esquema
  auto-instalado y el primitivo de autenticación).
- **Aplicaciones de referencia genéricas**, cada una una aplicación real
  construida únicamente sobre las interfaces públicas del kernel:
  - **`heldar-entry`** - control de acceso genérico. Autorización de matrículas
    (un `DetectionConsumer`), un registro de vehículos/visitantes/listas de
    seguimiento, un flujo de trabajo de confirmación/rechazo para guardias e
    informes de entradas/excepciones/auditorías. Neutral al dominio: cualquier
    despliegue de acceso controlado lo usa tal cual.
  - **`heldar-movement`** - correlación entre cámaras. Un proponente de
    candidatos ReID multi-señal y un motor de reglas de violación de zonas
    restringidas, ambos ejecutándose como bucles supervisados en segundo plano
    sobre hechos del kernel ya almacenados. Un `appearance_score` opcional y
    desactivado por defecto (similitud CLIP de recortes) puede añadir una señal
    de ReID aditiva, secundaria al ancla de matrícula.
  - **`heldar-search`** - búsqueda semántica. Una capa de consulta de solo
    lectura que convierte una pregunta en lenguaje natural en un plan
    estructurado, lo ejecuta de forma determinista sobre hechos de eventos
    almacenados y devuelve las filas como respuesta. Ahora incluye también
    recuperación por similitud CLIP (consultas de texto e imagen) sobre
    embeddings de recortes almacenados en el kernel — sigue siendo de solo
    lectura, sin respuestas generadas por el modelo.
- **`heldar-server`** - el binario de composición de referencia que enlaza el
  kernel y las aplicaciones genéricas.
- **`apps/ai`** - el trabajador de IA de referencia en Python.
- **`apps/web`** - el panel de control React + Vite + TypeScript.
- La documentación, la infraestructura (configuración de MediaMTX) y los scripts.

## Qué es propietario

Los productos verticales y específicos para clientes residen como crates
separados en un repositorio privado (`heldar-proprietary`). **Dependen de** los
crates abiertos (a través de crates.io, o una etiqueta git previa a la
publicación, con un parche de ruta local para el desarrollo en paralelo) y
añaden sus especificidades de dominio encima. Nunca se copian en el repositorio
público y el kernel nunca los referencia.

El servidor de composición aísla la composición propietaria detrás de una
interfaz: en la compilación abierta esa interfaz es un stub sin operación, por
lo que el servidor de referencia no enlaza ningún código propietario. `main.rs`
es byte a byte idéntico entre las compilaciones abierta y privada.

## Por qué esta estructura

Poseer el kernel significa poseer el modelo de metadatos, el motor de eventos y
la lógica del producto, mientras que las interfaces mantienen las aplicaciones
desacopladas de él. Un despliegue se compone del kernel más las aplicaciones que
un cliente necesite (un único inquilino por despliegue), por lo que las
aplicaciones genéricas abiertas y cualquier vertical propietario son simplemente
crates enlazados en una compilación de servidor. Los cambios que rompen la
compatibilidad en las interfaces del kernel suponen un incremento de versión
mayor al que las aplicaciones se adhieren voluntariamente.

## Licencias

El kernel y las aplicaciones genéricas son Apache-2.0. Los verticales
propietarios tienen licencia por separado. Consulte
[LICENSING.md](https://github.com/Straits-AI/heldar/blob/main/LICENSING.md) para
conocer el límite.
