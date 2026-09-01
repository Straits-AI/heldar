# Heldar dashboard — build the Vite SPA, serve via nginx.
# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for reproducibility.
# To bump: `docker buildx imagetools inspect node:<tag>` and update the digest (see docs/SUPPLY-CHAIN.md).
FROM node:22.23.2-bookworm-slim@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436 AS builder
WORKDIR /app
COPY package*.json ./
RUN npm install
COPY . .
RUN npm run build

# Pinned runtime base: concrete minor.patch tag + multi-arch index @sha256 digest.
FROM nginx:1.27.5-alpine@sha256:65645c7bb6a0661892a8b03b89d0743208a18dd2f3f17a54ef4b76fb8e2f2a10
COPY --from=builder /app/dist /usr/share/nginx/html
# Rendered by the image's envsubst entrypoint step into /etc/nginx/conf.d/default.conf at start, so
# the listen address is deployment-settable (loopback-only behind the TLS overlay). The filter keeps
# envsubst away from nginx's own $uri/$host variables — without it they would be blanked out.
COPY nginx.conf /etc/nginx/templates/default.conf.template
ENV NGINX_LISTEN=8080 \
    NGINX_ENVSUBST_FILTER="^NGINX_"
EXPOSE 8080
