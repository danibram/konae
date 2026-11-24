const keys = __FIXTURE_KEYS__;
const data = new Uint8Array([0, 107, 111, 110, 97, 101, 0]).subarray(1, 6);
const { subtle } = crypto;
const hex = value => Array.from(new Uint8Array(value), byte => byte.toString(16).padStart(2, '0')).join('');
const unbase64 = value => Uint8Array.fromBase64
  ? Uint8Array.fromBase64(value, { alphabet: 'base64url' })
  : Uint8Array.from(atob(value.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
const text = value => new TextEncoder().encode(value);

function check(value, label) {
  if (!value) throw new Error(label);
}

async function rejects(name, operation) {
  try {
    await operation();
  } catch (error) {
    check(error.name === name, `Expected ${name}, got ${error.name}: ${error.message}`);
    return;
  }
  throw new Error(`Expected ${name}`);
}

const digestVectors = {
  'SHA-1': 'a9993e364706816aba3e25717850c26c9cd0d89d',
  'SHA-256': 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
  'SHA-384': 'cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7',
  'SHA-512': 'ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f',
};

const hmacVectors = {
  'SHA-1': 'b617318655057264e28bc0b6fb378c8ef146be00',
  'SHA-256': 'b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7',
  'SHA-384': 'afd03944d84895626b0825f4ab46907f15f9dadbe4101ec682aa034c7cebc59cfaea9ea9076ede7f4af152e8b2fa9cb6',
  'SHA-512': '87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854',
};

const asymmetric = [
  {
    fixture: keys.ES256,
    parameters: { name: 'ECDSA', namedCurve: 'P-256' },
    operation: { name: 'ECDSA', hash: 'SHA-256' },
    vector: 'EX2bfaWGTYbwlfcFeZV4ovViHwO__ecA3GxoQvqvRh1nvDJk01GNM6by1ZlhX6Pw2IBEoUHtkhYvTXJJtIEPLA',
  },
  {
    fixture: keys.RS256,
    parameters: { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' },
    operation: 'RSASSA-PKCS1-v1_5',
    vector: 'SeKztTb67YmSz100S6Lrv80KFQI7d3CfmWk1uv63-CjKUfe-JlZT05LYGDP-mtnlq0ldPPPrJfj0xWKH9FcegIr-MUofGc07evS6GpNVycY5ohbSV-3TuTdB9dTHo8JB0aamzcbothTmZgD5xRdxGtkPsrUvH_cBMEmCHzHUwnB1SSGX-FNPiuvAVl7nc6_2T3F3jKdBGY6RURbsWQskQF6Xm4p8iWHPtRGYpGhiEH8-N4c3L_sHi5HC67sG8xdq14cT7xrEvaBmp0MPEmKltfQe85e5UlpKyr-pEwuoCCN0_nbq3K_boWECiY6rDCkX4jpQmhcoe-qBQ9hi93gF4w',
  },
  {
    fixture: keys.RS256,
    parameters: { name: 'RSA-PSS', hash: 'SHA-256' },
    operation: { name: 'RSA-PSS', saltLength: 0 },
    vector: 'vDwzkN7Ux59LDiRHMOw9mKsdBIrsnaVfRkYjedGy_2eK4Urn4QwdDEc1pmqNpVB5ckb8hjQkCrx47a7X02Gy54w_KxMqUG48aj6wRLG4xu33LiCAXFowe6JrICN6QPFp3VVd72Lzl5ETzuUqG7_914m2cnW7WZuo0D2iYdWwnBejhdT_b4k4oaMGlPspPj7jxGnXmiXKb2IFwQImgXa3UhEIj8a2IY5cPMwb0gjrbLUDhR021qhGy1hIYhg-qb74vNdmdxemn_AW8u-jozDHhLNWmKDdn3s1mYTPzCdaK1VIwnV1uO0x5AdallV1yh-IcVIndkgzIcxEwSr3MIru2g',
  },
];

let stored;

export async function handle(request) {
  if (new URL(request.url).pathname === '/reuse') {
    return new Response(String(stored === undefined));
  }
  for (const [algorithm, vector] of Object.entries(digestVectors)) {
    const offset = new DataView(Uint8Array.of(0, 97, 98, 99, 0).buffer, 1, 3);
    check(hex(await subtle.digest(algorithm.toLowerCase(), offset)) === vector, algorithm);
  }
  const raw = new Uint8Array(20).fill(11);
  for (const [hash, vector] of Object.entries(hmacVectors)) {
    const algorithm = { name: 'HMAC', hash };
    const usages = ['sign', 'verify'];
    const key = await subtle.importKey('raw', raw, algorithm, true, usages);
    stored = key;
    check(key instanceof CryptoKey && key.type === 'secret' && key.algorithm.length === 160, 'HMAC key');
    check(hex(await subtle.exportKey('raw', key)) === hex(raw), 'raw round trip');
    const mac = await subtle.sign('hmac', key, text('Hi There'));
    check(hex(mac) === vector, hash + ' HMAC');
    if (hash !== 'SHA-1') {
      const jwk = await subtle.exportKey('jwk', key);
      check(jwk.ext === true && jwk.kty === 'oct' && jwk.alg === 'HS' + hash.slice(4), 'HMAC JWK');
      const imported = await subtle.importKey('jwk', jwk, algorithm, false, usages);
      check(await subtle.verify('HMAC', imported, mac, text('Hi There')), 'HMAC verify');
      check(!await subtle.verify('HMAC', imported, mac, text('Hi there')), 'changed message');
      await rejects('InvalidAccessError', () => subtle.exportKey('raw', imported));
      await rejects('DataError', () => subtle.importKey('jwk', { ...jwk, ext: false }, algorithm, true, usages));
    }
  }
  for (const { fixture, parameters, operation, vector } of asymmetric) {
    const name = parameters.name;
    const publicKey = await subtle.importKey('jwk', fixture.publicKey, parameters, true, ['verify']);
    const privateKey = await subtle.importKey('jwk', fixture.privateKey, parameters, true, ['sign']);
    const known = unbase64(vector);
    check(publicKey instanceof CryptoKey && privateKey instanceof CryptoKey, name + ' CryptoKey brand');
    check(publicKey.type === 'public' && privateKey.type === 'private', name + ' key type');
    check(publicKey.algorithm.name === name, name + ' key algorithm');
    if (name !== 'ECDSA') {
      check(publicKey.algorithm.modulusLength === 2048, name + ' key length');
      check(hex(publicKey.algorithm.publicExponent) === '010001', name + ' exponent');
    }
    check(await subtle.verify(operation, publicKey, known, data), name + ' fixed signature');
    check(!await subtle.verify(operation, publicKey, known, text('changed')), name + ' wrong message');
    const spki = await subtle.exportKey('spki', publicKey);
    const pkcs8 = await subtle.exportKey('pkcs8', privateKey);
    const spkiKey = await subtle.importKey('spki', spki, parameters, true, ['verify']);
    const pkcs8Key = await subtle.importKey('pkcs8', pkcs8, parameters, false, ['sign']);
    const signature = await subtle.sign(operation, pkcs8Key, data);
    if (name !== 'ECDSA') check(hex(signature) === hex(known), name + ' fixed signing');
    check(await subtle.verify(operation, spkiKey, signature, data), name + ' DER round trip');
    const jwk = await subtle.exportKey('jwk', privateKey);
    check(Object.entries(fixture.privateKey).every(([field, value]) => jwk[field] === value), name + ' JWK round trip');
    await rejects('InvalidAccessError', () => subtle.sign(operation, spkiKey, data));
    if (name === 'ECDSA') {
      const rawPublic = await subtle.exportKey('raw', publicKey);
      const rawKey = await subtle.importKey('raw', rawPublic, parameters, false, ['verify']);
      check(await subtle.verify(operation, rawKey, signature, data), 'ECDSA raw round trip');
      check(signature.byteLength === 64, 'ECDSA raw signature length');
    }
    if (name === 'RSA-PSS') {
      const withSalt = { name, saltLength: 7 };
      const salted = await subtle.sign(withSalt, privateKey, data);
      check(await subtle.verify(withSalt, publicKey, salted, data), 'PSS specified salt length');
      check(!await subtle.verify(operation, publicKey, salted, data), 'PSS mismatched salt length');
    }
  }
  await rejects('TypeError', () => subtle.sign('HMAC', { type: 'secret' }, data));
  await rejects('NotSupportedError', () => subtle.digest('MD5', data));
  const buffer = new Uint8Array(18).fill(0xff);
  const out = buffer.subarray(1, 17);
  check(crypto.getRandomValues(out) === out && buffer[0] === 0xff && buffer[17] === 0xff, 'random view');
  check(hex(crypto.getRandomValues(new Uint8Array(16))) !== hex(out), 'independent random samples');
  await rejects('QuotaExceededError', () => crypto.getRandomValues(new Uint8Array(65537)));
  check(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()), 'UUID bits');
  return new Response('passed');
}
