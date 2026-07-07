---
id: intro
title: Introducción
sidebar_label: Introducción
sidebar_position: 1
slug: /
---

# Heldar

Heldar es un sistema operativo de inteligencia visual de eventos para espacios físicos. Convierte transmisiones de cámaras en eventos estructurados, los eventos en flujos de trabajo, y los flujos de trabajo en inteligencia operacional. En lugar de envolver un DVR/NVR existente o comenzar desde funciones de IA, Heldar construye primero su propio **núcleo multimedia** (registro de cámaras, ingestión RTSP, grabación, reproducción, vista en vivo) y luego agrega percepción, un motor de zonas y aplicaciones encima como *consumidores*. FFmpeg y MediaMTX realizan el trabajo multimedia de bajo nivel; Heldar es propietario del modelo de metadatos, el motor de eventos y la lógica del producto.

## Código abierto con núcleo propietario

Heldar es open-core:

- Un **núcleo** Apache-2.0 (`heldar-kernel`) más **aplicaciones de referencia genéricas**
  (`heldar-entry`, `heldar-movement`, `heldar-search`), un servidor de composición de referencia,
  un trabajador de IA de referencia y un panel de control en React. Este es el repositorio
  público `heldar`.
- Los **productos verticales / de cliente** viven como crates propietarios separados en un
  repositorio privado y dependen de los crates abiertos. El núcleo nunca los referencia.

Las aplicaciones se conectan al núcleo únicamente a través de un pequeño conjunto de interfaces públicas, por lo que el núcleo **no** tiene dependencia de ninguna aplicación. Un despliegue se *compone* a partir del núcleo más las aplicaciones que el cliente necesite (un único inquilino por despliegue). Consulte [Código abierto con núcleo propietario](./concepts/open-core.md) para conocer los límites y [Arquitectura](./concepts/architecture.md) para las interfaces.

## Arquitectura de un vistazo

![Arquitectura open-core de Heldar](/img/diagrams/architecture.svg)

El núcleo es el **único** componente que se comunica con las cámaras. La grabadora 24/7 mantiene el flujo comprimido sin necesidad de decodificación; un muestreador con presupuesto es lo único que decodifica, escribiendo un fotograma actual por cámara. Los trabajadores de IA son clientes HTTP puros: extraen fotogramas muestreados y publican detecciones de vuelta. Las aplicaciones interpretan esas detecciones como eventos de dominio.

## Siguientes pasos

- [Inicio rápido](./getting-started/quickstart.md) - compilar, ejecutar, agregar una cámara y
  ejecutar el trabajador de IA.
- [Despliegue](./getting-started/deploy.md) - un binario, una URL (Docker en una línea,
  binario nativo o un dispositivo flasheado).
- [Acceso remoto](./getting-started/remote-access.md) - ver un sitio desde cualquier lugar
  en un navegador, mediante WebRTC, incluso detrás de CGNAT.
- [Uso del panel de control](./getting-started/dashboard.md) - un recorrido por la interfaz web:
  vista en vivo, reproducción, zonas, incidentes y la página del sistema.
- [Arquitectura](./concepts/architecture.md) - el núcleo y sus cuatro interfaces públicas.
- [Código abierto con núcleo propietario](./concepts/open-core.md) - qué es abierto y qué es propietario.
- [Crear un módulo](./develop/build-a-module.md) - crear su propia aplicación sobre el
  núcleo abierto.
- [Crear un trabajador de IA](./develop/ai-worker.md) - el contrato del SDK del trabajador de percepción.
- [Operación](./operate/index.md) - las guías de operador e integrador incluidas en el repositorio.

El código fuente está en
[github.com/Straits-AI/heldar](https://github.com/Straits-AI/heldar).
