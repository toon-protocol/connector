import { Logger } from 'pino';
import { KeyManagerBackend, HSMConfig } from '../key-manager';

/**
 * HSMBackend implements KeyManagerBackend using Hardware Security Module via PKCS#11
 * Supports EVM (secp256k1) key type
 * Note: Requires pkcs11js library and PKCS#11 library (e.g., SoftHSM)
 */
export class HSMBackend implements KeyManagerBackend {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private pkcs11: any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private session: any;
  private logger: Logger;

  constructor(config: HSMConfig, logger: Logger) {
    this.logger = logger.child({ component: 'HSMBackend' });

    try {
      // Dynamically load pkcs11js (optional dependency)
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const pkcs11js = require('pkcs11js');
      this.pkcs11 = new pkcs11js.PKCS11();

      // Load PKCS#11 library
      this.pkcs11.load(config.pkcs11LibraryPath);

      // Initialize PKCS#11
      this.pkcs11.C_Initialize();

      // Open session with specified slot
      this.session = this.pkcs11.C_OpenSession(
        config.slotId,
        pkcs11js.CKF_SERIAL_SESSION | pkcs11js.CKF_RW_SESSION
      );

      // Login to HSM with PIN
      const pin = config.pin || process.env.HSM_PIN;
      if (!pin) {
        throw new Error('HSM PIN not provided');
      }

      this.pkcs11.C_Login(this.session, pkcs11js.CKU_USER, pin);

      this.logger.info(
        { slotId: config.slotId, libraryPath: config.pkcs11LibraryPath },
        'HSMBackend initialized and logged in'
      );
    } catch (error) {
      this.logger.error({ error }, 'HSMBackend initialization failed');
      const mappedError = this._mapPKCS11Error(error);
      throw new Error(`HSMBackend initialization failed: ${mappedError.message}`);
    }
  }

  /**
   * Map PKCS#11 error codes to descriptive errors
   * @param error - Original error from PKCS#11 operation
   * @returns Mapped error with descriptive message
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private _mapPKCS11Error(error: any): Error {
    // PKCS#11 error codes
    const CKR_PIN_INCORRECT = 0x000000a0;
    const CKR_KEY_HANDLE_INVALID = 0x00000060;
    const CKR_FUNCTION_FAILED = 0x00000006;

    if (typeof error === 'object' && error !== null && 'code' in error) {
      const code = error.code;
      if (code === CKR_PIN_INCORRECT) {
        return new Error('Invalid HSM PIN');
      } else if (code === CKR_KEY_HANDLE_INVALID) {
        return new Error('Key not found');
      } else if (code === CKR_FUNCTION_FAILED) {
        return new Error('HSM operation failed');
      }
    }
    return error instanceof Error ? error : new Error(String(error));
  }

  /**
   * Detects key type based on keyLabel
   * @param keyLabel - Key label in HSM
   * @returns Key type (always 'evm' for EVM-only connector)
   */
  private _detectKeyType(_keyLabel: string): 'evm' {
    // EVM-only connector - always return 'evm'
    return 'evm';
  }

  /**
   * Finds private key handle by label
   * @param keyLabel - Key label
   * @returns Private key handle
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private _findPrivateKey(keyLabel: string): any {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const pkcs11js = require('pkcs11js');

    const template = [
      { type: pkcs11js.CKA_CLASS, value: pkcs11js.CKO_PRIVATE_KEY },
      { type: pkcs11js.CKA_LABEL, value: keyLabel },
    ];

    this.pkcs11.C_FindObjectsInit(this.session, template);
    const handles = this.pkcs11.C_FindObjects(this.session, 1);
    this.pkcs11.C_FindObjectsFinal(this.session);

    if (handles.length === 0) {
      throw new Error(`Private key with label "${keyLabel}" not found in HSM`);
    }

    return handles[0];
  }

  /**
   * Finds public key handle by label
   * @param keyLabel - Key label
   * @returns Public key handle
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private _findPublicKey(keyLabel: string): any {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const pkcs11js = require('pkcs11js');

    const template = [
      { type: pkcs11js.CKA_CLASS, value: pkcs11js.CKO_PUBLIC_KEY },
      { type: pkcs11js.CKA_LABEL, value: keyLabel },
    ];

    this.pkcs11.C_FindObjectsInit(this.session, template);
    const handles = this.pkcs11.C_FindObjects(this.session, 1);
    this.pkcs11.C_FindObjectsFinal(this.session);

    if (handles.length === 0) {
      throw new Error(`Public key with label "${keyLabel}" not found in HSM`);
    }

    return handles[0];
  }

  /**
   * Gets PKCS#11 mechanism for signing based on key type
   * @param _keyType - Key type ('evm')
   * @returns PKCS#11 mechanism
   */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private _getSignMechanism(_keyType: 'evm'): any {
    // eslint-disable-next-line @typescript-eslint/no-var-requires
    const pkcs11js = require('pkcs11js');

    return { mechanism: pkcs11js.CKM_ECDSA }; // ECDSA for secp256k1
  }

  /**
   * Signs a message using HSM PKCS#11 C_Sign mechanism
   * @param message - Message to sign
   * @param keyLabel - HSM key label
   * @returns Signature buffer
   */
  async sign(message: Buffer, keyLabel: string): Promise<Buffer> {
    const keyType = this._detectKeyType(keyLabel);
    const mechanism = this._getSignMechanism(keyType);

    this.logger.debug({ keyLabel, keyType, mechanism }, 'Signing with HSM');

    try {
      const privateKeyHandle = this._findPrivateKey(keyLabel);

      // Initialize signing operation
      this.pkcs11.C_SignInit(this.session, mechanism, privateKeyHandle);

      // Sign the message
      const signature = this.pkcs11.C_Sign(this.session, message, Buffer.alloc(256));

      this.logger.info({ keyLabel, signatureLength: signature.length }, 'HSM signature generated');

      return signature;
    } catch (error) {
      this.logger.error({ keyLabel, error }, 'HSM signing failed');
      throw this._mapPKCS11Error(error);
    }
  }

  /**
   * Retrieves public key from HSM using C_GetAttributeValue
   * @param keyLabel - HSM key label
   * @returns Public key buffer
   */
  async getPublicKey(keyLabel: string): Promise<Buffer> {
    this.logger.debug({ keyLabel }, 'Retrieving public key from HSM');

    try {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const pkcs11js = require('pkcs11js');

      const publicKeyHandle = this._findPublicKey(keyLabel);

      // Get public key value using C_GetAttributeValue
      const template = [{ type: pkcs11js.CKA_VALUE }];

      const attributes = this.pkcs11.C_GetAttributeValue(this.session, publicKeyHandle, template);

      if (!attributes[0].value) {
        throw new Error('HSM returned no public key value');
      }

      const publicKey = attributes[0].value as Buffer;
      this.logger.info({ keyLabel, publicKeyLength: publicKey.length }, 'HSM public key retrieved');

      return publicKey;
    } catch (error) {
      this.logger.error({ keyLabel, error }, 'HSM public key retrieval failed');
      throw this._mapPKCS11Error(error);
    }
  }

  /**
   * Generates a new key pair in HSM for rotation using C_GenerateKeyPair
   * @param keyLabel - Current key label
   * @returns New key label
   */
  async rotateKey(keyLabel: string): Promise<string> {
    const keyType = this._detectKeyType(keyLabel);

    this.logger.info(
      { oldKeyLabel: keyLabel, keyType },
      'Generating new HSM key pair for rotation'
    );

    try {
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const pkcs11js = require('pkcs11js');

      const newKeyLabel = `${keyLabel}-rotated-${Date.now()}`;

      // Define mechanism based on key type
      let mechanism;
      let publicKeyTemplate;
      let privateKeyTemplate;

      if (keyType === 'evm') {
        // Generate secp256k1 EC key pair
        mechanism = { mechanism: pkcs11js.CKM_EC_KEY_PAIR_GEN };

        // secp256k1 OID: 1.3.132.0.10
        const secp256k1Oid = Buffer.from([0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x0a]);

        publicKeyTemplate = [
          { type: pkcs11js.CKA_LABEL, value: newKeyLabel },
          { type: pkcs11js.CKA_EC_PARAMS, value: secp256k1Oid },
          { type: pkcs11js.CKA_VERIFY, value: true },
        ];

        privateKeyTemplate = [
          { type: pkcs11js.CKA_LABEL, value: newKeyLabel },
          { type: pkcs11js.CKA_SIGN, value: true },
          { type: pkcs11js.CKA_SENSITIVE, value: true },
          { type: pkcs11js.CKA_EXTRACTABLE, value: false },
        ];
      } else {
        // Generate ed25519 key pair
        mechanism = { mechanism: pkcs11js.CKM_EC_EDWARDS_KEY_PAIR_GEN };

        // Ed25519 OID: 1.3.101.112
        const ed25519Oid = Buffer.from([0x06, 0x03, 0x2b, 0x65, 0x70]);

        publicKeyTemplate = [
          { type: pkcs11js.CKA_LABEL, value: newKeyLabel },
          { type: pkcs11js.CKA_EC_PARAMS, value: ed25519Oid },
          { type: pkcs11js.CKA_VERIFY, value: true },
        ];

        privateKeyTemplate = [
          { type: pkcs11js.CKA_LABEL, value: newKeyLabel },
          { type: pkcs11js.CKA_SIGN, value: true },
          { type: pkcs11js.CKA_SENSITIVE, value: true },
          { type: pkcs11js.CKA_EXTRACTABLE, value: false },
        ];
      }

      // Generate key pair
      const keyPair = this.pkcs11.C_GenerateKeyPair(
        this.session,
        mechanism,
        publicKeyTemplate,
        privateKeyTemplate
      );

      this.logger.info(
        {
          oldKeyLabel: keyLabel,
          newKeyLabel,
          publicKey: keyPair.publicKey,
          privateKey: keyPair.privateKey,
        },
        'HSM key rotation completed'
      );

      return newKeyLabel;
    } catch (error) {
      this.logger.error({ keyLabel, error }, 'HSM key rotation failed');
      throw this._mapPKCS11Error(error);
    }
  }

  /**
   * Cleanup: Logout and close session
   */
  destroy(): void {
    try {
      if (this.session) {
        this.pkcs11.C_Logout(this.session);
        this.pkcs11.C_CloseSession(this.session);
      }
      if (this.pkcs11) {
        this.pkcs11.C_Finalize();
      }
      this.logger.info('HSMBackend destroyed');
    } catch (error) {
      this.logger.error({ error }, 'HSMBackend cleanup failed');
    }
  }
}
