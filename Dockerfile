# Multi-stage Dockerfile for Connector
#
# Stage 1 (builder): Compiles TypeScript to JavaScript with all dependencies
# Stage 1.5 (ui-builder): Builds Explorer UI with Vite
# Stage 2 (runtime): Runs compiled connector with production dependencies only
#
# Build: docker build -t connector .
# Run:   docker run -e NODE_ID=connector-a -e BTP_SERVER_PORT=3000 -p 3000:3000 -p 3001:3001 connector

# ============================================
# Stage 1: Builder
# ============================================
FROM node:22-alpine AS builder

# Set working directory
WORKDIR /app

# Copy dependency manifests first (for layer caching)
# Root package files define the workspace structure
COPY package.json package-lock.json ./
COPY tsconfig.base.json ./

# Copy workspace package.json files to preserve monorepo structure
COPY packages/connector/package.json ./packages/connector/
COPY packages/shared/package.json ./packages/shared/

# Install all dependencies (including devDependencies for TypeScript compilation)
# Use npm ci for reproducible builds
# Use --ignore-scripts to skip prepare script (git hooks not needed in Docker builds)
RUN npm ci --workspaces --ignore-scripts

# Copy TypeScript configuration and source code
COPY packages/connector/tsconfig.json ./packages/connector/
COPY packages/shared/tsconfig.json ./packages/shared/
COPY packages/connector/src ./packages/connector/src
COPY packages/shared/src ./packages/shared/src

# Build all packages (TypeScript compilation)
# Build shared first, then connector (dependency order)
# Use build:connector-only to skip UI build (UI is built in ui-builder stage)
RUN npm run build --workspace=@crosstown/shared && npm run build:connector-only --workspace=@crosstown/connector

# ============================================
# Stage 1.5: UI Builder (Explorer UI)
# ============================================
FROM node:22-alpine AS ui-builder

WORKDIR /app

# Copy explorer-ui package
COPY packages/connector/explorer-ui ./packages/connector/explorer-ui

# Change to explorer-ui directory and install dependencies
WORKDIR /app/packages/connector/explorer-ui

# Install dependencies and build (skip tsc type-check, vite handles transpilation)
RUN npm ci && npx vite build

# ============================================
# Stage 2: Runtime
# ============================================
FROM node:22-alpine AS runtime

# Set production environment
ENV NODE_ENV=production

# Set working directory
WORKDIR /app

# Copy dependency manifests for production installation
COPY package.json package-lock.json ./
COPY packages/connector/package.json ./packages/connector/
COPY packages/shared/package.json ./packages/shared/

# Install production dependencies only (excludes devDependencies like TypeScript)
# This significantly reduces image size
# Remove the 'prepare' script before install (it runs husky which is a devDependency)
# Then explicitly install the platform-specific libsql binary for Alpine ARM64/x64
RUN apk add --no-cache jq && \
    jq 'del(.scripts.prepare)' package.json > package.json.tmp && \
    mv package.json.tmp package.json && \
    npm ci --workspaces --omit=dev --ignore-scripts && \
    cd packages/connector && \
    LIBSQL_VERSION=$(npm ls libsql --json 2>/dev/null | jq -r '.dependencies.libsql.version // .dependencies["@libsql/client"].dependencies.libsql.version // "0.4.7"') && \
    (npm install "@libsql/linux-arm64-musl@${LIBSQL_VERSION}" --no-save 2>/dev/null || \
     npm install "@libsql/linux-x64-musl@${LIBSQL_VERSION}" --no-save 2>/dev/null || true) && \
    cd ../.. && \
    apk del jq

# Copy compiled JavaScript from builder stage
# Only copy dist directories, not source code
COPY --from=builder /app/packages/connector/dist ./packages/connector/dist
COPY --from=builder /app/packages/shared/dist ./packages/shared/dist

# Copy built Explorer UI from ui-builder stage
# Vite outputs to ../dist/explorer-ui (relative to explorer-ui directory)
# UI is served by ExplorerServer from ./dist/explorer-ui
COPY --from=ui-builder /app/packages/connector/dist/explorer-ui ./packages/connector/dist/explorer-ui

# Install wget for health check (minimal package, available in Alpine)
# Used by Docker HEALTHCHECK to query HTTP health endpoint
RUN apk add --no-cache wget

# Security hardening: Run as non-root user
# Alpine's node image includes a 'node' user by default
# Create data directory for Explorer UI SQLite databases and change ownership
RUN mkdir -p /app/data && chown -R node:node /app

# Switch to non-root user (prevents privilege escalation attacks)
USER node

# Expose BTP server port (WebSocket)
# Default: 3000 (configurable via BTP_SERVER_PORT environment variable)
EXPOSE 3000

# Expose Explorer UI port (HTTP/WebSocket)
# Default: 3001 (configurable via EXPLORER_PORT environment variable)
EXPOSE 3001

# Expose health check HTTP port
# Default: 8080 (configurable via HEALTH_CHECK_PORT environment variable)
EXPOSE 8080

# Health check: Query HTTP health endpoint
# Interval: Check every 30 seconds (balance between responsiveness and overhead)
# Timeout: Health endpoint must respond within 10 seconds
# Start period: Allow 40 seconds for connector startup (BTP connections establishment)
# Retries: Mark unhealthy after 3 consecutive failures
#
# The health endpoint returns:
# - 200 OK when connector is healthy (≥50% peers connected)
# - 503 Service Unavailable when unhealthy or starting
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
  CMD wget --no-verbose --tries=1 --spider http://localhost:8080/health || exit 1

# Start connector
# Environment variables:
# - NODE_ID: Connector identifier (default: 'connector-node')
# - BTP_SERVER_PORT: BTP server listening port (default: 3000)
# - LOG_LEVEL: Pino log level (default: 'info', options: debug|info|warn|error)
CMD ["node", "packages/connector/dist/main.js"]
