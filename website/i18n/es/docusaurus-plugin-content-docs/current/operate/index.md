---
id: operate
title: Operar
sidebar_label: Operar
sidebar_position: 1
slug: /operate
---

# Operar

Ejecución, seguridad y mantenimiento de un despliegue de Heldar. Para configurar
un despliegue por primera vez, comience con [Desplegar](../getting-started/deploy.md).

Las guías detalladas para operadores e integradores se encuentran actualmente en el repositorio.
Cada enlace a continuación abre la guía en el repositorio en GitHub:

- [Control de Acceso](https://github.com/Straits-AI/heldar/blob/main/docs/ACCESS-CONTROL.md)
  - el motor de entrada con autorización de matrículas, el registro de vehículos/visitantes/lista de vigilancia,
  el flujo de confirmación/rechazo del guardia, RBAC e informes.
- [Movimiento](https://github.com/Straits-AI/heldar/blob/main/docs/MOVEMENT.md)
  - candidatos ReID multi-señal entre cámaras (revisados por humanos, con una señal
  `appearance_score` de similitud CLIP opcional y desactivada por defecto, secundaria
  al ancla de matrícula) e incidentes de vulneración de zonas restringidas.
- [Búsqueda](https://github.com/Straits-AI/heldar/blob/main/docs/SEARCH.md)
  - consulta determinista sobre hechos de eventos almacenados, con el plan en lenguaje natural como
  el único paso falible y una capa de prueba sobre cada respuesta.
- [Observabilidad](https://github.com/Straits-AI/heldar/blob/main/docs/OBSERVABILITY.md)
  - las APIs de salud/métricas/eventos, la exposición de Prometheus, el webhook de alertas,
  la monitorización del almacenamiento y el reporte de brechas en grabaciones.
- [Acceso Remoto](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md)
  - visualización remota desde el navegador mediante WebRTC, con traversal de NAT gestionado por un
  relay de señalización + TURN y el flujo de medios cifrado de extremo a extremo, más una
  superposición de red autoalojada opcional para acceder a un sitio detrás de CGNAT.
- [Endurecimiento para producción](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md)
  - la lista de verificación de seguridad para un despliegue expuesto a internet: auth + cookie TLS
  requeridos, bloqueo de inicio de sesión por cuenta, cifrado en reposo de credenciales de cámara,
  los guardianes de inicio en modo fallo ruidoso y los secretos del Worker de encuentro (incluido el
  desafío de inicio de sesión opcional de Cloudflare Turnstile).
- [Dimensionamiento](https://github.com/Straits-AI/heldar/blob/main/docs/sizing.md)
  - planificación de capacidad para cámaras, almacenamiento y el presupuesto de fotogramas de IA.
- [Puesta en marcha](https://github.com/Straits-AI/heldar/blob/main/docs/commissioning-checklist.md)
  - la lista de verificación para poner en línea un nuevo sitio.

Para conocer la arquitectura detrás de esto, consulte
[ARCHITECTURE.md](https://github.com/Straits-AI/heldar/blob/main/ARCHITECTURE.md)
y la descripción general de [Arquitectura](../concepts/architecture.md).
