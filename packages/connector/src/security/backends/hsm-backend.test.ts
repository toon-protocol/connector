/* eslint-disable @typescript-eslint/no-explicit-any */
import { HSMBackend } from './hsm-backend';
import type { HSMConfig } from '../key-manager';
import pino from 'pino';

/**
 * Unit tests for HSMBackend
 *
 * Note: These tests use mocks for pkcs11js to avoid requiring real HSM hardware
 * Story: 12.2 Task 3 - HSM Backend Testing
 */

// Check if pkcs11js is available
let PKCS11Available = false;
let pkcs11js: any;

try {
  pkcs11js = require('pkcs11js');
  PKCS11Available = true;
} catch (error) {
  // pkcs11js not installed - tests will be skipped
}

const describeIf = PKCS11Available ? describe : describe.skip;

describeIf('HSMBackend', () => {
  let logger: pino.Logger;
  let config: HSMConfig;
  let backend: HSMBackend;
  let mockPKCS11Instance: any;

  beforeEach(() => {
    jest.clearAllMocks();

    logger = pino({ level: 'silent' });

    config = {
      pkcs11LibraryPath: '/usr/lib/softhsm/libsofthsm2.so',
      slotId: 0,
      pin: 'test-pin',
      evmKeyLabel: 'evm-key',
    };

    // Create mock PKCS#11 instance
    mockPKCS11Instance = {
      load: jest.fn(),
      C_Initialize: jest.fn(),
      C_OpenSession: jest.fn().mockReturnValue(1),
      C_Login: jest.fn(),
      C_FindObjectsInit: jest.fn(),
      C_FindObjects: jest.fn().mockReturnValue([100]),
      C_FindObjectsFinal: jest.fn(),
      C_SignInit: jest.fn(),
      C_Sign: jest.fn(),
      C_GetAttributeValue: jest.fn(),
      C_GenerateKeyPair: jest.fn(),
      C_Logout: jest.fn(),
      C_CloseSession: jest.fn(),
      C_Finalize: jest.fn(),
    };

    // Mock the PKCS11 constructor
    pkcs11js.PKCS11 = jest.fn(() => mockPKCS11Instance);
  });

  afterEach(() => {
    if (backend) {
      backend.destroy();
    }
  });

  describe('Initialization', () => {
    it('should initialize PKCS#11 library successfully', () => {
      backend = new HSMBackend(config, logger);

      expect(mockPKCS11Instance.load).toHaveBeenCalledWith(config.pkcs11LibraryPath);
      expect(mockPKCS11Instance.C_Initialize).toHaveBeenCalled();
      expect(mockPKCS11Instance.C_OpenSession).toHaveBeenCalledWith(
        config.slotId,
        pkcs11js.CKF_SERIAL_SESSION | pkcs11js.CKF_RW_SESSION
      );
      expect(mockPKCS11Instance.C_Login).toHaveBeenCalledWith(
        1, // session handle
        pkcs11js.CKU_USER,
        config.pin
      );
      expect(logger.info).toHaveBeenCalledWith(
        expect.objectContaining({
          slotId: config.slotId,
          libraryPath: config.pkcs11LibraryPath,
        }),
        'HSMBackend initialized and logged in'
      );
    });

    it('should throw error if PKCS#11 library not found', () => {
      mockPKCS11Instance.load.mockImplementationOnce(() => {
        throw new Error('Library not found');
      });

      expect(() => new HSMBackend(config, logger)).toThrow(/initialization failed/);
      expect(logger.error).toHaveBeenCalled();
    });

    it('should throw error if HSM PIN not provided', () => {
      const configNoPIN = { ...config, pin: '' };
      delete process.env.HSM_PIN;

      expect(() => new HSMBackend(configNoPIN, logger)).toThrow(/HSM PIN not provided/);
    });

    it('should use HSM_PIN from environment if not in config', () => {
      process.env.HSM_PIN = 'env-pin';
      const configNoPIN = { ...config, pin: '' };

      backend = new HSMBackend(configNoPIN, logger);

      expect(mockPKCS11Instance.C_Login).toHaveBeenCalledWith(1, pkcs11js.CKU_USER, 'env-pin');

      delete process.env.HSM_PIN;
    });
  });

  describe('sign()', () => {
    beforeEach(() => {
      backend = new HSMBackend(config, logger);
      mockPKCS11Instance.C_Sign.mockReturnValue(Buffer.from('signature-bytes'));
    });

    it('should sign EVM message using CKM_ECDSA mechanism', async () => {
      const message = Buffer.from('test-message');
      const signature = await backend.sign(message, config.evmKeyLabel);

      expect(mockPKCS11Instance.C_FindObjectsInit).toHaveBeenCalledWith(1, [
        { type: pkcs11js.CKA_CLASS, value: pkcs11js.CKO_PRIVATE_KEY },
        { type: pkcs11js.CKA_LABEL, value: config.evmKeyLabel },
      ]);
      expect(mockPKCS11Instance.C_FindObjects).toHaveBeenCalledWith(1, 1);
      expect(mockPKCS11Instance.C_FindObjectsFinal).toHaveBeenCalledWith(1);

      expect(mockPKCS11Instance.C_SignInit).toHaveBeenCalledWith(
        1,
        { mechanism: pkcs11js.CKM_ECDSA },
        100 // private key handle
      );
      expect(mockPKCS11Instance.C_Sign).toHaveBeenCalledWith(1, message, expect.any(Buffer));
      expect(signature).toEqual(Buffer.from('signature-bytes'));
      expect(logger.info).toHaveBeenCalledWith(
        expect.objectContaining({ keyLabel: config.evmKeyLabel }),
        'HSM signature generated'
      );
    });

    it('should detect key type from keyLabel containing "evm"', async () => {
      const message = Buffer.from('test');
      await backend.sign(message, 'my-evm-signing-key');

      expect(mockPKCS11Instance.C_SignInit).toHaveBeenCalledWith(
        1,
        { mechanism: pkcs11js.CKM_ECDSA },
        100
      );
    });

    it('should throw error if private key not found', async () => {
      mockPKCS11Instance.C_FindObjects.mockReturnValueOnce([]); // No keys found

      const message = Buffer.from('test');

      await expect(backend.sign(message, 'nonexistent-key')).rejects.toThrow(
        /Private key with label "nonexistent-key" not found/
      );
      expect(logger.error).toHaveBeenCalledWith(
        expect.objectContaining({ keyLabel: 'nonexistent-key' }),
        'HSM signing failed'
      );
    });

    it('should throw error if C_Sign fails', async () => {
      mockPKCS11Instance.C_Sign.mockImplementationOnce(() => {
        throw new Error('Signing failed');
      });

      const message = Buffer.from('test');

      await expect(backend.sign(message, config.evmKeyLabel)).rejects.toThrow('Signing failed');
      expect(logger.error).toHaveBeenCalled();
    });
  });

  describe('getPublicKey()', () => {
    beforeEach(() => {
      backend = new HSMBackend(config, logger);
      mockPKCS11Instance.C_FindObjects.mockReturnValue([200]); // Public key handle
      mockPKCS11Instance.C_GetAttributeValue.mockReturnValue([
        { value: Buffer.from('public-key-bytes') },
      ]);
    });

    it('should retrieve public key using C_GetAttributeValue', async () => {
      const publicKey = await backend.getPublicKey(config.evmKeyLabel);

      expect(mockPKCS11Instance.C_FindObjectsInit).toHaveBeenCalledWith(1, [
        { type: pkcs11js.CKA_CLASS, value: pkcs11js.CKO_PUBLIC_KEY },
        { type: pkcs11js.CKA_LABEL, value: config.evmKeyLabel },
      ]);
      expect(mockPKCS11Instance.C_FindObjects).toHaveBeenCalledWith(1, 1);
      expect(mockPKCS11Instance.C_GetAttributeValue).toHaveBeenCalledWith(1, 200, [
        { type: pkcs11js.CKA_VALUE },
      ]);
      expect(publicKey).toEqual(Buffer.from('public-key-bytes'));
      expect(logger.info).toHaveBeenCalledWith(
        expect.objectContaining({ keyLabel: config.evmKeyLabel }),
        'HSM public key retrieved'
      );
    });

    it('should throw error if public key not found', async () => {
      mockPKCS11Instance.C_FindObjects.mockReturnValueOnce([]);

      await expect(backend.getPublicKey('nonexistent-key')).rejects.toThrow(
        /Public key with label "nonexistent-key" not found/
      );
      expect(logger.error).toHaveBeenCalled();
    });

    it('should throw error if public key value is empty', async () => {
      mockPKCS11Instance.C_GetAttributeValue.mockReturnValueOnce([{ value: null }]);

      await expect(backend.getPublicKey(config.evmKeyLabel)).rejects.toThrow(
        /HSM returned no public key value/
      );
    });
  });

  describe('rotateKey()', () => {
    beforeEach(() => {
      backend = new HSMBackend(config, logger);
      mockPKCS11Instance.C_GenerateKeyPair.mockReturnValue({
        publicKey: 300,
        privateKey: 400,
      });
    });

    it('should generate new secp256k1 key pair for EVM keys', async () => {
      const newKeyLabel = await backend.rotateKey(config.evmKeyLabel);

      expect(newKeyLabel).toMatch(/evm-key-rotated-\d+/);

      // Verify mechanism and templates
      const generateCall = mockPKCS11Instance.C_GenerateKeyPair.mock.calls[0];
      expect(generateCall[0]).toBe(1); // session
      expect(generateCall[1]).toEqual({ mechanism: pkcs11js.CKM_EC_KEY_PAIR_GEN });

      // Verify secp256k1 OID in public key template
      const publicKeyTemplate = generateCall[2];
      const ecParamsAttr = publicKeyTemplate.find(
        (attr: any) => attr.type === pkcs11js.CKA_EC_PARAMS
      );
      expect(ecParamsAttr).toBeDefined();
      expect(ecParamsAttr.value).toEqual(Buffer.from([0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x0a]));

      // Verify private key template
      const privateKeyTemplate = generateCall[3];
      const signAttr = privateKeyTemplate.find((attr: any) => attr.type === pkcs11js.CKA_SIGN);
      expect(signAttr.value).toBe(true);

      expect(logger.info).toHaveBeenCalledWith(
        expect.objectContaining({ oldKeyLabel: config.evmKeyLabel, newKeyLabel }),
        'HSM key rotation completed'
      );
    });

    it('should mark new private key as non-extractable', async () => {
      await backend.rotateKey(config.evmKeyLabel);

      const generateCall = mockPKCS11Instance.C_GenerateKeyPair.mock.calls[0];
      const privateKeyTemplate = generateCall[3];

      const extractableAttr = privateKeyTemplate.find(
        (attr: any) => attr.type === pkcs11js.CKA_EXTRACTABLE
      );
      expect(extractableAttr.value).toBe(false);

      const sensitiveAttr = privateKeyTemplate.find(
        (attr: any) => attr.type === pkcs11js.CKA_SENSITIVE
      );
      expect(sensitiveAttr.value).toBe(true);
    });

    it('should throw error if key generation fails', async () => {
      mockPKCS11Instance.C_GenerateKeyPair.mockImplementationOnce(() => {
        throw new Error('Key generation failed');
      });

      await expect(backend.rotateKey(config.evmKeyLabel)).rejects.toThrow('Key generation failed');
      expect(logger.error).toHaveBeenCalledWith(
        expect.objectContaining({ keyLabel: config.evmKeyLabel }),
        'HSM key rotation failed'
      );
    });
  });

  describe('destroy()', () => {
    it('should logout, close session, and finalize on cleanup', () => {
      backend = new HSMBackend(config, logger);

      backend.destroy();

      expect(mockPKCS11Instance.C_Logout).toHaveBeenCalledWith(1);
      expect(mockPKCS11Instance.C_CloseSession).toHaveBeenCalledWith(1);
      expect(mockPKCS11Instance.C_Finalize).toHaveBeenCalled();
      expect(logger.info).toHaveBeenCalledWith('HSMBackend destroyed');
    });

    it('should handle cleanup errors gracefully', () => {
      backend = new HSMBackend(config, logger);

      mockPKCS11Instance.C_Logout.mockImplementationOnce(() => {
        throw new Error('Logout failed');
      });

      backend.destroy();

      expect(logger.error).toHaveBeenCalledWith(
        expect.objectContaining({ error: expect.any(Error) }),
        'HSMBackend cleanup failed'
      );
    });
  });

  describe('Error Handling', () => {
    beforeEach(() => {
      backend = new HSMBackend(config, logger);
    });

    it('should handle PIN incorrect error', async () => {
      mockPKCS11Instance.C_Login.mockImplementationOnce(() => {
        const error: any = new Error('PIN incorrect');
        error.code = 0x000000a0; // CKR_PIN_INCORRECT
        throw error;
      });

      expect(() => new HSMBackend(config, logger)).toThrow(/initialization failed/);
    });

    it('should handle key handle invalid error', async () => {
      mockPKCS11Instance.C_SignInit.mockImplementationOnce(() => {
        const error: any = new Error('Invalid key handle');
        error.code = 0x00000060; // CKR_KEY_HANDLE_INVALID
        throw error;
      });

      await expect(backend.sign(Buffer.from('test'), config.evmKeyLabel)).rejects.toThrow();
    });
  });
});
