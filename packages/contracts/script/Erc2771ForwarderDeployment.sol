// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import "@openzeppelin/contracts/metatx/ERC2771Forwarder.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";

/**
 * @title Erc2771ForwarderDeployment
 * @notice The one ERC-2771 deploy sequence, shared by every script that creates a `TokenNetwork`
 *         through a `TokenNetworkRegistry`. Extracted from DeployTestnetCutoverScript by issue
 *         #1261, which found DeployLocalScript had never been given the same treatment: it
 *         deployed no forwarder at all, so every locally deployed `TokenNetwork` was born
 *         trusting `address(0)` and could not host a meta-transaction.
 * @dev The ORDER is the whole content of this file. `TokenNetwork` is `ERC2771Context`, whose
 *      forwarder is an immutable constructor argument, and the registry reads its own
 *      `trustedForwarder` when it constructs one. So the forwarder must exist and be set on the
 *      registry BEFORE `createTokenNetwork`; afterwards is too late, and no later call can
 *      repair it -- `setTrustedForwarder` only reaches `TokenNetwork`s created from then on, and
 *      replacing a live `TokenNetwork` changes the EIP-712 domain every existing claim was
 *      signed under (ADR 0024).
 *
 *      A caller is free to deploy the registry before or after the forwarder -- only the
 *      forwarder-before-`createTokenNetwork` edge matters to the contracts. DeployLocalScript
 *      deploys the registry first on purpose: the local chain's contract addresses are a
 *      deterministic function of the deployer's nonce sequence, and committed configs across
 *      `local/` hardcode the `TokenNetwork` this deploy lands at.
 */
abstract contract Erc2771ForwarderDeployment {
    /// @notice The forwarder's EIP-712 domain name. One constant for every deploy and every test,
    ///         so a signer built against one deployment signs valid requests for all of them --
    ///         the domain separator is over this name, and a drifted name reverts every forwarded
    ///         call with an invalid signer.
    string public constant FORWARDER_NAME = "TokenNetworkForwarder";

    /// @notice Deploy a forwarder, make `registry` mint forwarder-aware networks, and create the
    ///         `TokenNetwork` for `token` -- in that order, which is the only order that works.
    /// @param registry The registry that will create the network. Must already be deployed and
    ///        owned by the caller (`setTrustedForwarder` is `onlyOwner`).
    /// @param token The ERC20 the `TokenNetwork` settles in.
    /// @return forwarder The freshly deployed forwarder, now the registry's trusted one.
    /// @return tokenNetwork The network created through `registry`, immutably trusting `forwarder`.
    function _createForwarderAwareTokenNetwork(TokenNetworkRegistry registry, address token)
        internal
        returns (ERC2771Forwarder forwarder, TokenNetwork tokenNetwork)
    {
        forwarder = new ERC2771Forwarder(FORWARDER_NAME);
        registry.setTrustedForwarder(address(forwarder));
        tokenNetwork = TokenNetwork(registry.createTokenNetwork(token));
    }
}
