#!/usr/bin/env node

/**
 * Fund Peers CLI Tool
 *
 * Funds peer wallets from the treasury wallet using ETH and ERC20 tokens.
 * Reads treasury credentials from testnet-wallets.json.
 */

import { Command } from 'commander';
import { ethers, Contract } from 'ethers';
import pino from 'pino';
import * as dotenv from 'dotenv';
import * as fs from 'fs';
import * as path from 'path';

// Load environment variables
dotenv.config();

// ERC20 ABI - minimal interface for transfers
const ERC20_ABI = [
  'function transfer(address to, uint256 amount) returns (bool)',
  'function balanceOf(address account) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function symbol() view returns (string)',
];

/**
 * Structure of testnet-wallets.json
 */
interface TestnetWallets {
  seed: string;
  funding: {
    evm: {
      address: string;
      privateKey: string;
      publicKey: string;
    };
  };
  contracts: {
    evm: {
      network: string;
      chainId: number;
      rpcUrl: string;
      tokenNetworkRegistry: string;
      token: {
        name: string;
        symbol: string;
        address: string;
        decimals: number;
      };
      tokenNetwork: string;
    };
  };
}

/**
 * Load testnet wallets configuration from JSON file
 */
function loadTestnetWallets(walletsPath: string): TestnetWallets {
  if (!fs.existsSync(walletsPath)) {
    throw new Error(`Wallets file not found: ${walletsPath}`);
  }

  const content = fs.readFileSync(walletsPath, 'utf-8');
  return JSON.parse(content) as TestnetWallets;
}

/**
 * Main CLI program
 */
const program = new Command();

program
  .name('fund-peers')
  .description('CLI tool to fund peer wallets from treasury (reads from testnet-wallets.json)')
  .version('0.2.0');

// Required options
program
  .requiredOption(
    '-p, --peers <list>',
    'Comma-separated list of peer names (e.g., peer1,peer2,peer3)'
  )
  .option('--eth-amount <amount>', 'ETH amount to send to each peer (in ETH)', '0.1')
  .option('--token-amount <amount>', 'ERC20 token amount to send to each peer', '1000')
  .option(
    '--wallets-file <path>',
    'Path to testnet-wallets.json',
    path.resolve(process.cwd(), 'testnet-wallets.json')
  )
  .option('--rpc-url <url>', 'Ethereum RPC URL (overrides testnet-wallets.json)')
  .option('--log-level <level>', 'Log level (debug, info, warn, error)', 'info');

// Add help examples
program.addHelpText(
  'after',
  `
Examples:
  # Fund 5 peers with default amounts (uses testnet-wallets.json from current directory)
  $ fund-peers --peers peer1,peer2,peer3,peer4,peer5

  # Fund peers with custom amounts
  $ fund-peers --peers peer1,peer2,peer3 --eth-amount 0.5 --token-amount 5000

  # Use custom wallets file
  $ fund-peers --peers peer1,peer2 --wallets-file /path/to/testnet-wallets.json

  # Override RPC URL
  $ fund-peers --peers peer1,peer2 --rpc-url http://localhost:8545

Configuration:
  The tool reads treasury credentials from testnet-wallets.json:
  - funding.evm.privateKey: Treasury wallet private key
  - contracts.evm.rpcUrl: Default RPC URL (can be overridden with --rpc-url)

  Peer addresses are read from environment variables:
  - PEER1_EVM_ADDRESS, PEER2_EVM_ADDRESS, etc.
  - Or from .env.peers file
`
);

// Action handler
program.action(async (options) => {
  // Create Pino logger
  const logger = pino({
    level: options.logLevel,
    transport: {
      target: 'pino-pretty',
      options: {
        colorize: true,
        translateTime: 'HH:MM:ss',
        ignore: 'pid,hostname',
      },
    },
  });

  try {
    logger.info(
      { options: { ...options, walletsFile: options.walletsFile } },
      'Starting fund-peers CLI'
    );

    // Load testnet wallets configuration
    logger.info({ path: options.walletsFile }, 'Loading testnet wallets configuration');
    const wallets = loadTestnetWallets(options.walletsFile);

    // Get treasury private key from testnet-wallets.json
    const treasuryPrivateKey = wallets.funding.evm.privateKey;
    if (!treasuryPrivateKey) {
      throw new Error(
        'Treasury EVM private key not found in testnet-wallets.json (funding.evm.privateKey)'
      );
    }

    // Determine RPC URL (CLI option overrides JSON config)
    const rpcUrl = options.rpcUrl || wallets.contracts.evm.rpcUrl || 'https://sepolia.base.org';

    // Parse options
    const peerNames = options.peers.split(',').map((p: string) => p.trim());
    const ethAmount = ethers.parseEther(options.ethAmount);
    const tokenAmount = BigInt(options.tokenAmount);

    // Connect to provider
    logger.info(
      { rpcUrl, network: wallets.contracts.evm.network },
      'Connecting to Ethereum provider'
    );
    const provider = new ethers.JsonRpcProvider(rpcUrl);
    const treasuryWallet = new ethers.Wallet(treasuryPrivateKey, provider);
    const treasuryAddress = treasuryWallet.address;

    // Verify treasury address matches
    if (treasuryAddress.toLowerCase() !== wallets.funding.evm.address.toLowerCase()) {
      logger.warn(
        { derived: treasuryAddress, expected: wallets.funding.evm.address },
        'Derived treasury address does not match address in wallets file'
      );
    }

    logger.info({ treasuryAddress }, 'Treasury wallet initialized');

    // Get treasury balance
    const balance = await provider.getBalance(treasuryAddress);
    logger.info({ balance: ethers.formatEther(balance) }, 'Treasury ETH balance');

    // Check if treasury has enough balance
    const totalNeeded = ethAmount * BigInt(peerNames.length);
    if (balance < totalNeeded) {
      throw new Error(
        `Insufficient treasury balance. Have: ${ethers.formatEther(balance)} ETH, ` +
          `Need: ${ethers.formatEther(totalNeeded)} ETH for ${peerNames.length} peers`
      );
    }

    // Load .env.peers if it exists (for peer addresses)
    const envPeersPath = path.resolve(process.cwd(), '.env.peers');
    if (fs.existsSync(envPeersPath)) {
      logger.info({ path: envPeersPath }, 'Loading peer addresses from .env.peers');
      dotenv.config({ path: envPeersPath });
    }

    // Get peer addresses from environment
    const peerAddresses: Record<string, string> = {};
    for (const peerName of peerNames) {
      const envVar = `${peerName.toUpperCase()}_EVM_ADDRESS`;
      const address = process.env[envVar];

      if (!address) {
        logger.warn(
          { peerName, envVar },
          'Peer address not found in environment, generating new address'
        );

        // Generate a new wallet for this peer
        const peerWallet = ethers.Wallet.createRandom();
        peerAddresses[peerName] = peerWallet.address;

        logger.info(
          {
            peerName,
            address: peerWallet.address,
            privateKey: peerWallet.privateKey,
          },
          'Generated new wallet for peer (SAVE THIS PRIVATE KEY!)'
        );
      } else {
        peerAddresses[peerName] = address;
        logger.info({ peerName, address }, 'Loaded peer address from environment');
      }
    }

    // Fund each peer with ETH
    logger.info({ peerCount: peerNames.length }, 'Funding peers with ETH');

    for (const peerName of peerNames) {
      const peerAddress = peerAddresses[peerName];

      // Check current balance of peer
      const peerBalance = await provider.getBalance(peerAddress as string);
      logger.info(
        { peerName, peerAddress, currentBalance: ethers.formatEther(peerBalance) },
        'Current peer balance'
      );

      logger.info(
        { peerName, peerAddress, amount: ethers.formatEther(ethAmount) },
        'Sending ETH to peer'
      );

      try {
        const tx = await treasuryWallet.sendTransaction({
          to: peerAddress,
          value: ethAmount,
        });

        logger.info(
          { peerName, txHash: tx.hash },
          'ETH transaction sent, waiting for confirmation'
        );

        await tx.wait();

        logger.info({ peerName, txHash: tx.hash }, 'ETH transfer confirmed');
      } catch (error) {
        const errorMessage = error instanceof Error ? error.message : String(error);
        logger.error({ peerName, error: errorMessage }, 'Failed to send ETH to peer');
      }
    }

    logger.info('ETH funding complete');

    // Fund each peer with ERC20 tokens
    const tokenConfig = wallets.contracts.evm.token;
    if (tokenConfig && tokenConfig.address && tokenAmount > 0n) {
      logger.info(
        {
          tokenAddress: tokenConfig.address,
          tokenSymbol: tokenConfig.symbol,
          tokenDecimals: tokenConfig.decimals,
          peerCount: peerNames.length,
        },
        'Funding peers with ERC20 tokens'
      );

      // Create token contract instance
      const tokenContract = new Contract(tokenConfig.address, ERC20_ABI, treasuryWallet);

      // Get treasury token balance using getFunction for proper typing
      const balanceOf = tokenContract.getFunction('balanceOf');
      const transfer = tokenContract.getFunction('transfer');

      // Get treasury token balance
      const treasuryTokenBalance = await balanceOf(treasuryAddress);
      const decimals = tokenConfig.decimals || 18;
      const formattedBalance = ethers.formatUnits(treasuryTokenBalance, decimals);
      logger.info(
        { balance: formattedBalance, symbol: tokenConfig.symbol },
        'Treasury token balance'
      );

      // Calculate token amount with decimals
      const tokenAmountWithDecimals = tokenAmount * BigInt(10 ** decimals);
      const totalTokensNeeded = tokenAmountWithDecimals * BigInt(peerNames.length);

      if (treasuryTokenBalance < totalTokensNeeded) {
        logger.warn(
          {
            have: formattedBalance,
            need: ethers.formatUnits(totalTokensNeeded, decimals),
            symbol: tokenConfig.symbol,
          },
          'Insufficient treasury token balance - will transfer what we can'
        );
      }

      // Transfer tokens to each peer
      for (const peerName of peerNames) {
        const peerAddress = peerAddresses[peerName];

        // Check current token balance of peer
        const peerTokenBalance = await balanceOf(peerAddress);
        logger.info(
          {
            peerName,
            peerAddress,
            currentBalance: ethers.formatUnits(peerTokenBalance, decimals),
            symbol: tokenConfig.symbol,
          },
          'Current peer token balance'
        );

        logger.info(
          {
            peerName,
            peerAddress,
            amount: tokenAmount.toString(),
            symbol: tokenConfig.symbol,
          },
          'Sending tokens to peer'
        );

        try {
          const tx = await transfer(peerAddress, tokenAmountWithDecimals);

          logger.info(
            { peerName, txHash: tx.hash },
            'Token transaction sent, waiting for confirmation'
          );

          await tx.wait();

          logger.info(
            { peerName, txHash: tx.hash, symbol: tokenConfig.symbol },
            'Token transfer confirmed'
          );
        } catch (error) {
          const errorMessage = error instanceof Error ? error.message : String(error);
          logger.error({ peerName, error: errorMessage }, 'Failed to send tokens to peer');
        }
      }

      logger.info({ symbol: tokenConfig.symbol }, 'Token funding complete');
    } else {
      logger.info('Skipping token funding (no token configured or amount is 0)');
    }

    logger.info('All peers funded successfully');

    // Summary
    logger.info(
      {
        peersCount: peerNames.length,
        ethPerPeer: ethers.formatEther(ethAmount),
        tokenPerPeer: tokenAmount.toString(),
        treasuryAddress,
        network: wallets.contracts.evm.network,
      },
      'Funding complete'
    );

    // Display peer addresses
    logger.info('Peer Addresses:');
    for (const [peerName, address] of Object.entries(peerAddresses)) {
      logger.info(`  ${peerName}: ${address}`);
    }

    process.exit(0);
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : String(error);
    logger.error({ error: errorMessage }, 'Failed to fund peers');
    process.exit(1);
  }
});

// Parse arguments
program.parse(process.argv);
