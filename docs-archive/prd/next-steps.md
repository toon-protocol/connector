# Next Steps

## UX Expert Prompt

You are the UX/Design expert for the ILP Connector Visualization project. Please review the attached PRD (docs/prd.md) and Project Brief (docs/brief.md), then create detailed UX specifications including:

1. **User Journey Flows** - Map the three primary user journeys: (a) deploying network and observing first packet, (b) debugging failed packet routing, (c) experimenting with custom topology
2. **Dashboard Wireframes** - Design the visualization dashboard UI including network topology graph, packet animation layer, packet detail panel, node status panel, and log viewer
3. **Information Architecture** - Organize dashboard components for optimal observability and debugging workflows
4. **Interaction Design** - Specify how users interact with network graph (zoom, pan, click packets/nodes), log filtering, and panel management

Your designs should prioritize developer/researcher workflows and technical precision over aesthetic polish. Reference the UI Design Goals section in the PRD for constraints and direction.

## Architect Prompt

You are the Technical Architect for the ILP Connector Visualization project. Please review the attached PRD (docs/prd.md) and Project Brief (docs/brief.md), then create the technical architecture specification including:

1. **System Architecture** - Design the component architecture (connector nodes, dashboard, telemetry aggregation) with clear module boundaries and interfaces
2. **Data Flow Diagrams** - Document packet flow, BTP communication, and telemetry emission with sequence diagrams
3. **Telemetry Protocol Specification** - Define telemetry message schemas, WebSocket transport, and aggregation strategy
4. **Docker Architecture** - Specify container networking, Compose orchestration, and configuration management
5. **Testing Strategy** - Design unit, integration, and E2E testing approach with concrete examples
6. **Technology Selection Details** - Validate and refine tech stack decisions from PRD Technical Assumptions section

Follow the functional requirements (FR1-FR20), non-functional requirements (NFR1-NFR12), and technical assumptions from the PRD. Ensure all architectural decisions support the core goal: observable ILP network with real-time visualization.
