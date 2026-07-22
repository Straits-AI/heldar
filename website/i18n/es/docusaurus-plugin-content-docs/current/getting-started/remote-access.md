---
id: remote-access
title: Acceso Remoto
sidebar_label: Acceso Remoto
sidebar_position: 3
---

# Acceso Remoto

Visualice un despliegue de Heldar desde fuera de su LAN — incluso desde detrás de **CGNAT** (el caso habitual en hogares y sitios pequeños: una IP pública compartida, sin puerto entrante para redirigir). La visualización remota se realiza **en el navegador mediante WebRTC** y es una **capacidad del kernel abierto**: cada despliegue obtiene acceso remoto privado sin aplicación cliente y sin puerto entrante.

## Cómo funciona

El dispositivo **marca hacia afuera** hacia un punto de encuentro; el espectador simplemente abre el panel de control en un navegador. CGNAT siempre permite las conexiones salientes, por lo que esto funciona donde la redirección de puertos y el DDNS no pueden.

- **El vídeo en directo** viaja por **MediaMTX / WHEP** y está **cifrado de extremo a extremo (DTLS-SRTP)**. El punto de encuentro solo gestiona el intercambio SDP/ICE y el control retransmitido — nunca los bytes de vídeo. Cuando no es posible abrir un camino directo entre pares a través de CGNAT simétrico, **TURN** retransmite paquetes que no puede leer.
- **El panel de control completo** (vista en directo, reproducción grabada, configuración, eventos) funciona a través del mismo camino intermediado, detrás de un modelo de autenticación de **doble puerta** que mantiene al kernel como única autoridad RBAC:
  - **Puerta exterior** — una capacidad de corta duración, por usuario y con ámbito de sitio, que prueba que el navegador puede *alcanzar* este dispositivo.
  - **Puerta interior** — su **sesión real del kernel** se reenvía literalmente y se reproduce contra el propio kernel `127.0.0.1` del dispositivo, que ejecuta su autenticación y RBAC normales. El relay es un conducto silencioso con lista de permitidos, nunca un bypass de autenticación; el token de sesión vive en una cookie HttpOnly que el JS del navegador nunca posee.
- **Fail-safe:** el relay **se niega a funcionar a menos que la autenticación esté habilitada y exista un usuario real** — la API abierta sin autenticación nunca se expone de forma remota.

> **¿Por qué no usar redirección de puertos / DDNS / un proxy inverso público?** El CGNAT de los operadores bloquea todas las conexiones entrantes y normalmente es NAT *simétrico*, lo que frustra la apertura de agujeros STUN simple. Lo único que funciona de forma fiable es que el dispositivo marque hacia afuera — que es exactamente lo que hace el camino WebRTC (y la alternativa de overlay que se describe más abajo).

:::info Dónde empieza el nivel gestionado
El **lado del dispositivo** de este camino — el cliente de marcado saliente, el puente WHEP, el relay — es código abierto del kernel, pero necesita un **punto de encuentro** al que marcar: el punto de encuentro alojado es el nivel gestionado de Heldar. Si prefiere no depender de ningún servicio alojado, la alternativa totalmente auto-alojada es el overlay de red que se describe más abajo.
:::

## Activación (en el lado del dispositivo)

El acceso remoto es opcional. El dispositivo necesita la autenticación activada y un punto de encuentro al que marcar:

```bash
HELDAR_AUTH_ENABLED=true              # required — the relay refuses to run without it
HELDAR_AUTH_COOKIE_SECURE=true        # the rendezvous terminates TLS
HELDAR_REMOTE_RENDEZVOUS_URL=https://<your-rendezvous>   # the box dials OUT to this
HELDAR_CP_TOKEN=<dial-out bearer>     # the per-box token your rendezvous issued for this HELDAR_SITE_ID (site-bound)
HELDAR_SITE_ID=<stable-id-for-this-box>
```

Con estos valores configurados, el kernel marca hacia afuera, conecta las ofertas WHEP del navegador con su propio MediaMTX, y programa los servidores ICE de MediaMTX para que el dispositivo obtenga un candidato relay para la traversal de NAT simétrico. Los espectadores abren el panel de control que el punto de encuentro sirve para su sitio.

**TURN — use el punto de encuentro gestionado o configure el suyo propio:**

- **Gestionado:** apunte `HELDAR_REMOTE_RENDEZVOUS_URL` al punto de encuentro alojado; el kernel obtiene credenciales TURN de corta duración de él y las renueva automáticamente.
- **El suyo propio:** establezca `HELDAR_WEBRTC_ICE_SERVERS` como un array JSON `webrtcICEServers2` de MediaMTX apuntando a cualquier STUN/TURN que gestione (coturn, Cloudflare Realtime, …). El kernel lo programa en MediaMTX.
- **Ninguno** → MediaMTX se queda en su línea base STUN: solo LAN / NAT no simétrico.

La reproducción grabada es **paso directo de HEVC/H.265** — el dispositivo envía el flujo de bits grabado sin modificar y el hardware del cliente lo decodifica (el camino más eficiente sobre un enlace ascendente estrecho); los navegadores sin HEVC reciben una nota clara en lugar de un fotograma negro.

:::warning Refuerce la seguridad antes de exponerlo
El relay aplica autenticación, pero un dispositivo expuesto a internet necesita la lista de comprobación completa — una cookie `Secure`, un TTL de sesión corto más tiempo de espera por inactividad, cifrado de credenciales de cámara (`HELDAR_SECRET_KEY`), y los secretos del punto de encuentro (con un desafío de inicio de sesión opcional de Cloudflare Turnstile). Consulte primero la [guía de refuerzo para producción](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md) — el kernel también **falla ruidosamente** y se niega a arrancar con la autenticación desactivada detrás de un punto de encuentro.
:::

## Alternativa: un overlay de red auto-alojado

Si prefiere tener acceso L3 completo a todo el host en lugar del punto de encuentro alojado, ejecute un overlay WireGuard como daemon externo y apunte el kernel a su interfaz para obtener el estado:

```bash
HELDAR_OVERLAY_ENABLED=true
HELDAR_OVERLAY_KIND=tailscale         # or netbird
HELDAR_OVERLAY_IFACE=tailscale0       # wt0 for netbird
```

- **Personal / desarrollo → Tailscale** (gratuito, operación casi nula; solo uso no comercial).
- **Producto desplegado → NetBird auto-alojado** (un contenedor por despliegue, sin coste por usuario, sin metadatos de terceros).

El kernel solo *observa* el overlay (nunca gestiona WireGuard) y expone su estado en `GET /api/v1/system → remote_access`. Restrinja la ACL del overlay a los puertos de medios y control (`8889` / `8888` / `8000`), no a todo el host.

## Más información

- **Referencia completa y recetas de overlay:**
  [`docs/REMOTE-ACCESS.md`](https://github.com/Straits-AI/heldar/blob/main/docs/REMOTE-ACCESS.md) — la
  justificación de CGNAT, el modelo de privacidad con prioridad P2P, y las recetas completas de Tailscale / NetBird.
- **Refuerzo de seguridad:**
  [Refuerzo para producción](https://github.com/Straits-AI/heldar/blob/main/docs/PRODUCTION.md).
- **Operar el hub:** [Operar](../operate/index.md).
