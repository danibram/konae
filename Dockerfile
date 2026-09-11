FROM oven/bun:1.3.11

# The oven/bun image ships an unprivileged `bun` user. Vite writes its cache
# under node_modules/.vite, so that user owns /app.
WORKDIR /app
RUN chown bun:bun /app
USER bun
COPY --chown=bun:bun package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY --chown=bun:bun . .

EXPOSE 4173
CMD ["bun", "run", "dev", "--", "--host", "0.0.0.0"]
