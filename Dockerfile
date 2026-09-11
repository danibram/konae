FROM oven/bun:1.3.11

WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .

EXPOSE 4173 8787
CMD ["bun", "run", "kernel"]
