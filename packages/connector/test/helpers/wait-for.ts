/**
 * Wait-for Utility with Exponential Backoff
 *
 * Replaces fixed setTimeout polling loops with exponential backoff pattern.
 * Use this utility instead of:
 *
 * ```typescript
 * while (Date.now() - startTime < timeoutMs) {
 *   await new Promise(resolve => setTimeout(resolve, 2000)); // FIXED 2s
 * }
 * ```
 *
 * @example
 * ```typescript
 * import { waitFor } from '../helpers/wait-for';
 *
 * // Wait for service to be healthy
 * await waitFor(
 *   async () => {
 *     const response = await fetch(url);
 *     return response.ok;
 *   },
 *   { timeout: 30000, interval: 100, backoff: 1.5 }
 * );
 * ```
 */

export interface WaitForOptions {
  /**
   * Maximum time to wait in milliseconds
   * @default 30000 (30 seconds)
   */
  timeout?: number;

  /**
   * Initial delay between checks in milliseconds
   * @default 100
   */
  interval?: number;

  /**
   * Backoff multiplier for exponential delay increase
   * @default 1.5
   */
  backoff?: number;
}

/**
 * Wait for a condition to become true with exponential backoff
 *
 * @param condition - Function that returns true when the condition is met
 * @param options - Configuration options for timeout, interval, and backoff
 * @throws {Error} If timeout is reached before condition becomes true
 */
export async function waitFor(
  condition: () => Promise<boolean> | boolean,
  options?: WaitForOptions
): Promise<void> {
  const { timeout = 30000, interval = 100, backoff = 1.5 } = options ?? {};

  const start = Date.now();
  let delay = interval;

  while (Date.now() - start < timeout) {
    try {
      const result = await condition();
      if (result) {
        return;
      }
    } catch (error) {
      // Condition threw an error, keep retrying
    }

    // Wait with exponential backoff (capped at 5 seconds)
    await new Promise((resolve) => setTimeout(resolve, delay));
    delay = Math.min(delay * backoff, 5000);
  }

  throw new Error(`waitFor timed out after ${timeout}ms`);
}
