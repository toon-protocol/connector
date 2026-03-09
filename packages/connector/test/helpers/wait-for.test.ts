import { waitFor } from './wait-for';

describe('waitFor', () => {
  jest.setTimeout(10000);

  it('should resolve when condition becomes true immediately', async () => {
    const condition = jest.fn().mockResolvedValue(true);

    await waitFor(condition);

    expect(condition).toHaveBeenCalledTimes(1);
  });

  it('should poll until condition becomes true', async () => {
    let callCount = 0;
    const condition = jest.fn().mockImplementation(() => {
      callCount++;
      return Promise.resolve(callCount >= 3);
    });

    await waitFor(condition, { interval: 10, backoff: 1 });

    expect(condition).toHaveBeenCalledTimes(3);
  });

  it('should throw error if timeout is reached', async () => {
    const condition = jest.fn().mockResolvedValue(false);

    await expect(waitFor(condition, { timeout: 100, interval: 20, backoff: 1 })).rejects.toThrow(
      'waitFor timed out after 100ms'
    );
  });

  it('should handle synchronous conditions', async () => {
    let callCount = 0;
    const condition = jest.fn().mockImplementation(() => {
      callCount++;
      return callCount >= 2;
    });

    await waitFor(condition, { interval: 10, backoff: 1 });

    expect(condition).toHaveBeenCalledTimes(2);
  });

  it('should continue retrying if condition throws error', async () => {
    let callCount = 0;
    const condition = jest.fn().mockImplementation(() => {
      callCount++;
      if (callCount < 3) {
        throw new Error('Not ready');
      }
      return true;
    });

    await waitFor(condition, { interval: 10, backoff: 1 });

    expect(condition).toHaveBeenCalledTimes(3);
  });

  it('should use exponential backoff', async () => {
    const startTime = Date.now();
    let callCount = 0;
    const delays: number[] = [];
    let lastCallTime = startTime;

    const condition = jest.fn().mockImplementation(() => {
      const now = Date.now();
      if (callCount > 0) {
        delays.push(now - lastCallTime);
      }
      lastCallTime = now;
      callCount++;
      return callCount >= 4;
    });

    await waitFor(condition, { interval: 50, backoff: 2, timeout: 5000 });

    expect(condition).toHaveBeenCalledTimes(4);

    // Verify delays are increasing (with some tolerance for timing variance)
    expect(delays[0]).toBeGreaterThanOrEqual(40); // ~50ms
    expect(delays[1]).toBeGreaterThanOrEqual(90); // ~100ms (50 * 2)
    expect(delays[2]).toBeGreaterThanOrEqual(180); // ~200ms (100 * 2)
  });

  it('should cap delay at 5 seconds', async () => {
    let callCount = 0;
    const delays: number[] = [];
    let lastCallTime = Date.now();

    const condition = jest.fn().mockImplementation(() => {
      const now = Date.now();
      if (callCount > 0) {
        delays.push(now - lastCallTime);
      }
      lastCallTime = now;
      callCount++;
      return callCount >= 5;
    });

    await waitFor(condition, { interval: 3000, backoff: 10, timeout: 30000 });

    // Even with high backoff (10x), delay should be capped at 5000ms
    const maxDelay = Math.max(...delays);
    expect(maxDelay).toBeLessThanOrEqual(5500); // 5000ms + tolerance
  }, 35000); // 35 second timeout for this test
});
