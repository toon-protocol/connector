# GitHub Repository Configuration and Release Guide

This document provides instructions for configuring the GitHub repository and creating the v0.1.0 release.

## Repository Metadata Configuration

### Repository Settings

1. **Navigate to Repository Settings** on GitHub
2. **Update repository description**:

   ```
   Educational ILPv4 connector with BTP support and real-time network visualization dashboard
   ```

3. **Add repository topics/tags**:
   - `interledger`
   - `ilp`
   - `ilpv4`
   - `btp`
   - `payment-networks`
   - `typescript`
   - `docker`
   - `connector`
   - `telemetry`
   - `dashboard`
   - `visualization`
   - `educational`

4. **Set website URL** (if deployed):

   ```
   https://your-demo-url.com
   ```

5. **Configure default branch**: Ensure `main` is set as the default branch

### Repository Features

Enable the following features in Settings → General:

- ✅ **Issues** - for bug reports and feature requests
- ✅ **Wiki** - for extended documentation (optional)
- ✅ **Discussions** - for community Q&A (optional)
- ✅ **Projects** - for roadmap tracking (optional)
- ⬜ **Sponsorships** - (optional)

### Branch Protection Rules

Protect the `main` branch with the following rules (Settings → Branches → Add rule):

- **Branch name pattern**: `main`
- ✅ **Require pull request reviews before merging**
  - Required approving reviews: 1
- ✅ **Require status checks to pass before merging**
  - ✅ CI build
  - ✅ Lint
  - ✅ Tests
- ✅ **Require conversation resolution before merging**
- ✅ **Do not allow bypassing the above settings**

## GitHub Actions Configuration

### Required Secrets

No additional secrets are required for the basic CI workflow. GitHub Actions automatically provides:

- `GITHUB_TOKEN` - for package publishing and API access

### Optional Secrets (for Docker publishing)

If publishing to Docker Hub, add these secrets in Settings → Secrets and variables → Actions:

- `DOCKERHUB_USERNAME` - Your Docker Hub username
- `DOCKERHUB_TOKEN` - Docker Hub access token

## Creating v0.1.0 Release

### Step 1: Ensure All Changes Are Committed

```bash
# Check git status
git status

# Stage and commit any remaining changes
git add .
git commit -m "Prepare v0.1.0 MVP release

- Bidirectional BTP forwarding implementation
- Resilient startup with peer retry
- Complete telemetry and dashboard
- Five example topology configurations
- Comprehensive documentation

🤖 Generated with Claude Code
Co-Authored-By: Claude <noreply@anthropic.com>"

# Push to remote
git push origin main
```

### Step 2: Create Git Tag

```bash
# Create annotated tag
git tag -a v0.1.0 -m "v0.1.0 MVP Release

Initial MVP release of M2M ILP Connector.

Features:
- ILPv4 packet handling and routing
- BTP protocol with bidirectional forwarding
- Real-time telemetry and network visualization
- Docker Compose topology configurations
- Comprehensive test coverage

See CHANGELOG.md for full release notes."

# Push tag to remote
git push origin v0.1.0
```

### Step 3: Create GitHub Release

1. **Navigate to Releases** page: `https://github.com/ALLiDoizCode/connector/releases`

2. **Click "Draft a new release"**

3. **Choose tag**: Select `v0.1.0` from dropdown

4. **Release title**: `v0.1.0 - MVP Release`

5. **Release notes**: Copy the following markdown:

````markdown
# v0.1.0 - MVP Release

Initial MVP release of the M2M ILP Connector - a complete educational implementation of the Interledger Protocol v4 with real-time network visualization.

## 🎉 Highlights

- **Full ILPv4 Implementation** - RFC-0027 compliant packet forwarding
- **Bidirectional BTP** - Supports both outbound and incoming peer connections
- **Real-time Dashboard** - Live network topology and packet visualization
- **Docker Ready** - 5 pre-configured network topologies
- **Production Resilient** - Robust startup, retry logic, and health monitoring

## ✨ Key Features

### Core ILP Functionality

- ILP Prepare, Fulfill, and Reject packet processing
- Longest-prefix match static routing
- Multi-hop packet forwarding
- OER serialization/deserialization
- Comprehensive error handling per RFC-0027

### BTP Protocol

- WebSocket-based peer connections
- Bidirectional forwarding (incoming + outbound)
- Auto-reconnection with exponential backoff
- Shared-secret authentication
- Resilient startup tolerating peer unavailability

### Monitoring & Telemetry

- Real-time WebSocket telemetry streaming
- NODE_STATUS, PACKET_ROUTED, and LOG events
- Health check HTTP endpoint
- Structured JSON logging with Pino
- Correlation IDs for request tracing

### Dashboard

- Interactive network topology graph (Cytoscape.js)
- Live packet animation showing routing paths
- Node status panel with connection health
- Packet detail inspection
- Filterable log viewer (level, node, search)

### Development Tools

- `send-packet` CLI for test packet injection
- 5 Docker Compose example topologies
- Comprehensive test coverage
- TypeScript strict mode

## 📦 Example Topologies

- **Linear 3-Node** - Simple chain (A→B→C)
- **Linear 5-Node** - Extended chain for performance testing
- **Mesh 4-Node** - Full mesh connectivity
- **Hub-Spoke** - Centralized hub topology
- **Complex 8-Node** - Mixed topology patterns

## 🚀 Quick Start

```bash
# Clone repository
git clone https://github.com/ALLiDoizCode/connector.git
cd m2m

# Build Docker images
docker build -t agent-runtime .
docker build -t ilp-dashboard -f packages/dashboard/Dockerfile .

# Start 3-node linear topology
docker-compose up -d

# Access dashboard
open http://localhost:8080

# Send test packet
cd tools/send-packet
npm install
npm run build
node dist/index.js -c ws://localhost:3000 -d g.connectorc.test -a 1000
```
````

## 📋 Requirements

- Node.js 20 LTS
- Docker Engine 20.10+
- Docker Compose 2.x

## 📚 Documentation

- [README.md](README.md) - Project overview and setup
- [CHANGELOG.md](CHANGELOG.md) - Detailed release notes
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines
- [docs/architecture.md](docs/architecture.md) - System architecture
- [docs/docker-publishing.md](docs/docker-publishing.md) - Docker image publishing guide

## ⚠️ Known Limitations

- Static routing only (no dynamic route discovery)
- No payment settlement (routing only)
- No STREAM protocol support
- In-memory state (no persistence)
- Shared-secret authentication (not production-grade)
- No TLS support

## 🙏 Acknowledgments

Built following official Interledger RFCs:

- [RFC-0027: ILPv4](https://github.com/interledger/rfcs/blob/master/0027-interledger-protocol-4/0027-interledger-protocol-4.md)
- [RFC-0023: Bilateral Transfer Protocol](https://github.com/interledger/rfcs/blob/master/0023-bilateral-transfer-protocol/0023-bilateral-transfer-protocol.md)
- [RFC-0030: OER Encoding](https://github.com/interledger/rfcs/blob/master/0030-notes-on-oer-encoding/0030-notes-on-oer-encoding.md)
- [RFC-0015: ILP Addresses](https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md)

## 📄 License

MIT License - see [LICENSE](LICENSE) for details.

---

**Full Changelog**: https://github.com/ALLiDoizCode/connector/commits/v0.1.0

````

6. **Mark as latest release**: ✅ Check "Set as the latest release"

7. **Pre-release**: ⬜ Leave unchecked (this is a stable release)

8. **Click "Publish release"**

### Step 4: Verify Release

After publishing:

1. ✅ Check release appears on releases page
2. ✅ Verify tag is created and pushed
3. ✅ Confirm changelog link works
4. ✅ Test download links for source code archives
5. ✅ Check GitHub Actions triggered (if CI configured for releases)

## Post-Release Tasks

### Update Documentation Links

Update any documentation that references version numbers or release status:

- README.md badges
- Docker Compose image tags (if using published images)
- Documentation site (if applicable)

### Announce Release

Optional announcement channels:

- GitHub Discussions (if enabled)
- Project blog or website
- Twitter/social media
- Interledger community forum
- Dev.to or Hashnode blog post

### Monitor Issues

After release, monitor GitHub Issues for:

- Bug reports from users trying the release
- Feature requests for next iteration
- Documentation improvements needed
- Installation/deployment issues

## Versioning Strategy

This project follows [Semantic Versioning (SemVer)](https://semver.org/):

- **MAJOR** (1.0.0): Breaking changes to API or behavior
- **MINOR** (0.1.0): New features, backward compatible
- **PATCH** (0.1.1): Bug fixes, backward compatible

Next versions:
- `v0.1.1` - Bug fixes
- `v0.2.0` - New features (e.g., STREAM protocol)
- `v1.0.0` - Production-ready release with complete feature set

## Troubleshooting

### Tag Already Exists

If tag creation fails:
```bash
# Delete local tag
git tag -d v0.1.0

# Delete remote tag
git push origin --delete v0.1.0

# Recreate tag
git tag -a v0.1.0 -m "Release message"
git push origin v0.1.0
````

### Release Draft Not Saving

- Check you have write permissions to repository
- Ensure tag exists before creating release
- Verify GitHub session is active

### CI Workflow Not Triggering

- Check `.github/workflows/*.yml` includes release trigger:
  ```yaml
  on:
    release:
      types: [published]
  ```
- Verify GitHub Actions is enabled in repository settings

---

## Summary Checklist

Before releasing v0.1.0:

- [x] All code tested and working
- [x] Documentation complete (README, CHANGELOG, CONTRIBUTING)
- [x] LICENSE file present
- [x] No TODO comments in code
- [x] Code linted and formatted
- [x] All tests passing
- [x] Docker images build successfully
- [x] Example configurations verified
- [ ] Repository metadata configured on GitHub
- [ ] Git tag v0.1.0 created and pushed
- [ ] GitHub release published
- [ ] Release announced (optional)

After completing all checklist items, the v0.1.0 MVP release is complete! 🎉
