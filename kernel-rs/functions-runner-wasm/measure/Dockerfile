ARG BASE
FROM node:22-bookworm-slim AS node

FROM ${BASE}
RUN apt-get update && apt-get install -y --no-install-recommends python3 procps libstdc++6 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=node /usr/local/bin/node /usr/local/bin/node
