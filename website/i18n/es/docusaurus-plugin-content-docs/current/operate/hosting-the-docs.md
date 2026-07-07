---
id: hosting-the-docs
title: Alojar la documentación
sidebar_label: Alojar la documentación
sidebar_position: 2
---

# Alojar la documentación

El sitio de documentación es una compilación estática de [Docusaurus](https://docusaurus.io/)
(el directorio `website/`) alojada en **Cloudflare Workers** mediante
[Static Assets](https://developers.cloudflare.com/workers/static-assets/).
Cloudflare [recomienda Workers sobre Pages](https://developers.cloudflare.com/workers/static-assets/migration-guides/migrate-from-pages/)
para proyectos nuevos: "deberías comenzar con Workers ... en adelante, toda nuestra
inversión, optimizaciones y trabajo de funcionalidades se dedicarán a mejorar Workers."

Es un **Worker solo de activos** (sin código de servidor): Cloudflare sirve la salida
precompilada directamente. La configuración reside en `website/wrangler.jsonc`:

```jsonc
{
  "name": "heldar-docs",
  "compatibility_date": "2026-06-15",
  "assets": {
    "directory": "./build",
    "not_found_handling": "404-page"
  }
}
```

`not_found_handling: "404-page"` sirve el `404.html` generado por Docusaurus para
las páginas no encontradas (usa `"single-page-application"` solo para enrutamiento
en el lado del cliente estilo SPA).
El sitio se sirve en la raíz del dominio del Worker, por lo que `baseUrl` permanece como `/`.

## Desplegar desde tu máquina (ya tienes wrangler)

```bash
cd website
npm ci
npm run build          # -> website/build
wrangler deploy        # uploads ./build as an assets-only Worker
```

Vista previa local antes de desplegar:

```bash
cd website
npm run build && wrangler dev   # serves the built site on http://localhost:8787
```

El primer `wrangler deploy` crea el Worker `heldar-docs` y lo publica en
`https://heldar-docs.<your-subdomain>.workers.dev`.

## Desplegar desde CI (opcional)

El repositorio incluye `.github/workflows/cloudflare-workers.yml`: en cada push a `main`
compila el sitio y ejecuta `wrangler deploy` mediante
[`cloudflare/wrangler-action`](https://github.com/cloudflare/wrangler-action). Siempre
compila (para que la compilación sea validada incluso sin secretos) y solo despliega
cuando el token está presente. Para habilitarlo:

1. Crea un **token de API** con alcance limitado con el permiso **Workers Scripts: Edit**
   (más la lectura de **Workers R2 / Account** según lo solicite la interfaz del token).
2. Agrega dos **secretos de repositorio** en la configuración de GitHub:
   - `CLOUDFLARE_API_TOKEN` - el token con alcance limitado.
   - `CLOUDFLARE_ACCOUNT_ID` - tu ID de cuenta (resumen de Workers y Pages, o la
     URL del panel).

## Dominio personalizado

En el panel de Cloudflare abre el Worker `heldar-docs`, ve a **Settings ->
Domains & Routes -> Add -> Custom Domain**, y agrega tu dominio (por ejemplo
`docs.heldar.ai`). Cloudflare aprovisiona el certificado automáticamente. Debido a
que el Worker sirve en la raíz del dominio, no se necesita cambiar `baseUrl` - permanece como `/`.
(Establece `url` en `website/docusaurus.config.ts` a ese dominio para URLs canónicas
y de mapa del sitio correctas.)
