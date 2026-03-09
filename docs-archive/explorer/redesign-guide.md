# Explorer UI Redesign Guide

> Epic 18 - Network Operations Center Aesthetic

This guide documents the design philosophy, color palette, typography, component hierarchy, and animation catalog for the Explorer UI redesign completed in Epic 18.

## Table of Contents

- [Design Philosophy](#design-philosophy)
- [Color Palette](#color-palette)
- [Typography](#typography)
- [Component Hierarchy](#component-hierarchy)
- [Animation Catalog](#animation-catalog)
- [CSS Source Reference](#css-source-reference)

---

## Design Philosophy

### NOC Aesthetic Inspiration

The Explorer UI is designed to resemble a **Network Operations Center (NOC)** dashboard - the type of interface used in data centers, financial trading floors, and 24/7 monitoring facilities. This aesthetic was chosen for several reasons:

1. **24/7 Viewing Optimization**: NOC interfaces are designed for extended viewing without eye strain. The dark background with high-contrast accent colors reduces fatigue during long monitoring sessions.

2. **Information Density**: ILP connectors process thousands of packets per second. The NOC aesthetic allows dense data presentation without overwhelming the operator.

3. **Status-at-a-Glance**: Critical metrics are immediately visible. Color coding (cyan for pending, green for success, red for failure) provides instant status recognition.

4. **Professional Credibility**: The NOC aesthetic conveys that this is serious infrastructure software, suitable for production financial systems.

### Design Principles

- **Dark-First**: Deep space background (`hsl(222, 47%, 6%)`) as the foundation
- **Accent Hierarchy**: Cyan (primary/neutral), Emerald (success), Rose (error)
- **Monospace for Data**: Technical values displayed in monospace fonts
- **Subtle Animations**: Motion enhances understanding without distraction
- **Accessibility**: `prefers-reduced-motion` support for all animations

---

## Color Palette

The color palette is carefully tuned for optimal contrast and readability on dark backgrounds.

### Core Colors

| Color            | HSL Value            | Hex       | CSS Variable         | Usage                       |
| ---------------- | -------------------- | --------- | -------------------- | --------------------------- |
| Background       | `hsl(222, 47%, 6%)`  | `#0D1829` | `--background`       | Deep space background       |
| Card             | `hsl(222, 47%, 8%)`  | `#111B2C` | `--card`             | Elevated cards and panels   |
| Border           | `hsl(217, 33%, 14%)` | `#1E2A3B` | `--border`           | Subtle borders and dividers |
| Foreground       | `hsl(210, 40%, 98%)` | `#F8FAFC` | `--foreground`       | Primary text                |
| Muted            | `hsl(217, 33%, 17%)` | `#242D3D` | `--muted`            | Secondary backgrounds       |
| Muted Foreground | `hsl(215, 20%, 65%)` | `#94A3B8` | `--muted-foreground` | Secondary text              |

### Accent Colors

| Color   | Hex       | Tailwind Class     | Usage                                               |
| ------- | --------- | ------------------ | --------------------------------------------------- |
| Cyan    | `#06B6D4` | `text-cyan-400`    | PREPARE packets, neutral status, primary actions    |
| Emerald | `#10B981` | `text-emerald-400` | FULFILL packets, success states, active connections |
| Rose    | `#F43F5E` | `text-rose-400`    | REJECT packets, error states, warnings              |
| Amber   | `#F59E0B` | `text-amber-400`   | Pending states, warnings                            |

### Usage Examples

```tsx
// Packet type coloring
<span className="text-cyan-400">PREPARE</span>
<span className="text-emerald-400">FULFILL</span>
<span className="text-rose-400">REJECT</span>

// Status indicators
<div className="bg-emerald-500/20 text-emerald-400">Connected</div>
<div className="bg-rose-500/20 text-rose-400">Disconnected</div>
```

---

## Typography

### Font Families

| Category  | Font Stack                             | Usage                                       |
| --------- | -------------------------------------- | ------------------------------------------- |
| UI Text   | `Inter, system-ui, sans-serif`         | Headers, labels, descriptions               |
| Monospace | `JetBrains Mono, Fira Code, monospace` | ILP addresses, packet data, amounts, hashes |

### Type Scale

| Element        | Size              | Weight | Line Height |
| -------------- | ----------------- | ------ | ----------- |
| Page Title     | `1.5rem` (24px)   | 600    | 1.2         |
| Section Header | `1.125rem` (18px) | 600    | 1.3         |
| Card Title     | `0.875rem` (14px) | 500    | 1.4         |
| Body Text      | `0.875rem` (14px) | 400    | 1.5         |
| Small/Caption  | `0.75rem` (12px)  | 400    | 1.4         |
| Monospace Data | `0.75rem` (12px)  | 400    | 1.4         |

### Typography Guidelines

1. **Use monospace for technical data**: ILP addresses, amounts, packet IDs, timestamps
2. **Use system fonts for UI elements**: Buttons, labels, navigation
3. **Maintain consistent sizing**: Don't mix type scales arbitrarily
4. **High contrast ratios**: All text meets WCAG AA contrast requirements against dark backgrounds

---

## Component Hierarchy

### Application Structure

```
App.tsx
├── Header.tsx                    # Branding, connection status, navigation
│   ├── Logo + Branding
│   ├── Connection Status Badge
│   └── Navigation Tabs
│
├── Tab Content (conditional)
│   ├── Dashboard.tsx             # Default landing page
│   │   ├── Metrics Grid (4 cards)
│   │   └── Live Packet Flow
│   │
│   ├── EventTable.tsx            # Packets tab
│   │   ├── FilterBar.tsx
│   │   └── Event Table + Detail Panel
│   │
│   ├── AccountsView.tsx          # Accounts tab
│   │   ├── AccountCard.tsx (per account)
│   │   │   ├── BalanceHistoryChart.tsx
│   │   │   └── SettlementTimeline.tsx
│   │   └── Empty State
│   │
│   ├── PeersView.tsx             # Peers tab
│   │   ├── Peer Cards
│   │   └── Routing Table
│   │
│   └── KeyManager.tsx            # Keys tab
│       ├── Key Display Cards
│       └── Copy Actions
│
└── UI Components (shadcn/ui)
    ├── Card, Button, Badge
    ├── Table, Tabs
    ├── Tooltip, Dialog
    └── Input, Select
```

### Component Responsibilities

| Component                 | Responsibility                                        |
| ------------------------- | ----------------------------------------------------- |
| `App.tsx`                 | Tab state management, keyboard shortcuts, layout      |
| `Header.tsx`              | Branding, WebSocket connection status, tab navigation |
| `Dashboard.tsx`           | Metrics grid, live packet flow visualization          |
| `EventTable.tsx`          | Packet list, filtering, search, detail panel          |
| `FilterBar.tsx`           | Filter controls for packet filtering                  |
| `AccountsView.tsx`        | Account list, balance display                         |
| `AccountCard.tsx`         | Individual account with balance history               |
| `BalanceHistoryChart.tsx` | Balance trend visualization                           |
| `SettlementTimeline.tsx`  | Settlement event timeline                             |
| `PeersView.tsx`           | Peer connection list, routing entries                 |
| `KeyManager.tsx`          | Key display and copy functionality                    |

---

## Animation Catalog

All animations are defined in `packages/connector/explorer-ui/src/index.css` and support `prefers-reduced-motion`.

### Hover Effects

| Class            | Effect                                  | Usage                     |
| ---------------- | --------------------------------------- | ------------------------- |
| `.hover-elevate` | Subtle scale (1.02) and shadow on hover | Cards, clickable elements |

```css
.hover-elevate {
  transition:
    transform 0.2s ease,
    box-shadow 0.2s ease;
}
.hover-elevate:hover {
  transform: translateY(-2px) scale(1.02);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3);
}
```

### Entry Animations

| Class         | Effect                   | Duration | Usage                   |
| ------------- | ------------------------ | -------- | ----------------------- |
| `.fade-in-up` | Fade in while sliding up | 0.3s     | New content appearing   |
| `.stagger-1`  | Delay 100ms              | -        | First item in sequence  |
| `.stagger-2`  | Delay 200ms              | -        | Second item in sequence |
| `.stagger-3`  | Delay 300ms              | -        | Third item in sequence  |
| `.stagger-4`  | Delay 400ms              | -        | Fourth item in sequence |

```tsx
// Staggered card animation
<div className="fade-in-up stagger-1">Card 1</div>
<div className="fade-in-up stagger-2">Card 2</div>
<div className="fade-in-up stagger-3">Card 3</div>
<div className="fade-in-up stagger-4">Card 4</div>
```

### Progress Animations

| Class                | Effect                    | Usage                         |
| -------------------- | ------------------------- | ----------------------------- |
| `.progress-smooth`   | Smooth width transition   | Progress bars, loading states |
| `.status-transition` | Color/opacity transitions | Status changes                |

### Accessibility

All animations respect the user's motion preferences:

```css
@media (prefers-reduced-motion: reduce) {
  .fade-in-up,
  .hover-elevate,
  .progress-smooth,
  .status-transition {
    animation: none;
    transition: none;
  }
}
```

---

## CSS Source Reference

The complete theme and animation definitions are located in:

```
packages/connector/explorer-ui/src/index.css
```

### Key Sections

1. **CSS Variables** (lines 1-50): Theme colors, radii, fonts
2. **Base Styles** (lines 51-100): Body, scrollbar, selection
3. **Animation Keyframes** (lines 101-150): `@keyframes` definitions
4. **Utility Classes** (lines 151-200): `.hover-elevate`, `.fade-in-up`, etc.
5. **Tailwind Directives**: `@tailwind base/components/utilities`

### Customizing the Theme

To customize colors, modify the CSS variables in the `:root` selector:

```css
:root {
  --background: 222 47% 6%;
  --foreground: 210 40% 98%;
  --primary: 199 89% 48%; /* Cyan */
  /* ... */
}
```
