import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  encodeVarUint,
  encodeVarOctetString,
  encodeGeneralizedTime,
  encodePrepare,
  TYPE_PREPARE,
} from './oer';

test('encodeVarUint: 0-127 encodes as a single byte', () => {
  assert.deepEqual(encodeVarUint(0), Buffer.from([0x00]));
  assert.deepEqual(encodeVarUint(1), Buffer.from([0x01]));
  assert.deepEqual(encodeVarUint(100), Buffer.from([0x64]));
  assert.deepEqual(encodeVarUint(127), Buffer.from([0x7f]));
});

test('encodeVarUint: 128+ encodes as a length-prefixed big-endian value', () => {
  assert.deepEqual(encodeVarUint(128), Buffer.from([0x81, 0x80]));
  assert.deepEqual(encodeVarUint(255), Buffer.from([0x81, 0xff]));
  assert.deepEqual(encodeVarUint(256), Buffer.from([0x82, 0x01, 0x00]));
  assert.deepEqual(encodeVarUint(65535), Buffer.from([0x82, 0xff, 0xff]));
  assert.deepEqual(encodeVarUint(65536), Buffer.from([0x83, 0x01, 0x00, 0x00]));
});

test('encodeVarUint: rejects negative and non-integer values', () => {
  assert.throws(() => encodeVarUint(-1), RangeError);
  assert.throws(() => encodeVarUint(1.5), RangeError);
});

test('encodeVarOctetString: length-prefixes the bytes', () => {
  const data = Buffer.from('hello');
  const encoded = encodeVarOctetString(data);
  assert.equal(encoded[0], 5);
  assert.deepEqual(encoded.subarray(1), data);
});

test('encodeGeneralizedTime: formats as YYYYMMDDHHMMSS.fffZ (19 bytes)', () => {
  const when = new Date(Date.UTC(2030, 5, 15, 12, 0, 0, 0)); // month is 0-indexed: June
  const encoded = encodeGeneralizedTime(when);
  assert.equal(encoded.length, 19);
  assert.equal(encoded.toString('utf8'), '20300615120000.000Z');
});

test('encodeGeneralizedTime: pads milliseconds to 3 digits', () => {
  const when = new Date(Date.UTC(2026, 0, 1, 0, 0, 0, 7));
  assert.equal(encodeGeneralizedTime(when).toString('utf8'), '20260101000000.007Z');
});

test('encodePrepare: leads with the PREPARE type byte and matches the Rust decoder field order', () => {
  const encoded = encodePrepare({
    amount: 100,
    expiresAt: new Date(Date.UTC(2030, 5, 15, 12, 0, 0, 0)),
    greeting: false,
    destination: 'g.example.app',
    data: Buffer.from('hello app'),
  });

  assert.equal(encoded[0], TYPE_PREPARE);
  assert.equal(TYPE_PREPARE, 12);

  // amount (VarUInt, 100 <= 127 => single byte 0x64)
  assert.equal(encoded[1], 0x64);
  // expiresAt (19-byte GeneralizedTime) starts at offset 2
  assert.equal(encoded.subarray(2, 21).toString('utf8'), '20300615120000.000Z');
  // greeting: 1 byte at offset 21
  assert.equal(encoded[21], 0x00);
  // destination: VarOctetString at offset 22 — length byte then the UTF-8 bytes
  const destBytes = Buffer.from('g.example.app', 'utf8');
  assert.equal(encoded[22], destBytes.length);
  assert.deepEqual(encoded.subarray(23, 23 + destBytes.length), destBytes);
  // data: VarOctetString right after
  const dataOffset = 23 + destBytes.length;
  const dataBytes = Buffer.from('hello app');
  assert.equal(encoded[dataOffset], dataBytes.length);
  assert.deepEqual(encoded.subarray(dataOffset + 1), dataBytes);
});

test('encodePrepare: greeting true encodes as 0x01', () => {
  const encoded = encodePrepare({
    amount: 0,
    expiresAt: new Date(Date.UTC(2030, 5, 15, 12, 0, 0, 0)),
    greeting: true,
    destination: 'g.example.app',
  });
  // type byte (1) + amount (0x00, 1 byte) + expiresAt (19 bytes) => greeting at offset 21
  assert.equal(encoded[21], 0x01);
});

test('encodePrepare: defaults data to empty when omitted', () => {
  const encoded = encodePrepare({
    amount: 0,
    expiresAt: new Date(Date.UTC(2030, 0, 1)),
    greeting: true,
    destination: 'g.x',
  });
  // last byte is the (zero) length prefix of the empty data VarOctetString
  assert.equal(encoded[encoded.length - 1], 0);
});
