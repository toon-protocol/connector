// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import "forge-std/Test.sol";
import "@openzeppelin/contracts/metatx/ERC2771Forwarder.sol";
import "../script/DeployLocal.s.sol";
import "../script/DeployTestnetCutover.s.sol";
import "../src/TokenNetwork.sol";
import "../src/TokenNetworkRegistry.sol";

/// @title DeployLocalTest
/// @notice Holds the local deploy (issue #1261) to the ERC-2771 wiring the testnet cutover
///         already proves: the registry must trust a real forwarder BEFORE it creates the
///         `TokenNetwork`, because `ERC2771Context`'s forwarder is an immutable constructor
///         argument and a `TokenNetwork` born trusting `address(0)` can never be repaired.
/// @dev Exercises `deploy()` rather than `run()` -- same split, and same reason, as
///      test/DeployTestnetCutover.fork.t.sol: `deploy()` carries the whole sequence with no
///      broadcast bookkeeping, so a test observes exactly what a broadcast would produce.
contract DeployLocalTest is Test {
    /// @notice The registry must hand every `TokenNetwork` it creates a real forwarder, not the
    ///         `address(0)` sentinel that means "meta-transactions disabled".
    function test_LocalDeploy_RegistryTrustsANonZeroForwarder() public {
        (, ERC2771Forwarder forwarder, TokenNetworkRegistry registry,,) = new DeployLocalScript().deploy();

        assertTrue(address(forwarder) != address(0), "the local deploy must deploy a real forwarder");
        assertGt(address(forwarder).code.length, 0, "the forwarder must be a real contract, not an EOA-shaped address");
        assertEq(
            registry.trustedForwarder(),
            address(forwarder),
            "the registry must carry the deployed forwarder, so every TokenNetwork it creates is born trusting it"
        );
    }

    /// @notice The registry trusting a forwarder is not the claim that matters -- the claim that
    ///         matters is that the value reached the `TokenNetwork`'s immutable constructor
    ///         argument, which only the network itself can answer.
    function test_LocalDeploy_TokenNetworkTrustsTheDeployedForwarder() public {
        (MockERC20 usdcToken, ERC2771Forwarder forwarder, TokenNetworkRegistry registry, TokenNetwork tokenNetwork,) =
            new DeployLocalScript().deploy();

        assertTrue(
            tokenNetwork.isTrustedForwarder(address(forwarder)),
            "the local TokenNetwork must trust the forwarder -- the value is immutable, so if it is wrong here it is wrong forever"
        );
        assertFalse(
            tokenNetwork.isTrustedForwarder(address(0)),
            "address(0) is the registry's `meta-transactions disabled` sentinel and must not be what this network was born with"
        );
        assertEq(
            registry.getTokenNetwork(address(usdcToken)),
            address(tokenNetwork),
            "the registry must resolve the mock USDC to the network it just created"
        );
    }

    /// @notice The end the whole fix exists for: a relayer submits a request alice signed, it
    ///         reaches the locally deployed `TokenNetwork`, and the contract authenticates alice
    ///         -- not the relayer and not the forwarder. Alice holds zero native gas throughout.
    /// @dev The participants are read out of the contract's own `ChannelOpened` event rather than
    ///      recomputed from `keccak256(p1, p2, epoch)` here, so the assertion cannot agree with
    ///      the contract by construction.
    function test_LocalDeploy_ForwardedCallReachesTokenNetworkAsTheOriginalSender() public {
        (, ERC2771Forwarder forwarder,, TokenNetwork tokenNetwork,) = new DeployLocalScript().deploy();

        uint256 alicePrivateKey = 0xA11CE;
        address alice = vm.addr(alicePrivateKey);
        address bob = makeAddr("bob");
        address relayer = makeAddr("relayer");
        vm.deal(relayer, 1 ether);

        assertEq(alice.balance, 0, "alice must hold zero native gas -- that is what the forwarder is for");

        vm.recordLogs();
        _executeForwarded(
            forwarder,
            tokenNetwork,
            relayer,
            alicePrivateKey,
            alice,
            abi.encodeCall(TokenNetwork.openChannel, (bob, 1 hours))
        );

        (bytes32 channelId, address participant1, address participant2) = _lastChannelOpened(address(tokenNetwork));

        assertTrue(
            (participant1 == alice && participant2 == bob) || (participant1 == bob && participant2 == alice),
            "the TokenNetwork must see alice as the sender -- a channel with the relayer or the forwarder in it means _msgSender() never unwrapped"
        );
        assertTrue(participant1 != relayer && participant2 != relayer, "the relayer must never become a participant");
        assertTrue(
            participant1 != address(forwarder) && participant2 != address(forwarder),
            "the forwarder must never become a participant"
        );

        (, TokenNetwork.ChannelState state,,, address storedP1, address storedP2) = tokenNetwork.channels(channelId);
        assertEq(
            uint256(state),
            uint256(TokenNetwork.ChannelState.Opened),
            "the forwarded call must really have opened the channel"
        );
        assertEq(storedP1, participant1);
        assertEq(storedP2, participant2);

        assertEq(alice.balance, 0, "alice must still hold zero native gas -- the relayer paid");
    }

    /// @notice The forwarder's EIP-712 domain name, read off the deployed contract via ERC-5267,
    ///         must be the one the testnet cutover and the proven ERC-2771 suite sign under. A
    ///         second convention would mean a signer built for one deployment produces invalid
    ///         signatures against the other, with nothing but a revert to say why.
    /// @dev The comparison against DeployTestnetCutoverScript holds by construction TODAY, because
    ///      #1261 made both scripts inherit one `Erc2771ForwarderDeployment.FORWARDER_NAME`. It is
    ///      kept because that is exactly what it guards: it starts failing the moment either
    ///      script grows a name of its own again. The assertion that can fail today is the one
    ///      against EXPECTED_FORWARDER_NAME, a literal this file owns.
    function test_LocalDeploy_ForwarderDomainNameMatchesTheTestnetCutover() public {
        (, ERC2771Forwarder forwarder,,,) = new DeployLocalScript().deploy();

        (, string memory deployedName, string memory version,, address verifyingContract,,) =
            IERC5267(address(forwarder)).eip712Domain();

        assertEq(deployedName, EXPECTED_FORWARDER_NAME, "the local forwarder's EIP-712 domain name must not drift");
        assertEq(
            deployedName,
            new DeployTestnetCutoverScript().FORWARDER_NAME(),
            "the local deploy and the testnet cutover must name the forwarder identically"
        );
        assertEq(version, "1", "ERC2771Forwarder's EIP-712 version is what the signing helpers assume");
        assertEq(verifyingContract, address(forwarder));
    }

    /// @notice The forwarder is inserted AFTER the registry, and that is not a style choice --
    ///         DeployLocalScript's own docblock carries why. This is the falsifier: a forwarder
    ///         deployed ahead of the registry moves the mock USDC, the registry and the
    ///         `TokenNetwork` at once, and this fails where the reordering is still on screen
    ///         rather than in the Rust gate or at `make local-verify` boot.
    /// @dev Two honest limits. The nonce BASE differs by caller and is not asserted: under
    ///      `forge test` the deploying account is this script contract (contract nonces start at
    ///      1), under `--broadcast` it is anvil account 0 (EOA nonces start at 0). And a contract's
    ///      nonce counts only its creations, so this pins the CREATION order, not the transaction
    ///      order -- an added plain CALL before `new TokenNetworkRegistry()` would move the
    ///      registry under a real broadcast with this still green. `regen-anvil-state.sh` plus
    ///      `evm_channel_domain_is_the_deployment.rs` remain the backstop for that.
    function test_LocalDeploy_ForwarderDoesNotMoveTheAddressesLocalConfigsHardcode() public {
        DeployLocalScript script = new DeployLocalScript();

        (MockERC20 usdcToken,, TokenNetworkRegistry registry, TokenNetwork tokenNetwork,) = script.deploy();

        assertEq(
            address(usdcToken),
            vm.computeCreateAddress(address(script), 1),
            "the mock USDC must stay the deploying account's FIRST contract creation"
        );
        assertEq(
            address(registry),
            vm.computeCreateAddress(address(script), 2),
            "the registry must stay the deploying account's SECOND contract creation -- deploying the forwarder ahead of it moves the registry, and the TokenNetwork with it"
        );
        assertEq(
            address(tokenNetwork),
            vm.computeCreateAddress(address(registry), 1),
            "the TokenNetwork must stay the registry's FIRST creation -- setTrustedForwarder creates nothing, so it must not come between"
        );
    }

    // ===== Helpers =====

    /// @notice The EIP-712 domain name a forwarded request must be signed under. Written as a
    ///         literal, and deliberately NOT read off the script under test: it is the same
    ///         literal test/TokenNetworkERC2771.t.sol and test/DeployTestnetCutover.fork.t.sol
    ///         sign with, so if the local deploy's forwarder name ever drifts from the proven
    ///         signing story, every forwarded call below reverts with an invalid signer.
    string internal constant EXPECTED_FORWARDER_NAME = "TokenNetworkForwarder";

    bytes32 internal constant FORWARD_REQUEST_TYPEHASH = keccak256(
        "ForwardRequest(address from,address to,uint256 value,uint256 gas,uint256 nonce,uint48 deadline,bytes data)"
    );

    bytes32 internal constant CHANNEL_OPENED_TOPIC = keccak256("ChannelOpened(bytes32,address,address,uint256)");

    function _domainSeparator(string memory name, address verifyingContract) internal view returns (bytes32) {
        bytes32 typeHash =
            keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)");
        return keccak256(abi.encode(typeHash, keccak256(bytes(name)), keccak256("1"), block.chainid, verifyingContract));
    }

    /// @dev Signs `data` as `from` and has `relayer` submit it through `forwarder` to `target`.
    function _executeForwarded(
        ERC2771Forwarder forwarder,
        TokenNetwork target,
        address relayer,
        uint256 signerKey,
        address from,
        bytes memory data
    ) internal {
        uint48 deadline = uint48(block.timestamp + 1 hours);
        bytes32 structHash = keccak256(
            abi.encode(
                FORWARD_REQUEST_TYPEHASH,
                from,
                address(target),
                uint256(0),
                uint256(500_000),
                forwarder.nonces(from),
                deadline,
                keccak256(data)
            )
        );
        bytes32 digest = keccak256(
            abi.encodePacked("\x19\x01", _domainSeparator(EXPECTED_FORWARDER_NAME, address(forwarder)), structHash)
        );
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(signerKey, digest);

        ERC2771Forwarder.ForwardRequestData memory request = ERC2771Forwarder.ForwardRequestData({
            from: from,
            to: address(target),
            value: 0,
            gas: 500_000,
            deadline: deadline,
            data: data,
            signature: abi.encodePacked(r, s, v)
        });

        // The relayer pays gas; the signer's balance never moves.
        vm.prank(relayer);
        forwarder.execute(request);
    }

    /// @notice The `ChannelOpened` the TokenNetwork itself emitted, decoded from its topics.
    function _lastChannelOpened(address tokenNetwork)
        internal
        returns (bytes32 channelId, address participant1, address participant2)
    {
        Vm.Log[] memory logs = vm.getRecordedLogs();
        for (uint256 i = logs.length; i > 0; i--) {
            Vm.Log memory entry = logs[i - 1];
            if (entry.emitter != tokenNetwork || entry.topics[0] != CHANNEL_OPENED_TOPIC) {
                continue;
            }
            return
                (
                    entry.topics[1],
                    address(uint160(uint256(entry.topics[2]))),
                    address(uint160(uint256(entry.topics[3])))
                );
        }
        revert("the forwarded openChannel emitted no ChannelOpened from the TokenNetwork");
    }
}
