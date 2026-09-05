# Heldar dashboard — build the Vite SPA, serve via nginx.
# Pinned base: concrete minor.patch tag + multi-arch index @sha256 digest for reproducibility.
# To bump: `docker buildx imagetools inspect node:<tag>` and update the digest (see docs/SUPPLY-CHAIN.md).
# Scanned by Trivy (HIGH/CRITICAL, fixed only) BEFORE anything is published — see
# docs/SUPPLY-CHAIN.md. A base bump that drags in a fixable HIGH fails the build rather than
# shipping. Bump the pinned digest to fix; where there is no fix yet, record it in
# security/dependency-exceptions.json with an owner and an expiry.
FROM node:24.20.0-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS builder
WORKDIR /app
COPY package*.json ./
RUN npm install
COPY . .
RUN npm run build

# Pinned runtime base: concrete minor.patch tag + multi-arch index @sha256 digest.
FROM nginx:1.31.4-alpine@sha256:db35bfc6b2951e7f8a72db5db120288c127ffaeeb4a6d4b95a26fead017d5913
COPY --from=builder /app/dist /usr/share/nginx/html
# Rendered by the image's envsubst entrypoint step into /etc/nginx/conf.d/default.conf at start, so
# the listen address is deployment-settable (loopback-only behind the TLS overlay). The filter keeps
# envsubst away from nginx's own $uri/$host variables — without it they would be blanked out.
COPY nginx.conf /etc/nginx/templates/default.conf.template
ENV NGINX_LISTEN=8080 \
    NGINX_ENVSUBST_FILTER="^NGINX_"
EXPOSE 8080
