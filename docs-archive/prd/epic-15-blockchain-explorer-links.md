# Epic 15: Blockchain Explorer Navigation Links

## Brownfield Enhancement

This epic enhances the existing Agent Explorer UI (Epic 11, Epic 12) to add clickable blockchain explorer links for all wallet addresses and transaction hashes, enabling users to navigate to Aptos Explorer, Base Sepolia Etherscan, and XRP Testnet Explorer with a single click from any address or transaction hash displayed in the UI.

---

## Epic Goal

Transform static wallet addresses and transaction hashes throughout the Agent Explorer into interactive, clickable links that open the corresponding blockchain explorer in a new tab, improving the user's ability to inspect on-chain state and verify transactions across all three settlement chains (Aptos, Base, XRP).

---

## Epic Description

### Existing System Context

- **Current relevant functionality:** Agent Explorer displays wallet addresses and transaction hashes in multiple views (WalletOverview, PaymentChannelCard, EventDetailPanel, AccountsView) but they are static text with copy-to-clipboard functionality only.
- **Technology stack:** React 18, TypeScript, shadcn/ui v4, TailwindCSS, custom FieldDisplay components (AddressField, HexField, etc.)
- **Integration points:**
  - `WalletOverview.tsx` - displays EVM addresses, XRP addresses, channel IDs
  - `PaymentChannelCard.tsx` - displays channel addresses and peer addresses
  - `FieldDisplay.tsx` - provides reusable field components (AddressField, HexField, PeerField)
  - `EventDetailPanel.tsx` - displays event-specific addresses and transaction hashes
  - `AccountsView.tsx` - displays peer account information

### Enhancement Details

**What's being added/changed:**

1. **Explorer URL Builder Utility:** Create a centralized utility function that maps addresses/transaction hashes to their respective blockchain explorer URLs:
   - **Aptos Testnet:** `https://explorer.aptoslabs.com/account/{address}?network=testnet`
   - **Aptos Testnet Transactions:** `https://explorer.aptoslabs.com/txn/{txHash}?network=testnet`
   - **Base Sepolia (Etherscan):** `https://sepolia.basescan.org/address/{address}`
   - **Base Sepolia Transactions:** `https://sepolia.basescan.org/tx/{txHash}`
   - **XRP Testnet:** `https://testnet.xrpl.org/accounts/{address}`
   - **XRP Testnet Transactions:** `https://testnet.xrpl.org/transactions/{txHash}`

2. **Enhanced Field Components:** Update `FieldDisplay.tsx` components to accept optional blockchain explorer links:
   - Add `explorerUrl` prop to `AddressField`, `HexField`
   - Add external link icon (Lucide `ExternalLink`) next to copy button
   - Style links with appropriate hover states and visual indicators

3. **Smart Address Detection:** Implement address type detection logic to automatically determine which blockchain an address belongs to:
   - **Aptos addresses:** Start with `0x` and are 66 characters long (64 hex chars + `0x`)
   - **EVM addresses:** Start with `0x` and are 42 characters long (40 hex chars + `0x`)
   - **XRP addresses:** Start with `r` and are typically 25-35 characters (base58 encoded)
   - **Transaction hashes:** Both Aptos and EVM use 66-character `0x` prefixed hashes (require context to distinguish)

4. **Component Integration:** Update all components that display addresses to use the new explorer link functionality:
   - `WalletOverview.tsx` - EVM address, XRP address, Aptos address (if present), channel IDs, peer addresses
   - `PaymentChannelCard.tsx` - channel address, peer address
   - `AccountsView.tsx` - peer account addresses
   - `EventDetailPanel.tsx` - event-specific addresses and transaction hashes
   - Any TOON event rendering that includes blockchain addresses

**How it integrates:**

All changes are within the existing `packages/connector/explorer-ui/` frontend. No backend changes required. The enhancement adds a new utility module (`explorer-links.ts`) and updates existing display components to consume it. Address type detection uses simple string pattern matching based on length and prefix.

**Success criteria:**

- Clicking any EVM address opens Base Sepolia Etherscan in a new tab
- Clicking any XRP address opens XRP Testnet Explorer in a new tab
- Clicking any Aptos address opens Aptos Explorer Testnet in a new tab
- Clicking any transaction hash opens the appropriate explorer (requires context from telemetry event to distinguish Aptos vs EVM)
- External link icon is visually consistent across all address fields
- Hover states provide clear affordance that addresses are clickable
- Copy-to-clipboard functionality remains intact alongside explorer links
- All changes verified with Docker Agent Society test data showing real addresses

---

## Stories

### Story 29.1: Explorer URL Builder Utility & Address Type Detection

**Goal:** Create a centralized utility for building blockchain explorer URLs and detecting address types based on format patterns.

**Scope:**

- Create `packages/connector/explorer-ui/src/lib/explorer-links.ts` utility module
- Implement `detectAddressType(address: string): 'aptos' | 'evm' | 'xrp' | 'unknown'` function
- Implement `getExplorerUrl(address: string, type?: 'address' | 'tx', chain?: 'aptos' | 'evm' | 'xrp'): string | null` function
- Add unit tests for address detection edge cases (short addresses, invalid formats, mixed case)
- Export explorer base URLs as constants for testnet/mainnet configuration

**Acceptance Criteria:**

- [ ] `detectAddressType()` correctly identifies Aptos addresses (0x + 66 chars)
- [ ] `detectAddressType()` correctly identifies EVM addresses (0x + 42 chars)
- [ ] `detectAddressType()` correctly identifies XRP addresses (r prefix + base58)
- [ ] `getExplorerUrl()` returns correct URL for each blockchain type
- [ ] Unit tests cover edge cases (null, empty string, malformed addresses)
- [ ] TypeScript types exported for address detection results

**Technical Notes:**

- Aptos addresses are case-insensitive hex
- EVM addresses should be checksummed but detection works on unchecked format
- XRP addresses use base58 (charset: rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz)
- Transaction hashes for both Aptos and EVM are 66 chars (0x + 64 hex), so `getExplorerUrl()` should accept optional `chain` parameter to disambiguate

---

### Story 29.2: Enhanced AddressField and HexField Components

**Goal:** Update core field display components to support optional blockchain explorer links with external link icons.

**Scope:**

- Update `AddressField` component in `FieldDisplay.tsx`:
  - Add optional `explorerUrl?: string` prop
  - Render external link icon (Lucide `ExternalLink`) when `explorerUrl` is provided
  - Add click handler to open explorer URL in new tab (`target="_blank" rel="noopener noreferrer"`)
  - Style address text as a link (blue color, underline on hover) when clickable
- Update `HexField` component similarly for transaction hash support
- Add hover states and focus states for accessibility
- Ensure copy button and explorer link button are visually distinct

**Acceptance Criteria:**

- [ ] `AddressField` accepts `explorerUrl` prop and renders external link icon
- [ ] Clicking address or external link icon opens explorer in new tab
- [ ] Hover state shows underline and blue color on address text
- [ ] Copy button remains functional and visually separated from external link
- [ ] `HexField` supports same explorer link pattern for transaction hashes
- [ ] Focus states work correctly for keyboard navigation
- [ ] Storybook or visual test shows both linked and non-linked address variants

**Technical Notes:**

- Use `window.open(explorerUrl, '_blank', 'noopener,noreferrer')` for security
- External link icon from `lucide-react`: `import { ExternalLink } from 'lucide-react'`
- Link button should have `aria-label` for screen readers (e.g., "View on Aptos Explorer")

---

### Story 29.3: WalletOverview Explorer Link Integration

**Goal:** Add blockchain explorer links to all addresses displayed in the WalletOverview component.

**Scope:**

- Update `WalletOverview.tsx` to use enhanced `AddressField` with explorer URLs:
  - EVM address → Base Sepolia Etherscan
  - XRP address → XRP Testnet Explorer
  - Aptos address (if present in future) → Aptos Explorer
  - EVM channel IDs → Base Sepolia Etherscan (contract addresses)
  - XRP channel IDs → XRP Testnet Explorer (channel ID lookup if supported, otherwise account)
- Use `getExplorerUrl()` utility to generate URLs based on address type
- Update `CopyableAddress` inline component to support explorer links
- Verify with Docker Agent Society test data showing real EVM and XRP addresses

**Acceptance Criteria:**

- [ ] EVM address in header opens Base Sepolia Etherscan
- [ ] XRP address in header opens XRP Testnet Explorer
- [ ] EVM channel IDs in channel table open Base Sepolia Etherscan
- [ ] XRP channel IDs in channel table open XRP Testnet Explorer (or XRP account if channel ID not supported)
- [ ] All links open in new tab with correct URL format
- [ ] Visual verification with Docker Agent Society test data
- [ ] Copy-to-clipboard functionality unaffected

**Technical Notes:**

- EVM channel IDs are Ethereum contract addresses (42 chars, 0x prefix)
- XRP channel IDs are 64-character hex strings (may not have direct explorer support - link to account instead)
- Test with actual Docker Agent Society test to ensure addresses match expected formats

---

## Compatibility Requirements

- [x] Existing APIs remain unchanged (no backend changes)
- [x] Database schema changes are backward compatible (N/A - no DB changes)
- [x] UI changes follow existing patterns (FieldDisplay components)
- [x] Performance impact is minimal (client-side URL building only)

---

## Risk Mitigation

- **Primary Risk:** Incorrect address type detection causing wrong explorer links (e.g., linking an Aptos address to Etherscan)
- **Mitigation:**
  - Comprehensive unit tests for address type detection
  - Visual verification with real Docker Agent Society test data
  - Fallback to copy-only mode if address type is unknown
  - Add telemetry event context (chain type) to disambiguate transaction hashes
- **Rollback Plan:** Remove `explorerUrl` prop from components, leaving copy-to-clipboard as only interaction

---

## Definition of Done

- [ ] All stories completed with acceptance criteria met
- [ ] Address type detection tested with real Docker Agent Society addresses
- [ ] All three blockchain explorers (Aptos, Base, XRP) link correctly
- [ ] Existing copy-to-clipboard functionality verified
- [ ] Visual regression testing passes (no unexpected layout changes)
- [ ] Keyboard navigation and accessibility verified
- [ ] No performance degradation (link generation is O(1) string operations)
- [ ] Documentation updated (optional: add developer note about explorer URL utility)

---

## Technical Implementation Notes

### Address Format Reference

| Blockchain | Address Format      | Example                                                              | Length      |
| ---------- | ------------------- | -------------------------------------------------------------------- | ----------- |
| Aptos      | `0x` + 64 hex chars | `0xb206e544e69642e894f4eb4d2ba8b6e2b26bf1fd4b5a76cfc0d73c55ca725b6a` | 66 chars    |
| EVM (Base) | `0x` + 40 hex chars | `0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb`                          | 42 chars    |
| XRP        | `r` + base58        | `r3rfPzeWF9gSwi1zBP664vJGavk9faAkpR`                                 | 25-35 chars |

### Explorer URL Patterns

```typescript
// Aptos Testnet
https://explorer.aptoslabs.com/account/{address}?network=testnet
https://explorer.aptoslabs.com/txn/{txHash}?network=testnet

// Base Sepolia (Etherscan)
https://sepolia.basescan.org/address/{address}
https://sepolia.basescan.org/tx/{txHash}

// XRP Testnet
https://testnet.xrpl.org/accounts/{address}
https://testnet.xrpl.org/transactions/{txHash}
```

### Example Usage

```typescript
import { getExplorerUrl, detectAddressType } from '@/lib/explorer-links';

const evmAddress = '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb';
const type = detectAddressType(evmAddress); // 'evm'
const url = getExplorerUrl(evmAddress, 'address');
// https://sepolia.basescan.org/address/0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb

// Component usage
<AddressField
  label="EVM Address"
  value={evmAddress}
  explorerUrl={getExplorerUrl(evmAddress, 'address')}
/>
```

---

## Future Enhancements (Out of Scope)

- Mainnet explorer support (currently testnet-only)
- Configurable explorer URLs via environment variables
- Support for other blockchain explorers (Solana, Cosmos, etc.)
- Transaction hash auto-detection from telemetry events
- Deep linking to specific contract methods or event logs in explorers
