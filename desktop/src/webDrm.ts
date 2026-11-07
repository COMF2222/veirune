/** SoundCloud's public FairPlay envelope. The CDM owns all content keys. */
export function licenseChallenge(system: string, challenge: Uint8Array, assetUri: string): Uint8Array | string {
  if (system !== 'com.apple.fps') return challenge
  let binary = ''
  for (let i = 0; i < challenge.length; i += 8192) binary += String.fromCharCode(...challenge.subarray(i, i + 8192))
  return 'spc=' + btoa(binary) + '&assetId=' + assetUri.replace(/^skd:\/\//, '')
}

export function licenseResponse(system: string, response: ArrayBuffer): ArrayBuffer {
  if (system !== 'com.apple.fps') return response
  const binary = atob(new TextDecoder().decode(response).trim())
  return Uint8Array.from(binary, char => char.charCodeAt(0)).buffer
}
