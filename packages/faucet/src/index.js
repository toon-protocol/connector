import express from 'express';
import cors from 'cors';
import { ethers } from 'ethers';

const app = express();
const PORT = process.env.PORT || 3500;

// Configuration
const RPC_URL = process.env.RPC_URL || 'http://anvil:8545';
const ETH_PRIVATE_KEY =
  process.env.ETH_PRIVATE_KEY ||
  '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d'; // Anvil Account 1
const TOKEN_PRIVATE_KEY =
  process.env.TOKEN_PRIVATE_KEY ||
  '0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80'; // Anvil Account 0 (deployer)
const TOKEN_ADDRESS = process.env.TOKEN_ADDRESS;
const ETH_AMOUNT = process.env.ETH_AMOUNT || '100'; // 100 ETH
const TOKEN_AMOUNT = process.env.TOKEN_AMOUNT || '10000'; // 10,000 USDC tokens

// ERC20 ABI (minimal)
const ERC20_ABI = [
  'function transfer(address to, uint256 amount) returns (bool)',
  'function balanceOf(address account) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function symbol() view returns (string)',
];

// Middleware
app.use(cors());
app.use(express.json());
app.use(express.static('public'));

// Setup provider and wallets
const provider = new ethers.JsonRpcProvider(RPC_URL);
const ethWallet = new ethers.Wallet(ETH_PRIVATE_KEY, provider);
const tokenWallet = new ethers.Wallet(TOKEN_PRIVATE_KEY, provider);

// Token contract instance (will be set after deployment)
let tokenContract = null;
let tokenSymbol = 'USDC';
let tokenDecimals = 18;

// Initialize token contract
async function initTokenContract() {
  if (!TOKEN_ADDRESS) {
    console.log('⚠️  TOKEN_ADDRESS not set. Waiting for contract deployment...');
    return false;
  }

  try {
    tokenContract = new ethers.Contract(TOKEN_ADDRESS, ERC20_ABI, tokenWallet);
    tokenSymbol = await tokenContract.symbol();
    tokenDecimals = await tokenContract.decimals();
    console.log(`✅ Token contract initialized: ${tokenSymbol} at ${TOKEN_ADDRESS}`);
    return true;
  } catch (error) {
    console.error('❌ Failed to initialize token contract:', error.message);
    return false;
  }
}

// Validate Ethereum address
function isValidAddress(address) {
  try {
    return ethers.isAddress(address);
  } catch {
    return false;
  }
}

// Health check
app.get('/health', (req, res) => {
  res.json({
    status: 'ok',
    tokenAddress: TOKEN_ADDRESS,
    tokenReady: !!tokenContract,
  });
});

// Get faucet info
app.get('/api/info', async (req, res) => {
  try {
    const ethBalance = await provider.getBalance(ethWallet.address);
    let tokenBalance = '0';

    if (tokenContract) {
      const balance = await tokenContract.balanceOf(tokenWallet.address);
      tokenBalance = ethers.formatUnits(balance, tokenDecimals);
    }

    res.json({
      ethAmount: ETH_AMOUNT,
      tokenAmount: TOKEN_AMOUNT,
      tokenSymbol,
      tokenAddress: TOKEN_ADDRESS,
      faucetBalances: {
        eth: ethers.formatEther(ethBalance),
        token: tokenBalance,
      },
      ready: !!tokenContract,
    });
  } catch (error) {
    res.status(500).json({
      error: 'Failed to get faucet info',
      message: error.message,
    });
  }
});

// Request tokens
app.post('/api/request', async (req, res) => {
  try {
    const { address } = req.body;

    // Validate address
    if (!address || !isValidAddress(address)) {
      return res.status(400).json({
        error: 'Invalid Ethereum address',
      });
    }

    // Check if token contract is ready
    if (!tokenContract) {
      const initialized = await initTokenContract();
      if (!initialized) {
        return res.status(503).json({
          error: 'Token contract not yet deployed',
          message: 'Please wait for contract deployment to complete',
        });
      }
    }

    console.log(`💧 Faucet request for ${address}`);

    // Send ETH
    const ethTx = await ethWallet.sendTransaction({
      to: address,
      value: ethers.parseEther(ETH_AMOUNT),
    });
    console.log(`  📤 Sending ${ETH_AMOUNT} ETH: ${ethTx.hash}`);

    // Send tokens
    const tokenAmount = ethers.parseUnits(TOKEN_AMOUNT, tokenDecimals);
    const tokenTx = await tokenContract.transfer(address, tokenAmount);
    console.log(`  📤 Sending ${TOKEN_AMOUNT} ${tokenSymbol}: ${tokenTx.hash}`);

    // Wait for confirmations
    await ethTx.wait();
    await tokenTx.wait();

    console.log(`  ✅ Faucet request completed for ${address}`);

    res.json({
      success: true,
      transactions: {
        eth: {
          hash: ethTx.hash,
          amount: ETH_AMOUNT,
        },
        token: {
          hash: tokenTx.hash,
          amount: TOKEN_AMOUNT,
          symbol: tokenSymbol,
        },
      },
    });
  } catch (error) {
    console.error('❌ Faucet request failed:', error);
    res.status(500).json({
      error: 'Faucet request failed',
      message: error.message,
    });
  }
});

// Start server
app.listen(PORT, async () => {
  console.log('');
  console.log('═══════════════════════════════════════════════');
  console.log('   🚰 Token Faucet');
  console.log('═══════════════════════════════════════════════');
  console.log(`   Port:          ${PORT}`);
  console.log(`   RPC URL:       ${RPC_URL}`);
  console.log(`   ETH per drip:  ${ETH_AMOUNT} ETH`);
  console.log(`   Token per drip: ${TOKEN_AMOUNT} ${tokenSymbol}`);
  console.log('═══════════════════════════════════════════════');
  console.log('');

  // Try to initialize token contract
  await initTokenContract();

  console.log('✅ Faucet is running!');
  console.log(`   UI: http://localhost:${PORT}`);
  console.log('');
});
