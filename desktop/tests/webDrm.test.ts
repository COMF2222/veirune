import { test } from 'node:test'
import assert from 'node:assert/strict'
import { licenseChallenge, licenseResponse } from '../src/webDrm.ts'

test('Widevine and PlayReady CDM messages pass through unchanged', () => {
  const challenge = new Uint8Array([0, 128, 255])
  for (const system of ['com.widevine.alpha', 'com.microsoft.playready']) {
    assert.equal(licenseChallenge(system, challenge, ''), challenge)
    assert.equal(licenseResponse(system, challenge.buffer), challenge.buffer)
  }
})

test('FairPlay wraps the opaque SPC with the signaled asset ID and decodes its CKC envelope', () => {
  assert.equal(licenseChallenge('com.apple.fps', new Uint8Array([0, 128, 255]), 'skd://recording-id'), 'spc=AID/&assetId=recording-id')
  assert.deepEqual(new Uint8Array(licenseResponse('com.apple.fps', new TextEncoder().encode(' AID/\n').buffer)), new Uint8Array([0, 128, 255]))
  assert.throws(() => licenseResponse('com.apple.fps', new TextEncoder().encode('invalid!').buffer))
})
