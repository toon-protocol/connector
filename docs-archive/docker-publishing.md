# Docker Image Publishing Guide

This document describes the optional process for publishing M2M connector and dashboard Docker images to a container registry.

## Overview

The M2M project currently builds Docker images locally for use with Docker Compose. Publishing images to a registry (Docker Hub, GitHub Container Registry, or private registry) is optional but recommended for:

- **Production Deployments**: Pull pre-built images instead of building locally
- **CI/CD Pipelines**: Automated testing and deployment
- **Team Collaboration**: Share consistent images across developers
- **Version Management**: Tag and distribute specific versions

## Prerequisites

- Docker installed and logged in to your registry
- Push access to the target container registry
- Built and tested images locally

## Publishing to Docker Hub

### 1. Tag Images

```bash
# Connector image
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:0.1.0
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:latest

# Dashboard image
docker tag ilp-dashboard:latest ALLiDoizCode/ilp-dashboard:0.1.0
docker tag ilp-dashboard:latest ALLiDoizCode/ilp-dashboard:latest
```

### 2. Push Images

```bash
# Login to Docker Hub
docker login

# Push connector
docker push ALLiDoizCode/agent-runtime:0.1.0
docker push ALLiDoizCode/agent-runtime:latest

# Push dashboard
docker push ALLiDoizCode/ilp-dashboard:0.1.0
docker push ALLiDoizCode/ilp-dashboard:latest
```

### 3. Update Docker Compose

Update `docker-compose.yml` and other compose files to use published images:

```yaml
services:
  connector-a:
    image: ALLiDoizCode/agent-runtime:0.1.0
    # Remove 'build:' directive
```

## Publishing to GitHub Container Registry (GHCR)

### 1. Create Personal Access Token

1. Go to GitHub Settings → Developer settings → Personal access tokens
2. Generate token with `write:packages` and `read:packages` scopes

### 2. Login to GHCR

```bash
echo YOUR_TOKEN | docker login ghcr.io -u YOUR_USERNAME --password-stdin
```

### 3. Tag and Push

```bash
# Connector
docker tag agent-runtime:latest ghcr.io/ALLiDoizCode/agent-runtime:0.1.0
docker push ghcr.io/ALLiDoizCode/agent-runtime:0.1.0

# Dashboard
docker tag ilp-dashboard:latest ghcr.io/ALLiDoizCode/ilp-dashboard:0.1.0
docker push ghcr.io/ALLiDoizCode/ilp-dashboard:0.1.0
```

## Automated Publishing with GitHub Actions

Create `.github/workflows/docker-publish.yml`:

```yaml
name: Publish Docker Images

on:
  release:
    types: [published]
  workflow_dispatch:

env:
  REGISTRY: ghcr.io
  IMAGE_NAME_CONNECTOR: ${{ github.repository }}/connector
  IMAGE_NAME_DASHBOARD: ${{ github.repository }}/dashboard

jobs:
  build-and-push:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      packages: write

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Log in to Container Registry
        uses: docker/login-action@v3
        with:
          registry: ${{ env.REGISTRY }}
          username: ${{ github.actor }}
          password: ${{ secrets.GITHUB_TOKEN }}

      - name: Extract metadata (tags, labels) for Connector
        id: meta-connector
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME_CONNECTOR }}
          tags: |
            type=ref,event=branch
            type=ref,event=pr
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}

      - name: Build and push Connector image
        uses: docker/build-push-action@v5
        with:
          context: .
          file: ./Dockerfile
          push: true
          tags: ${{ steps.meta-connector.outputs.tags }}
          labels: ${{ steps.meta-connector.outputs.labels }}

      - name: Extract metadata (tags, labels) for Dashboard
        id: meta-dashboard
        uses: docker/metadata-action@v5
        with:
          images: ${{ env.REGISTRY }}/${{ env.IMAGE_NAME_DASHBOARD }}
          tags: |
            type=ref,event=branch
            type=ref,event=pr
            type=semver,pattern={{version}}
            type=semver,pattern={{major}}.{{minor}}

      - name: Build and push Dashboard image
        uses: docker/build-push-action@v5
        with:
          context: .
          file: ./packages/dashboard/Dockerfile
          push: true
          tags: ${{ steps.meta-dashboard.outputs.tags }}
          labels: ${{ steps.meta-dashboard.outputs.labels }}
```

## Version Tagging Strategy

Follow semantic versioning for image tags:

- **Latest**: `latest` - always points to most recent stable release
- **Version**: `0.1.0`, `1.0.0` - specific version tags
- **Minor**: `0.1`, `1.0` - tracks latest patch for minor version
- **Major**: `0`, `1` - tracks latest minor for major version

Example tagging for v0.1.0 release:

```bash
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:0.1.0
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:0.1
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:0
docker tag agent-runtime:latest ALLiDoizCode/agent-runtime:latest
```

## Security Considerations

- **Image Scanning**: Scan images for vulnerabilities before publishing
  ```bash
  docker scan ALLiDoizCode/agent-runtime:0.1.0
  ```
- **Multi-arch**: Consider building for multiple architectures (amd64, arm64)
  ```bash
  docker buildx build --platform linux/amd64,linux/arm64 -t ALLiDoizCode/agent-runtime:0.1.0 --push .
  ```
- **Secrets**: Never include secrets in images - use environment variables
- **Private Registries**: For production, use private registries with access controls

## Pulling Published Images

Users can pull and run published images:

```bash
# Pull connector
docker pull ALLiDoizCode/agent-runtime:0.1.0

# Pull dashboard
docker pull ALLiDoizCode/ilp-dashboard:0.1.0

# Run with docker-compose (using published images)
docker-compose up -d
```

## Image Maintenance

- **Update README**: Add pull instructions to project README
- **Deprecation**: Mark old versions as deprecated in registry
- **Cleanup**: Remove very old images to save registry storage
- **Documentation**: Keep version tags documented in CHANGELOG.md

## Current Status

**As of v0.1.0**: Docker images are NOT published to any public registry. Users must build images locally using:

```bash
docker build -t agent-runtime .
docker build -t ilp-dashboard -f packages/dashboard/Dockerfile .
```

For production use, follow this guide to publish images to your preferred registry.
