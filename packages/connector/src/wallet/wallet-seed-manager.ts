/**
 * Wallet Seed Manager - HD wallet master seed management
 * @packageDocumentation
 * @remarks
 * Implements BIP-39 mnemonic generation, AES-256-GCM encryption, and secure storage
 * for hierarchical deterministic (HD) wallet master seeds. Supports backup/recovery
 * with checksum validation and optional HSM/KMS integration (Epic 12).
 */

import { createCipheriv, createDecipheriv, createHash, pbkdf2Sync, randomBytes } from 'crypto';
import { promises as fs } from 'fs';
import * as path from 'path';
import pino from 'pino';
import { requireOptional } from '../utils/optional-require';

/**
 * KeyManager interface for HSM/KMS integration (Epic 12)
 * @remarks
 * Stub interface for future HSM/KMS integration. Epic 12 Story 12.2 will implement
 * concrete classes (AWSKMSKeyManager, VaultKeyManager, etc.).
 */
export interface KeyManager {
  storeSecret(name: string, value: string): Promise<void>;
  retrieveSecret(name: string): Promise<string>;
  deleteSecret(name: string): Promise<void>;
}

/**
 * Seed configuration options
 */
export interface SeedConfig {
  storageBackend: 'filesystem' | 'hsm';
  storagePath?: string; // Filesystem storage path (default: ./data/wallet)
  hsmConfig?: object; // HSM configuration (Epic 12)
}

/**
 * Master seed data structure
 * @remarks
 * Contains BIP-39 mnemonic phrase and derived 512-bit seed.
 * Encryption key is optional and used internally during encryption operations.
 */
export interface MasterSeed {
  mnemonic: string; // BIP-39 mnemonic phrase (12 or 24 words)
  seed: Buffer; // 512-bit seed derived from mnemonic
  createdAt: number; // Unix timestamp
  encryptionKey?: Buffer; // Optional: AES-256 encryption key
}

/**
 * Backup data structure with integrity validation
 * @remarks
 * Encrypted backup format with version metadata and SHA-256 checksum
 * for integrity validation before restore.
 */
export interface BackupData {
  version: string; // Backup format version (e.g., "1.0")
  createdAt: number; // Master seed creation timestamp
  encryptedSeed: string; // Base64-encoded encrypted mnemonic
  backupDate: number; // Backup creation timestamp
  checksum: string; // SHA-256 checksum for integrity validation
}

/**
 * Paper wallet data structure
 * @remarks
 * WARNING: Paper wallets contain unencrypted mnemonic phrase.
 * Store in physically secure location (safe, vault, etc.).
 */
export interface PaperWallet {
  mnemonic: string; // BIP-39 mnemonic phrase (unencrypted)
  qrCodeDataUrl: string; // QR code as data URL (data:image/png;base64,...)
  createdAt: number; // Paper wallet creation timestamp
}

/**
 * BIP-44 derivation path constants for agent wallets
 * @remarks
 * Account index 1 is used for agent wallets (index 0 reserved for platform wallet).
 * Format: m/44'/coinType'/account'/change/addressIndex
 */
export const DERIVATION_PATHS = {
  EVM: "m/44'/60'/1'/0", // Ethereum/Base L2 (coin type 60)
} as const;

/**
 * Custom error class for invalid mnemonic
 */
export class InvalidMnemonicError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InvalidMnemonicError';
  }
}

/**
 * Custom error class for decryption failures
 */
export class DecryptionError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'DecryptionError';
  }
}

/**
 * Custom error class for invalid backup data
 */
export class InvalidBackupError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'InvalidBackupError';
  }
}

/**
 * Custom error class for weak passwords
 */
export class WeakPasswordError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'WeakPasswordError';
  }
}

/**
 * HD Wallet Master Seed Manager
 * @remarks
 * Implements secure master seed generation, encryption, storage, and backup/recovery.
 * Uses BIP-39 for mnemonic generation, AES-256-GCM for encryption, and PBKDF2 for
 * password-based key derivation. Supports optional HSM/KMS integration via KeyManager.
 *
 * @example Generate and store master seed
 * ```typescript
 * const manager = new WalletSeedManager();
 * await manager.initialize();
 * const masterSeed = await manager.generateMasterSeed(256); // 24-word mnemonic
 * await manager.encryptAndStore(masterSeed, 'StrongP@ssw0rd123456');
 * ```
 *
 * @example Restore from backup
 * ```typescript
 * const manager = new WalletSeedManager();
 * await manager.initialize();
 * const masterSeed = await manager.decryptAndLoad('StrongP@ssw0rd123456');
 * console.log(`Restored seed created at: ${new Date(masterSeed.createdAt)}`);
 * ```
 */
export class WalletSeedManager {
  private logger: pino.Logger;
  private keyManager?: KeyManager;
  private config: SeedConfig;
  private salt?: Buffer;

  /**
   * Create WalletSeedManager instance
   * @param keyManager - Optional KeyManager for HSM/KMS integration (Epic 12)
   * @param config - Seed configuration (storage backend, paths, etc.)
   */
  constructor(keyManager?: KeyManager, config?: Partial<SeedConfig>) {
    this.keyManager = keyManager;
    this.config = {
      storageBackend: config?.storageBackend ?? 'filesystem',
      storagePath: config?.storagePath ?? './data/wallet',
      hsmConfig: config?.hsmConfig,
    };
    this.logger = pino({ name: 'wallet-seed-manager' });
  }

  /**
   * Initialize wallet seed manager
   * @remarks
   * Loads or generates encryption salt. Must be called before any other operations.
   */
  async initialize(): Promise<void> {
    this.salt = await this.initializeSalt();
    this.logger.info('WalletSeedManager initialized');
  }

  /**
   * Initialize encryption salt on first run or load existing salt
   * @returns 32-byte salt for PBKDF2 key derivation
   * @remarks
   * Generates random 32-byte salt using crypto.randomBytes on first run.
   * Salt is stored in ./data/wallet/encryption-salt and loaded on subsequent runs.
   * Salt is per-installation (shared across all seeds).
   */
  private async initializeSalt(): Promise<Buffer> {
    const saltPath = path.join(this.config.storagePath!, 'encryption-salt');

    try {
      // Try to load existing salt
      const saltData = await fs.readFile(saltPath);
      this.logger.info('Loaded existing encryption salt');
      return saltData;
    } catch (error) {
      // Salt file doesn't exist, generate new salt
      this.logger.info('Generating new encryption salt');
      const newSalt = randomBytes(32);

      // Create directory if not exists
      await fs.mkdir(path.dirname(saltPath), { recursive: true });

      // Write salt to file
      await fs.writeFile(saltPath, newSalt);
      this.logger.info({ saltPath }, 'Encryption salt saved');

      return newSalt;
    }
  }

  /**
   * Validate password meets complexity requirements
   * @param password - Password to validate
   * @returns true if password is strong
   * @throws WeakPasswordError if password doesn't meet requirements
   * @remarks
   * Password requirements:
   * - Minimum 16 characters
   * - At least 1 uppercase letter
   * - At least 1 lowercase letter
   * - At least 1 number
   * - At least 1 symbol (!@#$%^&*()_+-=[]{}|;:,.<>?)
   */
  validatePassword(password: string): boolean {
    const minLength = 16;
    const hasUppercase = /[A-Z]/.test(password);
    const hasLowercase = /[a-z]/.test(password);
    const hasNumber = /[0-9]/.test(password);
    const hasSymbol = /[!@#$%^&*()_+\-=[\]{}|;:,.<>?]/.test(password);

    if (password.length < minLength) {
      throw new WeakPasswordError(`Password must be at least ${minLength} characters long`);
    }

    if (!hasUppercase) {
      throw new WeakPasswordError('Password must contain at least one uppercase letter');
    }

    if (!hasLowercase) {
      throw new WeakPasswordError('Password must contain at least one lowercase letter');
    }

    if (!hasNumber) {
      throw new WeakPasswordError('Password must contain at least one number');
    }

    if (!hasSymbol) {
      throw new WeakPasswordError('Password must contain at least one symbol');
    }

    return true;
  }

  /**
   * Derive encryption key from password using PBKDF2
   * @param password - User password
   * @returns 256-bit encryption key
   * @remarks
   * Uses PBKDF2-SHA256 with 100,000 iterations (NIST recommendation).
   * Validates password strength before derivation.
   */
  private deriveEncryptionKey(password: string): Buffer {
    // Validate password strength
    this.validatePassword(password);

    if (!this.salt) {
      throw new Error('Salt not initialized. Call initialize() first.');
    }

    // Derive 256-bit key using PBKDF2
    return pbkdf2Sync(password, this.salt, 100000, 32, 'sha256');
  }

  /**
   * Generate master seed with BIP-39 mnemonic
   * @param strength - Entropy strength in bits (128 = 12 words, 256 = 24 words)
   * @returns Master seed with mnemonic and derived seed
   * @remarks
   * Uses bip39.generateMnemonic() for cryptographically secure mnemonic generation.
   * Derives 512-bit seed from mnemonic using bip39.mnemonicToSeed().
   *
   * @example Generate 24-word mnemonic
   * ```typescript
   * const masterSeed = await manager.generateMasterSeed(256);
   * console.log(`Mnemonic: ${masterSeed.mnemonic.split(' ').length} words`);
   * ```
   */
  async generateMasterSeed(strength: 128 | 256 = 256): Promise<MasterSeed> {
    try {
      const bip39 = await requireOptional<typeof import('bip39')>(
        'bip39',
        'BIP-39 mnemonic generation'
      );

      // Generate mnemonic with specified entropy
      const mnemonic = bip39.generateMnemonic(strength);

      // Derive 512-bit seed from mnemonic
      const seed = await bip39.mnemonicToSeed(mnemonic);

      const masterSeed: MasterSeed = {
        mnemonic,
        seed: Buffer.from(seed),
        createdAt: Date.now(),
      };

      this.logger.info(
        { strength, wordCount: mnemonic.split(' ').length },
        'Master seed generated'
      );

      return masterSeed;
    } catch (error) {
      this.logger.error({ error }, 'Failed to generate master seed');
      throw error;
    }
  }

  /**
   * Import master seed from existing mnemonic
   * @param mnemonic - BIP-39 mnemonic phrase (12 or 24 words)
   * @returns Master seed with mnemonic and derived seed
   * @throws InvalidMnemonicError if mnemonic checksum is invalid
   * @remarks
   * Validates mnemonic checksum using bip39.validateMnemonic().
   * Supports import of both 12-word (128-bit) and 24-word (256-bit) mnemonics.
   *
   * @example Import existing mnemonic
   * ```typescript
   * const mnemonic = 'abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about';
   * const masterSeed = await manager.importMasterSeed(mnemonic);
   * ```
   */
  async importMasterSeed(mnemonic: string): Promise<MasterSeed> {
    try {
      const bip39 = await requireOptional<typeof import('bip39')>(
        'bip39',
        'BIP-39 mnemonic validation'
      );

      // Validate mnemonic checksum
      if (!bip39.validateMnemonic(mnemonic)) {
        throw new InvalidMnemonicError('Invalid mnemonic: checksum validation failed');
      }

      // Derive 512-bit seed from mnemonic
      const seed = await bip39.mnemonicToSeed(mnemonic);

      const masterSeed: MasterSeed = {
        mnemonic,
        seed: Buffer.from(seed),
        createdAt: Date.now(),
      };

      this.logger.info(
        { wordCount: mnemonic.split(' ').length },
        'Master seed imported from mnemonic'
      );

      return masterSeed;
    } catch (error) {
      if (error instanceof InvalidMnemonicError) {
        throw error;
      }
      this.logger.error({ error }, 'Failed to import master seed');
      throw error;
    }
  }

  /**
   * Encrypt master seed and store to filesystem or HSM
   * @param masterSeed - Master seed to encrypt
   * @param password - Password for encryption
   * @returns Base64-encoded encrypted data
   * @remarks
   * Uses AES-256-GCM for encryption with random IV per operation.
   * Encrypts mnemonic (not seed Buffer) to reduce storage size.
   * Format: IV (16 bytes) + AuthTag (16 bytes) + Encrypted Mnemonic
   * Password must meet complexity requirements (validated via deriveEncryptionKey).
   *
   * @example Encrypt and store seed
   * ```typescript
   * const masterSeed = await manager.generateMasterSeed(256);
   * const encryptedData = await manager.encryptAndStore(masterSeed, 'StrongP@ssw0rd123456');
   * ```
   */
  async encryptAndStore(masterSeed: MasterSeed, password: string): Promise<string> {
    try {
      // Derive encryption key from password (includes password validation)
      const key = this.deriveEncryptionKey(password);

      // Generate random IV
      const iv = randomBytes(16);

      // Create AES-256-GCM cipher
      const cipher = createCipheriv('aes-256-gcm', key, iv);

      // Encrypt mnemonic (not seed) to reduce storage size
      let encrypted = cipher.update(masterSeed.mnemonic, 'utf8');
      encrypted = Buffer.concat([encrypted, cipher.final()]);

      // Get authentication tag
      const authTag = cipher.getAuthTag();

      // Concatenate: IV + AuthTag + Encrypted Mnemonic
      const encryptedData = Buffer.concat([iv, authTag, encrypted]);

      // Base64 encode for storage
      const encryptedBase64 = encryptedData.toString('base64');

      // Store encrypted seed
      if (this.keyManager) {
        // Use KeyManager for HSM/KMS storage
        await this.keyManager.storeSecret('master-seed', encryptedBase64);
        this.logger.info('Master seed encrypted and stored to HSM/KMS');
      } else {
        // Use filesystem storage
        const seedPath = path.join(this.config.storagePath!, 'master-seed.enc');
        await fs.mkdir(path.dirname(seedPath), { recursive: true });
        await fs.writeFile(seedPath, encryptedBase64, 'utf8');
        this.logger.info({ seedPath }, 'Master seed encrypted and stored to filesystem');
      }

      return encryptedBase64;
    } catch (error) {
      this.logger.error({ error }, 'Failed to encrypt and store master seed');
      throw error;
    }
  }

  /**
   * Decrypt and load master seed from storage
   * @param password - Password for decryption
   * @returns Decrypted master seed
   * @throws DecryptionError if password is incorrect or data is corrupted
   * @remarks
   * Loads encrypted seed from filesystem or HSM/KMS.
   * Validates authentication tag before decryption (prevents tampering).
   * Re-validates mnemonic checksum after decryption.
   *
   * @example Decrypt and load seed
   * ```typescript
   * const masterSeed = await manager.decryptAndLoad('StrongP@ssw0rd123456');
   * console.log(`Loaded seed with ${masterSeed.mnemonic.split(' ').length} words`);
   * ```
   */
  async decryptAndLoad(password: string): Promise<MasterSeed> {
    try {
      // Load encrypted data
      let encryptedBase64: string;

      if (this.keyManager) {
        // Load from KeyManager (HSM/KMS)
        encryptedBase64 = await this.keyManager.retrieveSecret('master-seed');
        this.logger.info('Loaded encrypted master seed from HSM/KMS');
      } else {
        // Load from filesystem
        const seedPath = path.join(this.config.storagePath!, 'master-seed.enc');
        encryptedBase64 = await fs.readFile(seedPath, 'utf8');
        this.logger.info({ seedPath }, 'Loaded encrypted master seed from filesystem');
      }

      // Base64 decode
      const encryptedData = Buffer.from(encryptedBase64, 'base64');

      // Extract IV, AuthTag, and encrypted mnemonic
      const iv = encryptedData.subarray(0, 16);
      const authTag = encryptedData.subarray(16, 32);
      const encrypted = encryptedData.subarray(32);

      // Derive encryption key from password
      const key = this.deriveEncryptionKey(password);

      // Create AES-256-GCM decipher
      const decipher = createDecipheriv('aes-256-gcm', key, iv);
      decipher.setAuthTag(authTag);

      // Decrypt
      let decrypted: Buffer;
      try {
        decrypted = decipher.update(encrypted);
        decrypted = Buffer.concat([decrypted, decipher.final()]);
      } catch (error) {
        // Catch authentication tag mismatch during decryption
        this.logger.error('Decryption failed: invalid password or corrupted data');
        throw new DecryptionError('Invalid password or corrupted data');
      }

      const mnemonic = decrypted.toString('utf8');

      // Import mnemonic to validate and return MasterSeed
      return await this.importMasterSeed(mnemonic);
    } catch (error) {
      // Re-throw DecryptionError
      if (error instanceof DecryptionError) {
        throw error;
      }

      // Catch other errors
      this.logger.error({ error }, 'Failed to decrypt and load master seed');
      throw error;
    }
  }

  /**
   * Calculate SHA-256 checksum for integrity validation
   * @param data - Data to hash
   * @returns Hex-encoded SHA-256 checksum
   */
  private calculateChecksum(data: string): string {
    return createHash('sha256').update(data).digest('hex');
  }

  /**
   * Export encrypted backup with checksum validation
   * @param masterSeed - Master seed to backup
   * @param password - Password for encryption
   * @returns Backup data with checksum
   * @remarks
   * Creates encrypted backup with metadata and SHA-256 checksum.
   * Backup format version 1.0 includes: version, timestamps, encrypted seed, checksum.
   * Optionally saves backup to ./backups/wallet-backup-{timestamp}.json.
   *
   * @example Export backup to file
   * ```typescript
   * const backup = await manager.exportBackup(masterSeed, 'StrongP@ssw0rd123456');
   * await fs.writeFile(`./backups/wallet-backup-${Date.now()}.json`, JSON.stringify(backup, null, 2));
   * ```
   */
  async exportBackup(masterSeed: MasterSeed, password: string): Promise<BackupData> {
    try {
      // Encrypt master seed
      const encryptedSeed = await this.encryptAndStore(masterSeed, password);

      // Calculate checksum
      const checksum = this.calculateChecksum(encryptedSeed);

      const backup: BackupData = {
        version: '1.0',
        createdAt: masterSeed.createdAt,
        encryptedSeed,
        backupDate: Date.now(),
        checksum,
      };

      this.logger.info({ checksum }, 'Backup exported with checksum validation');

      return backup;
    } catch (error) {
      this.logger.error({ error }, 'Failed to export backup');
      throw error;
    }
  }

  /**
   * Restore master seed from backup with integrity validation
   * @param backup - Backup data to restore
   * @param password - Password for decryption
   * @returns Restored master seed
   * @throws InvalidBackupError if checksum validation fails
   * @remarks
   * Validates backup checksum before decryption.
   * Prevents restore from tampered or corrupted backup files.
   *
   * @example Restore from backup file
   * ```typescript
   * const backupJson = await fs.readFile('./backups/wallet-backup-1234567890.json', 'utf8');
   * const backup: BackupData = JSON.parse(backupJson);
   * const masterSeed = await manager.restoreFromBackup(backup, 'StrongP@ssw0rd123456');
   * ```
   */
  async restoreFromBackup(backup: BackupData, password: string): Promise<MasterSeed> {
    try {
      // Validate checksum
      const expectedChecksum = this.calculateChecksum(backup.encryptedSeed);
      if (backup.checksum !== expectedChecksum) {
        throw new InvalidBackupError('Backup integrity check failed: checksum mismatch');
      }

      this.logger.info({ checksum: backup.checksum }, 'Backup integrity validated');

      // Temporarily store encrypted seed for decryption
      const seedPath = path.join(this.config.storagePath!, 'master-seed.enc');
      await fs.mkdir(path.dirname(seedPath), { recursive: true });
      await fs.writeFile(seedPath, backup.encryptedSeed, 'utf8');

      // Decrypt and load master seed
      const masterSeed = await this.decryptAndLoad(password);

      // Restore original creation timestamp
      masterSeed.createdAt = backup.createdAt;

      this.logger.info('Master seed restored from backup');

      return masterSeed;
    } catch (error) {
      this.logger.error({ error }, 'Failed to restore from backup');
      throw error;
    }
  }

  /**
   * Generate paper wallet with QR code
   * @param masterSeed - Master seed to generate paper wallet from
   * @returns Paper wallet with mnemonic and QR code data URL
   * @remarks
   * WARNING: Paper wallets contain unencrypted mnemonic phrase.
   * Store in physically secure location (safe, vault, etc.).
   * QR code can be scanned for easy mnemonic recovery.
   *
   * @example Generate paper wallet
   * ```typescript
   * const masterSeed = await manager.generateMasterSeed(256);
   * const paperWallet = await manager.generatePaperWallet(masterSeed);
   * console.log(`QR Code: ${paperWallet.qrCodeDataUrl}`);
   * ```
   */
  async generatePaperWallet(masterSeed: MasterSeed): Promise<PaperWallet> {
    try {
      const QRCode = await requireOptional<typeof import('qrcode')>(
        'qrcode',
        'QR code generation for paper wallets'
      );

      // Generate QR code from mnemonic
      const qrCodeDataUrl = await QRCode.toDataURL(masterSeed.mnemonic);

      const paperWallet: PaperWallet = {
        mnemonic: masterSeed.mnemonic,
        qrCodeDataUrl,
        createdAt: Date.now(),
      };

      this.logger.info('Paper wallet generated (WARNING: contains unencrypted mnemonic)');

      return paperWallet;
    } catch (error) {
      this.logger.error({ error }, 'Failed to generate paper wallet');
      throw error;
    }
  }

  /**
   * Export paper wallet to HTML file
   * @param paperWallet - Paper wallet to export
   * @param outputPath - Optional output path (default: ./backups/paper-wallet-{timestamp}.html)
   * @returns Path to exported HTML file
   * @remarks
   * Generates HTML file with QR code image and mnemonic text.
   * Includes security warnings about physical storage.
   *
   * @example Export paper wallet to HTML
   * ```typescript
   * const paperWallet = await manager.generatePaperWallet(masterSeed);
   * const filePath = await manager.exportPaperWallet(paperWallet);
   * console.log(`Paper wallet saved to: ${filePath}`);
   * ```
   */
  async exportPaperWallet(paperWallet: PaperWallet, outputPath?: string): Promise<string> {
    try {
      const fileName = outputPath ?? `paper-wallet-${Date.now()}.html`;
      const filePath = path.join('./backups', fileName);

      // Create backups directory if not exists
      await fs.mkdir('./backups', { recursive: true });

      // Generate HTML
      const html = `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>HD Wallet Paper Wallet</title>
  <style>
    body {
      font-family: Arial, sans-serif;
      max-width: 800px;
      margin: 50px auto;
      padding: 20px;
      background: #f5f5f5;
    }
    .container {
      background: white;
      padding: 40px;
      border-radius: 10px;
      box-shadow: 0 2px 10px rgba(0,0,0,0.1);
    }
    h1 {
      color: #333;
      text-align: center;
      margin-bottom: 30px;
    }
    .warning {
      background: #fff3cd;
      border: 2px solid #ffc107;
      border-radius: 5px;
      padding: 15px;
      margin-bottom: 30px;
      color: #856404;
    }
    .warning strong {
      display: block;
      font-size: 1.2em;
      margin-bottom: 10px;
    }
    .qr-code {
      text-align: center;
      margin: 30px 0;
    }
    .qr-code img {
      border: 2px solid #333;
      padding: 10px;
      background: white;
    }
    .mnemonic {
      background: #f9f9f9;
      border: 2px solid #333;
      border-radius: 5px;
      padding: 20px;
      margin: 20px 0;
      font-family: 'Courier New', monospace;
      font-size: 1.1em;
      line-height: 1.8;
      word-spacing: 5px;
      text-align: center;
    }
    .instructions {
      background: #e3f2fd;
      border-left: 4px solid #2196f3;
      padding: 15px;
      margin-top: 30px;
    }
    .instructions h3 {
      margin-top: 0;
      color: #1976d2;
    }
    .timestamp {
      text-align: center;
      color: #666;
      font-size: 0.9em;
      margin-top: 30px;
    }
  </style>
</head>
<body>
  <div class="container">
    <h1>🔐 HD Wallet Paper Wallet</h1>

    <div class="warning">
      <strong>⚠️ SECURITY WARNING</strong>
      This paper wallet contains your unencrypted master seed mnemonic phrase.
      Anyone with access to this phrase can derive all your agent wallets.
      Store this document in a physically secure location (safe, vault, etc.).
      Never share this mnemonic or take digital photos of it.
    </div>

    <div class="qr-code">
      <h2>Recovery QR Code</h2>
      <img src="${paperWallet.qrCodeDataUrl}" alt="Mnemonic QR Code" />
    </div>

    <h2 style="text-align: center;">Recovery Phrase</h2>
    <div class="mnemonic">
      ${paperWallet.mnemonic}
    </div>

    <div class="instructions">
      <h3>📝 Recovery Instructions</h3>
      <ol>
        <li>Store this paper wallet in a secure, dry location</li>
        <li>Consider making multiple copies stored in different secure locations</li>
        <li>To recover your wallet, import the 24-word mnemonic phrase using WalletSeedManager.importMasterSeed()</li>
        <li>Alternatively, scan the QR code with a compatible wallet application</li>
        <li>Test recovery process with a small amount before securing large funds</li>
      </ol>
    </div>

    <div class="timestamp">
      Created: ${new Date(paperWallet.createdAt).toISOString()}<br/>
      Generated by M2M AI Agent Wallet Infrastructure
    </div>
  </div>
</body>
</html>`;

      // Write HTML to file
      await fs.writeFile(filePath, html, 'utf8');

      this.logger.info({ filePath }, 'Paper wallet exported to HTML file');

      return filePath;
    } catch (error) {
      this.logger.error({ error }, 'Failed to export paper wallet');
      throw error;
    }
  }

  /**
   * Export backup to JSON file
   * @param backup - Backup data to export
   * @param outputPath - Optional output path (default: ./backups/wallet-backup-{timestamp}.json)
   * @returns Path to exported JSON file
   *
   * @example Export backup to file
   * ```typescript
   * const backup = await manager.exportBackup(masterSeed, 'StrongP@ssw0rd123456');
   * const filePath = await manager.exportBackupToFile(backup);
   * console.log(`Backup saved to: ${filePath}`);
   * ```
   */
  async exportBackupToFile(backup: BackupData, outputPath?: string): Promise<string> {
    try {
      const fileName = outputPath ?? `wallet-backup-${Date.now()}.json`;
      const filePath = path.join('./backups', fileName);

      // Create backups directory if not exists
      await fs.mkdir('./backups', { recursive: true });

      // Write backup as formatted JSON
      await fs.writeFile(filePath, JSON.stringify(backup, null, 2), 'utf8');

      this.logger.info({ filePath }, 'Backup exported to JSON file');

      return filePath;
    } catch (error) {
      this.logger.error({ error }, 'Failed to export backup to file');
      throw error;
    }
  }
}
