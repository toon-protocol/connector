#!/usr/bin/env node

/**
 * Send Packet CLI Tool
 * Command-line utility for injecting test ILP packets into connector network
 */

import { Command } from 'commander';
import pino from 'pino';
import { PacketType } from '@toon-protocol/shared';
import { createTestPreparePacket } from './packet-factory';
import { BTPSender } from './btp-sender';

/**
 * Main CLI program
 */
const program = new Command();

program
  .name('send-packet')
  .description('CLI tool to inject test ILP packets into connector network')
  .version('0.1.0');

// Required options
program
  .requiredOption(
    '-c, --connector-url <url>',
    'WebSocket URL of connector to send packet to (e.g., ws://localhost:3000)'
  )
  .requiredOption(
    '-d, --destination <address>',
    'ILP destination address (e.g., g.connectora.dest)'
  )
  .requiredOption('-a, --amount <value>', 'Payment amount in smallest unit (e.g., 1000)', (val) =>
    BigInt(val)
  );

// Optional options
program
  .option('--auth-token <token>', 'BTP authentication token', 'test-token')
  .option('--expiry <seconds>', 'Packet expiry time in seconds from now', '30')
  .option('--data <payload>', 'Optional data payload (UTF-8 string)')
  .option('--log-level <level>', 'Log level (debug, info, warn, error)', 'info')
  .option('--batch <count>', 'Send N packets in parallel batch mode', '1')
  .option('--sequence <count>', 'Send N packets sequentially', '1')
  .option('--delay <ms>', 'Delay in milliseconds between sequential packets', '0');

// Add help examples
program.addHelpText(
  'after',
  `
Examples:
  # Send basic test packet
  $ send-packet -c ws://localhost:3000 -d g.connectora.dest -a 1000

  # Send packet with custom expiry and data
  $ send-packet -c ws://localhost:3000 -d g.connectora.dest -a 5000 --expiry 60 --data "Hello ILP"

  # Send 10 packets in parallel (batch mode)
  $ send-packet -c ws://localhost:3000 -d g.connectora.dest -a 1000 --batch 10

  # Send 5 packets sequentially with 1-second delay
  $ send-packet -c ws://localhost:3000 -d g.connectora.dest -a 1000 --sequence 5 --delay 1000

  # Send packet with debug logging
  $ send-packet -c ws://localhost:3000 -d g.connectora.dest -a 1000 --log-level debug
`
);

// Action handler
program.action(async (options) => {
  // Create Pino logger with pino-pretty for CLI output
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
    logger.info({ options }, 'Starting send-packet CLI');

    // Parse options
    const dataBuffer = options.data ? Buffer.from(options.data, 'utf8') : undefined;
    const expirySeconds = parseInt(options.expiry, 10);
    const batchCount = parseInt(options.batch, 10);
    const sequenceCount = parseInt(options.sequence, 10);
    const delayMs = parseInt(options.delay, 10);

    // Create BTPSender instance
    const sender = new BTPSender(options.connectorUrl, options.authToken, logger);

    // Connect to connector
    await sender.connect();

    // Determine mode: batch, sequence, or single
    if (batchCount > 1) {
      // Batch mode: send N packets in parallel
      logger.info({ batchCount }, `Sending batch of ${batchCount} packets...`);

      // Create N packets
      const packets = [];
      for (let i = 0; i < batchCount; i++) {
        const { packet } = createTestPreparePacket(
          options.destination,
          options.amount,
          expirySeconds,
          dataBuffer
        );
        packets.push(packet);
      }

      // Send all packets concurrently
      const results = await Promise.allSettled(packets.map((packet) => sender.sendPacket(packet)));

      // Count fulfilled vs rejected
      let fulfilledCount = 0;
      let rejectedCount = 0;

      for (const result of results) {
        if (result.status === 'fulfilled') {
          if (result.value.type === PacketType.FULFILL) {
            fulfilledCount++;
          } else {
            rejectedCount++;
          }
        } else {
          rejectedCount++;
        }
      }

      logger.info(
        { fulfilledCount, rejectedCount },
        `Batch complete: ${fulfilledCount} fulfilled, ${rejectedCount} rejected`
      );

      await sender.disconnect();
      process.exit(rejectedCount > 0 ? 1 : 0);
    } else if (sequenceCount > 1) {
      // Sequence mode: send N packets sequentially with optional delay
      logger.info({ sequenceCount, delayMs }, `Sending sequence of ${sequenceCount} packets...`);

      let fulfilledCount = 0;
      let rejectedCount = 0;

      for (let i = 0; i < sequenceCount; i++) {
        const { packet } = createTestPreparePacket(
          options.destination,
          options.amount,
          expirySeconds,
          dataBuffer
        );

        try {
          const response = await sender.sendPacket(packet);

          if (response.type === PacketType.FULFILL) {
            fulfilledCount++;
          } else {
            rejectedCount++;
          }

          logger.info(
            { packetNum: i + 1, total: sequenceCount },
            `Sent packet ${i + 1}/${sequenceCount}`
          );

          // Delay before next packet (except for last packet)
          if (delayMs > 0 && i < sequenceCount - 1) {
            await new Promise((resolve) => setTimeout(resolve, delayMs));
          }
        } catch (error) {
          rejectedCount++;
          const errorMessage = error instanceof Error ? error.message : String(error);
          logger.error({ error: errorMessage, packetNum: i + 1 }, `Failed to send packet ${i + 1}`);
        }
      }

      logger.info(
        { fulfilledCount, rejectedCount },
        `Sequence complete: ${fulfilledCount} fulfilled, ${rejectedCount} rejected`
      );

      await sender.disconnect();
      process.exit(rejectedCount > 0 ? 1 : 0);
    } else {
      // Single packet mode
      const { packet } = createTestPreparePacket(
        options.destination,
        options.amount,
        expirySeconds,
        dataBuffer
      );

      logger.info(
        {
          destination: packet.destination,
          amount: packet.amount.toString(),
          expiresAt: packet.expiresAt.toISOString(),
        },
        'Packet created'
      );

      // Send packet
      const response = await sender.sendPacket(packet);

      // Log response
      if (response.type === PacketType.FULFILL) {
        logger.info(
          {
            packetType: 'FULFILL',
            fulfillment: response.fulfillment.toString('hex').substring(0, 16) + '...',
          },
          'Packet fulfilled'
        );

        await sender.disconnect();
        process.exit(0);
      } else if (response.type === PacketType.REJECT) {
        logger.warn(
          {
            packetType: 'REJECT',
            code: response.code,
            message: response.message,
            triggeredBy: response.triggeredBy,
          },
          'Packet rejected'
        );

        await sender.disconnect();
        process.exit(1);
      } else {
        logger.error('Unexpected response packet type');

        await sender.disconnect();
        process.exit(1);
      }
    }
  } catch (error) {
    const errorMessage = error instanceof Error ? error.message : String(error);
    logger.error({ error: errorMessage }, 'Failed to send packet');
    process.exit(1);
  }
});

// Parse arguments
program.parse(process.argv);
