// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// SS58 address encoding for account public keys. Needed because the DotSpark
// attestation backend expects `candidateAccountId` as an SS58 string (a hex
// account id is accepted by the HTTP layer but fails the on-chain attest), and
// the vendored wasm only exposes prefix-0 addresses / SS58 decoding — not a
// generic (prefix 42) encoder. Gecko has no blake2b (NSS/WebCrypto are SHA
// only), so the SS58 checksum hash is implemented here.
//
// blake2b is a compact port of blakejs (https://github.com/dcposch/blakejs,
// public domain / CC0), reduced to the single-shot hashing this needs.

const B58_ALPHABET =
  "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

const BLAKE2B_IV32 = new Uint32Array([
  0xf3bcc908, 0x6a09e667, 0x84caa73b, 0xbb67ae85, 0xfe94f82b, 0x3c6ef372,
  0x5f1d36f1, 0xa54ff53a, 0xade682d1, 0x510e527f, 0x2b3e6c1f, 0x9b05688c,
  0xfb41bd6b, 0x1f83d9ab, 0x137e2179, 0x5be0cd19,
]);

// prettier-ignore
const SIGMA8 = [
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
  14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3,
  11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4,
  7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8,
  9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13,
  2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9,
  12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11,
  13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10,
  6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5,
  10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0,
  0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
  14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3,
];
const SIGMA82 = new Uint8Array(SIGMA8.map(x => x * 2));

const v = new Uint32Array(32);
const m = new Uint32Array(32);

function ADD64AA(arr, a, b) {
  const o0 = arr[a] + arr[b];
  let o1 = arr[a + 1] + arr[b + 1];
  if (o0 >= 0x100000000) {
    o1++;
  }
  arr[a] = o0;
  arr[a + 1] = o1;
}

function ADD64AC(arr, a, b0, b1) {
  let o0 = arr[a] + b0;
  if (b0 < 0) {
    o0 += 0x100000000;
  }
  let o1 = arr[a + 1] + b1;
  if (o0 >= 0x100000000) {
    o1++;
  }
  arr[a] = o0;
  arr[a + 1] = o1;
}

function B2B_GET32(arr, i) {
  return arr[i] ^ (arr[i + 1] << 8) ^ (arr[i + 2] << 16) ^ (arr[i + 3] << 24);
}

function B2B_G(a, b, c, d, ix, iy) {
  const x0 = m[ix];
  const x1 = m[ix + 1];
  const y0 = m[iy];
  const y1 = m[iy + 1];

  ADD64AA(v, a, b);
  ADD64AC(v, a, x0, x1);

  let xor0 = v[d] ^ v[a];
  let xor1 = v[d + 1] ^ v[a + 1];
  v[d] = xor1;
  v[d + 1] = xor0;

  ADD64AA(v, c, d);

  xor0 = v[b] ^ v[c];
  xor1 = v[b + 1] ^ v[c + 1];
  v[b] = (xor0 >>> 24) ^ (xor1 << 8);
  v[b + 1] = (xor1 >>> 24) ^ (xor0 << 8);

  ADD64AA(v, a, b);
  ADD64AC(v, a, y0, y1);

  xor0 = v[d] ^ v[a];
  xor1 = v[d + 1] ^ v[a + 1];
  v[d] = (xor0 >>> 16) ^ (xor1 << 16);
  v[d + 1] = (xor1 >>> 16) ^ (xor0 << 16);

  ADD64AA(v, c, d);

  xor0 = v[b] ^ v[c];
  xor1 = v[b + 1] ^ v[c + 1];
  v[b] = (xor1 >>> 31) ^ (xor0 << 1);
  v[b + 1] = (xor0 >>> 31) ^ (xor1 << 1);
}

function blake2bCompress(ctx, last) {
  let i = 0;
  for (i = 0; i < 16; i++) {
    v[i] = ctx.h[i];
    v[i + 16] = BLAKE2B_IV32[i];
  }

  v[24] = v[24] ^ ctx.t;
  v[25] = v[25] ^ (ctx.t / 0x100000000);

  if (last) {
    v[28] = ~v[28];
    v[29] = ~v[29];
  }

  for (i = 0; i < 32; i++) {
    m[i] = B2B_GET32(ctx.b, 4 * i);
  }

  for (i = 0; i < 12; i++) {
    B2B_G(0, 8, 16, 24, SIGMA82[i * 16 + 0], SIGMA82[i * 16 + 1]);
    B2B_G(2, 10, 18, 26, SIGMA82[i * 16 + 2], SIGMA82[i * 16 + 3]);
    B2B_G(4, 12, 20, 28, SIGMA82[i * 16 + 4], SIGMA82[i * 16 + 5]);
    B2B_G(6, 14, 22, 30, SIGMA82[i * 16 + 6], SIGMA82[i * 16 + 7]);
    B2B_G(0, 10, 20, 30, SIGMA82[i * 16 + 8], SIGMA82[i * 16 + 9]);
    B2B_G(2, 12, 22, 24, SIGMA82[i * 16 + 10], SIGMA82[i * 16 + 11]);
    B2B_G(4, 14, 16, 26, SIGMA82[i * 16 + 12], SIGMA82[i * 16 + 13]);
    B2B_G(6, 8, 18, 28, SIGMA82[i * 16 + 14], SIGMA82[i * 16 + 15]);
  }

  for (i = 0; i < 16; i++) {
    ctx.h[i] = ctx.h[i] ^ v[i] ^ v[i + 16];
  }
}

/** Single-shot blake2b. @param {Uint8Array} input @param {number} outlen */
function blake2b(input, outlen) {
  const ctx = {
    b: new Uint8Array(128),
    h: new Uint32Array(16),
    t: 0,
    c: 0,
    outlen,
  };
  for (let i = 0; i < 16; i++) {
    ctx.h[i] = BLAKE2B_IV32[i];
  }
  ctx.h[0] ^= 0x01010000 ^ outlen;

  for (let i = 0; i < input.length; i++) {
    if (ctx.c === 128) {
      ctx.t += ctx.c;
      blake2bCompress(ctx, false);
      ctx.c = 0;
    }
    ctx.b[ctx.c++] = input[i];
  }

  ctx.t += ctx.c;
  while (ctx.c < 128) {
    ctx.b[ctx.c++] = 0;
  }
  blake2bCompress(ctx, true);

  const out = new Uint8Array(outlen);
  for (let i = 0; i < outlen; i++) {
    out[i] = ctx.h[i >> 2] >> (8 * (i & 3));
  }
  return out;
}

function base58(bytes) {
  let x = 0n;
  for (const b of bytes) {
    x = x * 256n + BigInt(b);
  }
  let out = "";
  while (x > 0n) {
    out = B58_ALPHABET[Number(x % 58n)] + out;
    x /= 58n;
  }
  for (const b of bytes) {
    if (b === 0) {
      out = "1" + out;
    } else {
      break;
    }
  }
  return out;
}

/**
 * SS58-encode a public key. Only single-byte prefixes (< 64) are supported,
 * which covers the substrate default (42) this host uses.
 *
 * @param {Uint8Array} pubkey - 32-byte account public key.
 * @param {number} prefix - SS58 network prefix (default 42, substrate generic).
 * @returns {string} the SS58 address.
 */
export function encodeSs58(pubkey, prefix = 42) {
  if (prefix >= 64) {
    throw new Error(`unsupported SS58 prefix: ${prefix}`);
  }
  const payload = new Uint8Array(1 + pubkey.length);
  payload[0] = prefix;
  payload.set(pubkey, 1);

  const preimage = new Uint8Array(7 + payload.length);
  preimage.set([0x53, 0x53, 0x35, 0x38, 0x50, 0x52, 0x45], 0); // "SS58PRE"
  preimage.set(payload, 7);
  const checksum = blake2b(preimage, 64);

  const full = new Uint8Array(payload.length + 2);
  full.set(payload, 0);
  full[payload.length] = checksum[0];
  full[payload.length + 1] = checksum[1];
  return base58(full);
}

// Namespace export for consumers that lazy-import via
// ChromeUtils.defineESModuleGetters ({ EpocaSs58: "..." } → lazy.EpocaSs58).
export const EpocaSs58 = { encodeSs58 };
