/* eslint-disable no-console */
/**
 * Documentation Completeness Audit
 *
 * NOTE: This is a filesystem compliance check, not a code test. It validates
 * that required documentation files exist and contain expected sections.
 * No production code is exercised. Consider moving to a pre-commit hook
 * or CI lint step for faster feedback.
 *
 * @see Story 12.10: Production Acceptance Testing and Go-Live
 */

import * as fs from 'fs';
import * as path from 'path';

// Filesystem checks are fast — 30 seconds is more than enough
jest.setTimeout(30000);

// Project root directory
const PROJECT_ROOT = path.resolve(__dirname, '..', '..', '..', '..');

interface DocumentRequirement {
  path: string;
  description: string;
  requiredSections?: string[];
  minSizeBytes?: number;
}

interface AuditResult {
  document: string;
  exists: boolean;
  sizeBytes: number;
  missingSections: string[];
  passed: boolean;
  issues: string[];
}

// Required documentation files
const REQUIRED_DOCS: DocumentRequirement[] = [
  // Architecture Documentation
  {
    path: 'docs/architecture/tech-stack.md',
    description: 'Technology Stack Overview',
    requiredSections: ['Overview', 'Dependencies'],
    minSizeBytes: 500,
  },
  {
    path: 'docs/architecture/source-tree.md',
    description: 'Source Code Structure',
    requiredSections: ['packages', 'Structure'],
    minSizeBytes: 500,
  },
  {
    path: 'docs/architecture/coding-standards.md',
    description: 'Coding Standards',
    requiredSections: ['TypeScript', 'Testing'],
    minSizeBytes: 500,
  },

  // Operator Documentation
  {
    path: 'docs/operators/load-testing-guide.md',
    description: 'Load Testing Guide',
    requiredSections: ['Prerequisites', 'Configuration', 'Troubleshooting'],
    minSizeBytes: 1000,
  },

  // README
  {
    path: 'README.md',
    description: 'Project README',
    requiredSections: ['Getting Started', 'Installation'],
    minSizeBytes: 500,
  },
];

// Required connector-specific documentation
const CONNECTOR_DOCS: DocumentRequirement[] = [
  {
    path: 'packages/connector/README.md',
    description: 'Connector Package README',
    minSizeBytes: 100,
  },
];

/**
 * Check if a file exists and return its size
 */
function checkFileExists(filePath: string): { exists: boolean; sizeBytes: number } {
  const fullPath = path.join(PROJECT_ROOT, filePath);
  try {
    const stats = fs.statSync(fullPath);
    return { exists: true, sizeBytes: stats.size };
  } catch {
    return { exists: false, sizeBytes: 0 };
  }
}

/**
 * Read file content
 */
function readFileContent(filePath: string): string {
  const fullPath = path.join(PROJECT_ROOT, filePath);
  try {
    return fs.readFileSync(fullPath, 'utf-8');
  } catch {
    return '';
  }
}

/**
 * Check if required sections exist in markdown content
 */
function checkSections(content: string, requiredSections: string[]): string[] {
  const missing: string[] = [];

  for (const section of requiredSections) {
    // Check for section as header or keyword
    const headerPattern = new RegExp(`#.*${section}`, 'i');
    const keywordPattern = new RegExp(section, 'i');

    if (!headerPattern.test(content) && !keywordPattern.test(content)) {
      missing.push(section);
    }
  }

  return missing;
}

/**
 * Audit a single document
 */
function auditDocument(requirement: DocumentRequirement): AuditResult {
  const issues: string[] = [];
  const { exists, sizeBytes } = checkFileExists(requirement.path);

  if (!exists) {
    return {
      document: requirement.path,
      exists: false,
      sizeBytes: 0,
      missingSections: requirement.requiredSections || [],
      passed: false,
      issues: ['File does not exist'],
    };
  }

  // Check minimum size
  if (requirement.minSizeBytes && sizeBytes < requirement.minSizeBytes) {
    issues.push(`File too small: ${sizeBytes} bytes (min: ${requirement.minSizeBytes} bytes)`);
  }

  // Check required sections
  let missingSections: string[] = [];
  if (requirement.requiredSections) {
    const content = readFileContent(requirement.path);
    missingSections = checkSections(content, requirement.requiredSections);

    if (missingSections.length > 0) {
      issues.push(`Missing sections: ${missingSections.join(', ')}`);
    }
  }

  // Check for placeholder content
  const content = readFileContent(requirement.path);
  if (content.includes('TODO') || content.includes('PLACEHOLDER')) {
    issues.push('Contains TODO or PLACEHOLDER text');
  }

  return {
    document: requirement.path,
    exists,
    sizeBytes,
    missingSections,
    passed: issues.length === 0,
    issues,
  };
}

describe('Documentation Completeness Audit', () => {
  const auditResults: AuditResult[] = [];

  afterAll(() => {
    // Print audit summary
    console.log('\n=== Documentation Audit Summary ===\n');

    const passed = auditResults.filter((r) => r.passed).length;
    const failed = auditResults.filter((r) => !r.passed).length;
    const total = auditResults.length;

    console.log(`Total Documents: ${total}`);
    console.log(`Passed: ${passed}`);
    console.log(`Failed: ${failed}`);
    console.log('');

    for (const result of auditResults) {
      const status = result.passed ? '✅' : '❌';
      console.log(`${status} ${result.document}`);
      if (!result.passed) {
        result.issues.forEach((issue) => console.log(`   - ${issue}`));
      }
    }
    console.log('');
  });

  describe('Architecture Documentation', () => {
    it('should have technology stack documentation', () => {
      const requirement = REQUIRED_DOCS.find((d) => d.path.includes('tech-stack'))!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
      expect(result.sizeBytes).toBeGreaterThan(requirement.minSizeBytes ?? 0);
    });

    it('should have source tree documentation', () => {
      const requirement = REQUIRED_DOCS.find((d) => d.path.includes('source-tree'))!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
      expect(result.sizeBytes).toBeGreaterThan(requirement.minSizeBytes ?? 0);
    });

    it('should have coding standards documentation', () => {
      const requirement = REQUIRED_DOCS.find((d) => d.path.includes('coding-standards'))!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
      expect(result.sizeBytes).toBeGreaterThan(requirement.minSizeBytes ?? 0);
    });
  });

  describe('Operational Documentation', () => {
    it('should have load testing guide', () => {
      const requirement = REQUIRED_DOCS.find((d) => d.path.includes('load-testing-guide'))!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
      expect(result.missingSections.length).toBe(0);
    });

    it('should have operational runbooks directory', () => {
      const operatorsPath = path.join(PROJECT_ROOT, 'docs', 'operators');
      const exists = fs.existsSync(operatorsPath);

      auditResults.push({
        document: 'docs/operators/',
        exists,
        sizeBytes: 0,
        missingSections: [],
        passed: exists,
        issues: exists ? [] : ['Operators directory does not exist'],
      });

      expect(exists).toBe(true);
    });
  });

  describe('Project Documentation', () => {
    it('should have main README', () => {
      const requirement = REQUIRED_DOCS.find((d) => d.path === 'README.md')!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
    });

    it('should have connector package README', () => {
      const requirement = CONNECTOR_DOCS.find((d) => d.path.includes('connector/README'))!;
      const result = auditDocument(requirement);
      auditResults.push(result);

      expect(result.exists).toBe(true);
    });
  });

  describe('Documentation Quality', () => {
    it('should not have excessive placeholder content', () => {
      let placeholderCount = 0;

      for (const doc of [...REQUIRED_DOCS, ...CONNECTOR_DOCS]) {
        const content = readFileContent(doc.path);
        const todoMatches = (content.match(/TODO/gi) || []).length;
        const placeholderMatches = (content.match(/PLACEHOLDER/gi) || []).length;
        placeholderCount += todoMatches + placeholderMatches;
      }

      // Allow some TODOs but not excessive
      expect(placeholderCount).toBeLessThan(50);
    });

    it('should have consistent markdown formatting', () => {
      const markdownDocs = [...REQUIRED_DOCS, ...CONNECTOR_DOCS].filter((d) =>
        d.path.endsWith('.md')
      );

      for (const doc of markdownDocs) {
        const content = readFileContent(doc.path);
        if (content.length === 0) continue;

        // Check for basic markdown structure
        const hasHeaders = /#\s+\w+/.test(content);
        expect(hasHeaders).toBe(true);
      }
    });
  });

  describe('Story Documentation', () => {
    it('should have story documents directory', () => {
      const storiesPath = path.join(PROJECT_ROOT, 'docs', 'stories');
      const exists = fs.existsSync(storiesPath);

      auditResults.push({
        document: 'docs/stories/',
        exists,
        sizeBytes: 0,
        missingSections: [],
        passed: exists,
        issues: exists ? [] : ['Stories directory does not exist'],
      });

      expect(exists).toBe(true);
    });

    it('should have Epic 12 story documents', () => {
      const storiesPath = path.join(PROJECT_ROOT, 'docs', 'stories');

      if (!fs.existsSync(storiesPath)) {
        return; // Skip if directory doesn't exist
      }

      const files = fs.readdirSync(storiesPath);
      const epic12Stories = files.filter((f) => f.startsWith('12.'));

      auditResults.push({
        document: 'docs/stories/12.*.story.md',
        exists: epic12Stories.length > 0,
        sizeBytes: 0,
        missingSections: [],
        passed: epic12Stories.length > 0,
        issues: epic12Stories.length === 0 ? ['No Epic 12 story documents found'] : [],
      });

      expect(epic12Stories.length).toBeGreaterThan(0);
    });
  });

  describe('API Documentation', () => {
    it('should have shared package with type exports', () => {
      const sharedPath = path.join(PROJECT_ROOT, 'packages', 'shared');
      const exists = fs.existsSync(sharedPath);

      auditResults.push({
        document: 'packages/shared/',
        exists,
        sizeBytes: 0,
        missingSections: [],
        passed: exists,
        issues: exists ? [] : ['Shared package does not exist'],
      });

      expect(exists).toBe(true);
    });

    it('should have TypeScript definitions for API types', () => {
      const typesPath = path.join(PROJECT_ROOT, 'packages', 'shared', 'src');

      if (!fs.existsSync(typesPath)) {
        return; // Skip if directory doesn't exist
      }

      const files = fs.readdirSync(typesPath);
      const hasTypeFiles = files.some((f) => f.endsWith('.ts') && !f.endsWith('.test.ts'));

      auditResults.push({
        document: 'packages/shared/src/*.ts',
        exists: hasTypeFiles,
        sizeBytes: 0,
        missingSections: [],
        passed: hasTypeFiles,
        issues: hasTypeFiles ? [] : ['No TypeScript definition files found'],
      });

      expect(hasTypeFiles).toBe(true);
    });
  });

  describe('Test Documentation', () => {
    it('should have acceptance test directory', () => {
      const acceptancePath = path.join(PROJECT_ROOT, 'packages', 'connector', 'test', 'acceptance');
      const exists = fs.existsSync(acceptancePath);

      auditResults.push({
        document: 'packages/connector/test/acceptance/',
        exists,
        sizeBytes: 0,
        missingSections: [],
        passed: exists,
        issues: exists ? [] : ['Acceptance test directory does not exist'],
      });

      expect(exists).toBe(true);
    });

    it('should have comprehensive test coverage structure', () => {
      const testPath = path.join(PROJECT_ROOT, 'packages', 'connector', 'test');

      if (!fs.existsSync(testPath)) {
        return;
      }

      const testDirs = fs.readdirSync(testPath).filter((f) => {
        const fullPath = path.join(testPath, f);
        return fs.statSync(fullPath).isDirectory();
      });

      // Should have multiple test categories
      expect(testDirs.length).toBeGreaterThanOrEqual(2);

      auditResults.push({
        document: 'packages/connector/test/*/',
        exists: true,
        sizeBytes: 0,
        missingSections: [],
        passed: testDirs.length >= 2,
        issues: testDirs.length < 2 ? ['Insufficient test directory structure'] : [],
      });
    });
  });
});
