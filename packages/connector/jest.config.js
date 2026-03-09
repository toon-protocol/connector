/** @type {import('jest').Config} */
module.exports = {
  displayName: 'connector',
  preset: 'ts-jest',
  testEnvironment: 'node',
  roots: ['<rootDir>/src', '<rootDir>/test'],
  testMatch: ['**/*.test.ts'],
  // Ignore cloud KMS backend tests - they require optional provider-specific packages
  // Ignore integration tests with missing type dependencies (future features)
  // Ignore acceptance tests (run separately)
  testPathIgnorePatterns: [
    '/node_modules/',
    'aws-kms-backend\.test\.ts$',
    'azure-kv-backend\.test\.ts$',
    'gcp-kms-backend\.test\.ts$',
    'wallet-disaster-recovery\.test\.ts$',
    'agent-wallet-integration\.doc\.test\.ts$',
    'tigerbeetle-5peer-deployment\.test\.ts$',
    'test/acceptance/', // Acceptance tests (run separately)
    'test/unit/performance/', // Unit performance tests (timing-sensitive)
    'evm-payment-channel\.test\.ts$', // Requires live Anvil + Docker infrastructure
  ],
  testTimeout: 30000, // 30 second default timeout for integration tests
  collectCoverageFrom: [
    'src/**/*.ts',
    '!src/**/*.d.ts',
    '!src/**/*.test.ts',
    '!src/**/__mocks__/**',
    '!src/index.ts', // Exclude index.ts (re-exports only)
  ],
  // Coverage thresholds increased after test suite cleanup (Story 30.6)
  // Fake tests and Docker orchestration tests removed, legitimate tests remain
  coverageThreshold: {
    global: {
      branches: 60, // Increased from 45% after cleanup
      functions: 75, // Increased from 70% after cleanup
      lines: 70, // Increased from 65% after cleanup
      statements: 70, // Increased from 65% after cleanup
    },
  },
  moduleFileExtensions: ['ts', 'js', 'json'],
  moduleNameMapper: {
    '^@crosstown/shared$': '<rootDir>/../shared/src/index.ts',
  },
  transform: {
    '^.+\\.ts$': [
      'ts-jest',
      {
        tsconfig: '<rootDir>/tsconfig.json',
      },
    ],
    '^.+\\.m?js$': 'babel-jest',
  },
  // Allow transformation of ESM-only packages
  transformIgnorePatterns: ['node_modules/(?!(@toon-format|@libsql)/)'],
};
