# Stage 1: Build the Rust Backend
FROM rust:1.80-slim-bookworm AS rust-builder
WORKDIR /app
# Install dependencies needed for compiling
RUN apt-get update && apt-get install -y pkg-config libssl-dev protobuf-compiler curl

# Copy the Rust projects
COPY Cargo.toml Cargo.lock ./
COPY src ./src

# Build the release binary
RUN cargo build --release

# Stage 2: Build the Next.js Frontend
FROM oven/bun:1.1 AS js-builder
WORKDIR /app

COPY package.json bun.lock* ./
RUN bun install --frozen-lockfile

COPY . ./

# Build the standalone Next.js application
RUN bun run build

# Stage 3: Make the final runtime Slim image
FROM oven/bun:1.1-slim AS runner
WORKDIR /app

# Install openssl which reqwest requires
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

# Next.js telemetry and node env
ENV NODE_ENV production
ENV NEXT_TELEMETRY_DISABLED 1

# Copy Next.js standalone build
COPY --from=js-builder /app/.next/standalone ./
COPY --from=js-builder /app/.next/static ./.next/static
COPY --from=js-builder /app/public ./public

# Copy Rust backend
COPY --from=rust-builder /app/target/release/caldav-ics-sync /app/caldav-ics-sync

# Expose ports
EXPOSE 3000
EXPOSE 3001

# Default environment configuration (can be overridden)
ENV SERVER_PORT=3000
ENV PORT=3001
ENV SERVER_PROXY_URL=http://localhost:3001

# Use a shell script to start both
RUN echo '#!/bin/sh\nbun server.js & \n./caldav-ics-sync\n' > /app/start.sh && \
    chmod +x /app/start.sh

# The volume mapping where users should mount their ICS backup
VOLUME ["/data"]

CMD ["/app/start.sh"]
