# User Interface Design Goals

## Overall UX Vision

The dashboard should feel like a "mission control" for Interledger packet flow—clean, technical, and information-dense without being overwhelming. The primary metaphor is a network monitoring tool where users observe live traffic flowing through a system. The interface prioritizes **immediate comprehension** of network state and packet movement, with progressive disclosure of detailed information on demand. Visual design should emphasize clarity and technical precision over aesthetic flourish, similar to developer tools like Chrome DevTools or network analyzers like Wireshark.

## Key Interaction Paradigms

- **Real-time observation:** Users primarily watch packet flow passively; the visualization updates automatically without requiring user action
- **Inspect-on-demand:** Clicking packets or nodes reveals detailed information in side panels or overlays without disrupting the live visualization
- **Filter and focus:** Users can filter logs and packet types to reduce noise when debugging specific scenarios
- **Configuration-first startup:** Network topology is defined via config files before launch; runtime reconfiguration is out of scope for MVP
- **Single-page application:** All functionality accessible from one dashboard view without page navigation

## Core Screens and Views

1. **Network Topology View** - Primary screen showing graph visualization of connector nodes and their BTP connections
2. **Live Packet Animation Layer** - Overlay on topology view displaying animated packets moving between nodes
3. **Packet Detail Panel** - Expandable side panel showing full ILP packet structure (triggered by clicking packet)
4. **Node Status Panel** - Info panel showing individual connector routing table, active connections, and health status (triggered by clicking node)
5. **Log Stream Viewer** - Bottom panel or separate tab displaying filterable, scrollable structured logs from all connectors
6. **Network Configuration Summary** - Header or info panel showing current topology type, number of nodes, and overall health

## Accessibility: None

MVP focuses on developer/researcher audience using modern browsers. Accessibility features (screen reader support, keyboard navigation, WCAG compliance) are deferred to post-MVP phases. Basic usability (readable fonts, sufficient color contrast for packet type differentiation) will be ensured, but formal accessibility standards are not a requirement.

## Branding

Minimal technical aesthetic with focus on functionality over brand identity. Color palette should emphasize:

- **Functional color-coding:** Blue (Prepare), Green (Fulfill), Red (Reject) for packet types
- **Neutral background:** Dark theme preferred (reduces eye strain during extended debugging sessions)
- **Monospace fonts:** For logs and packet data to align with developer tool conventions
- **Network graph styling:** Clean, minimal node/edge styling (avoid decorative elements)

No corporate branding or logo required. Project name and version displayed in header. Typography should prioritize readability for technical content (code, addresses, hex data).

## Target Device and Platforms: Web Responsive (Desktop-first)

Primary target is **desktop browsers on development machines** (1920x1080 or higher resolution). Responsive design should gracefully handle down to 1366x768 laptop screens. Mobile and tablet support explicitly out of scope for MVP—network visualization requires screen real estate and is intended for desktop debugging workflows. UI should be usable on macOS, Linux, and Windows desktop environments without platform-specific adaptations.

---
