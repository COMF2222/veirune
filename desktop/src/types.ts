export type Track = {
  id: number
  title: string
  duration?: number | null
  full_duration_ms?: number | null
  artwork_url?: string | null
  waveform_url?: string | null
  artwork?: { '150x150'?: string | null; '500x500'?: string | null } | null
  user?: { id: number; username: string; avatar_url?: string | null } | null
  metadata_artist?: string | null
  publisher_metadata?: { artist?: string | null; album_title?: string | null; isrc?: string | null } | null
  genre?: string | null
  tag_list?: string | null
  playback_count?: number | null
  likes_count?: number | null
  comment_count?: number | null
  description?: string | null
  created_at?: string | null
  permalink_url?: string | null
  policy?: string | null
  access?: string | null
  monetization_model?: string | null
  feed_reposted?: boolean
}

export type Playlist = {
  id: number
  title: string
  artwork_url?: string | null
  artwork?: { '150x150'?: string | null; '500x500'?: string | null } | null
  user?: { id: number; username: string; avatar_url?: string | null } | null
  track_count?: number | null
  genre?: string | null
  tag_list?: string | null
  likes_count?: number | null
  is_album?: boolean
  playlist_type?: string | null
  set_type?: string | null
  duration_ms?: number | null
  created_at?: string | null
  permalink_url?: string | null
  feed_reposted?: boolean
  tracks?: Track[]
}

export type User = { id: number; username: string; avatar_url?: string | null; full_name?: string | null; description?: string | null; city?: string | null; country_code?: string | null; permalink_url?: string | null; followers_count: number; followings_count?: number; track_count?: number; public_playlists_count?: number | null }
export type Me = { id: number; username: string; avatar_url?: string | null; followers_count?: number | null }
export type Comment = { id: number; body: string; created_at?: string | null; timestamp_ms?: number | null; user?: { id: number; username: string; avatar_url?: string | null } | null }
export type LyricsRecord = { id: number; trackName: string; artistName: string; albumName?: string | null; duration?: number | null; plainLyrics?: string | null; syncedLyrics?: string | null; instrumental: boolean; source?: string; sourceUrl?: string | null }
export type LyricTrackMatch = { track: Track; excerpt: string; matchScore: number; recordingScore: number }
export type WebProfile = { title?: string | null; url: string; service?: string | null }
export type QuickAccessShortcut =
  | { track: { id: number; title: string; artist: string; artwork_url: string | null } }
  | { playlist: { id: number; title: string; artist: string; artwork_url: string | null } }
  | { album: { id: number; title: string; artist: string; artwork_url: string | null } }
  | 'likes' | 'daily_mix' | 'fresh' | 'vibe' | 'history' | 'station'
export const quickAccessTarget = (item: QuickAccessShortcut) => typeof item === 'string' ? null : 'track' in item ? { kind: 'track' as const, ...item.track } : 'playlist' in item ? { kind: 'playlist' as const, ...item.playlist } : { kind: 'album' as const, ...item.album }
export type Data<T> = { status: 'loading' } | { status: 'unavailable' } | { status: 'ready'; data: T } | { status: 'failed'; data: string }
export function libraryCollections(sources: (Data<Playlist[]> | undefined)[], albums: boolean): Data<Playlist[]> {
  const items = new Map<number, Playlist>()
  for (const source of sources) if (source?.status === 'ready') for (const item of source.data) {
    if (!!(item.is_album || item.playlist_type === 'album' || item.set_type === 'album') === albums) items.set(item.id, item)
  }
  if (items.size) return { status: 'ready', data: [...items.values()] }
  if (sources.some(source => !source || source.status === 'loading')) return { status: 'loading' }
  const errors = sources.flatMap(source => source?.status === 'failed' ? [source.data] : [])
  if (errors.length) return { status: 'failed', data: errors.join('\n') }
  return { status: 'ready', data: [] }
}
export type Connection =
  | { status: 'demo' | 'public' | 'signed_in' | 'connecting' | 'registering' }
  | { status: 'pairing'; code: string; url: string }
  | { status: 'error'; message: string }
export type PlayerState = {
  canUndoQueue: boolean
  sleepRemainingMs: number | null
  sleepAfterTrack: boolean
  queue: Track[]
  current: number | null
  waveActive: boolean
  isPlaying: boolean
  loading: boolean
  positionMs: number
  durationMs: number
  previewFallback: boolean
  volume: number
  playbackSpeed: number
  shuffle: boolean
  repeat: 'Off' | 'All' | 'One'
  abStartMs: number | null
  abEndMs: number | null
  bitrateKbps: number
  sampleRate: number
  error: string | null
}
export type OfflineEntry = { track: Track; bytes: number; pinned: boolean }
export type PlaylistFolder = { name: string; playlistIds: number[]; pinned: boolean; order: number }
export type SmartPlaylist = { name: string; genre: string; addedDays: number; unplayedDays: number; limit: number }
export type PersonalCollections = { folders: Record<string, PlaylistFolder>; smartPlaylists: Record<string, SmartPlaylist>; syncStatus: string }
export type ListeningTrack = { trackId: number; title: string; artist: string; genre: string; ms: number; plays: number; lastPlayed: number }
export type ListeningStatistics = { totals: { ms: number; plays: number }; daily: { day: string; ms: number; plays: number }[]; tracks: ListeningTrack[]; artists?: { name: string; ms: number }[]; syncStatus: string }
export type OfflineCapacity = { usedBytes: number; freeBytes: number; limitBytes: number }
export type MusicTaste = { discovery: number; diversity: number; repeat_days: number; genres: string[] }
export type ProblemReport = { version: string; os: string; architecture: string; connection: string; queueLength: number; playing: boolean; loading: boolean; errorStage: string; sampleRate: number; bitrateKbps: number; settingsVersion: number }
export type StorageReport = { installationBytes: number; clapModelBytes: number; clapRuntimeBytes: number; clapPreparationBytes: number; offlineBytes: number; audioCacheBytes: number; artworkCacheBytes: number; otherDataBytes: number; otherCacheBytes: number; extraAppDataBytes: number; installationPath: string; dataPath: string; cachePath: string; extraAppDataPath: string }
export type MainWindowBounds = { x: number; y: number; width: number; height: number; maximized: boolean }
export type Settings = { normalization: boolean; crossfade_ms: number; gapless: boolean; settings_version: number; music_taste: MusicTaste; wave_mode: 'Balanced' | 'Familiar' | 'Discover' | 'Mood' | 'NoRepeats'; wave_mood: 'Calm' | 'Sad' | 'Happy' | 'Workout' | 'Road'; leaderboard_anonymous: boolean; show_community: boolean; theme_presets: import("./themePresets").ThemePreset[]; offline_limit_mb: number; autoplay: boolean; compact_rows: boolean; visualiser: 'Spectrum' | 'Scope' | 'Off'; theme: 'Dark' | 'Light' | 'System'; language: 'English' | 'Russian'; liked_ids: number[]; followed_user_ids: number[]; quick_access: QuickAccessShortcut[]; inbox: { label: string; link: string; at: number }[]; mono: boolean; balance: number; eq_enabled: boolean; eq_preamp_db: number; eq_gains_db: number[]; startup_page: 'Home' | 'Search' | 'Library' | 'Settings'; main_window_bounds: MainWindowBounds | null; close_to_tray: boolean; memory_profile: 'Eco' | 'Balanced' | 'Quality'; reduced_motion: boolean; accent_rgb: number[]; panel_rgb: number[] | null; panel_opacity: number; panel_blur: number; heading_opacity: number | null; text_rgb: number[] | null; muted_text_rgb: number[] | null; interface_text_scale: number; interface_scale: number; background_image: string | null; interface_font: string | null; background_opacity: number; background_dim: number; background_blur: number; background_overlay: number; lyrics_scale: number; lyrics_blur_past: boolean; lyrics_auto_scroll: boolean; show_track_numbers: boolean; soundcloud_profile_url: string | null; discord_client_id: string; discord_presence: boolean; audio_cache_limit_mb: number; eq_auto: boolean; mini_player_style: 'Airwave' | 'Winamp'; winamp_window: boolean; winamp_on_top: boolean; winamp_skin: string | null; winamp_shade: boolean; winamp_eq_window: boolean; winamp_eq_shade: boolean; winamp_pl_window: boolean; winamp_pl_shade: boolean; winamp_pl_rows: number; winamp_scale: number }
export type VisualiserFrame = { bars: number[]; peaks: (number | null)[]; scope: number[] }
export type WaveformSamples = { values: number[]; height: number }

const comparableArtistName = (name: string) => name.normalize('NFKC').toLocaleLowerCase().replace(/[\p{P}\p{S}\s]+/gu, '')
export const sameArtistName = (a: string, b: string) => comparableArtistName(a) === comparableArtistName(b)
const cleanAccountName = (name: string) => name.replace(/^[\p{S}\s]+|[\p{S}\s]+$/gu, '').trim() || name.trim()
export const artistCredit = (track: Track) => (track.publisher_metadata?.artist || track.metadata_artist || '').trim()
export const artist = (track: Track) => {
  const credit = artistCredit(track)
  const account = track.user?.username?.trim()
  if (!credit) return account ? cleanAccountName(account) : '—'
  return credit
}
export const creditedArtists = (track: Track): string[] => {
  const credit = artistCredit(track)
  if (!credit) return [artist(track)]
  const names = credit
    .replace(/\s*[(\[]\s*(?=(?:feat(?:uring)?|ft)\.?\s)/gi, ' ')
    .split(/\s*[,;]\s*|\s+(?:feat(?:uring)?\.?|ft\.?|x|×|&|and|и|\+|\/)\s+/i)
    .map(name => name.replace(/^[\s([{"']+|[\s)\]}"']+$/g, '').trim())
    .filter(Boolean)
  return [...new Map(names.map(name => [name.toLocaleLowerCase(), name])).values()]
}
export const cover = (item: Track | Playlist): string | undefined => {
  const direct = item.artwork?.['500x500'] || item.artwork?.['150x150'] || item.artwork_url
  if (direct) return direct
  return 'tracks' in item ? item.tracks?.map(cover).find((url): url is string => !!url) : undefined
}
export const soundcloudImageVariant = (source: string, variant: 't200x200' | 't500x500' | 't1080x1080' | 'original') => {
  try {
    const url = new URL(source)
    if (url.hostname !== 'sndcdn.com' && !url.hostname.endsWith('.sndcdn.com')) return source
    url.pathname = url.pathname.replace(/-(?:original|large|small|crop|t\d+x\d+)(\.[^/.]+)$/i, `-${variant}$1`)
    return url.toString()
  } catch { return source }
}

export const soundcloudImageAt = (source: string, pixels: number) => {
  // SoundCloud serves fixed CDN variants. Requests like t160x160 and
  // t320x320 return 404, leaving a tiny fallback stretched across the UI.
  return soundcloudImageVariant(source, pixels > 500 ? 'original' : pixels > 200 ? 't500x500' : 't200x200')
}

export const artworkForSize = (item: Track | Playlist, size: 'row' | 'card' | 'hero', profile: Settings['memory_profile']) => {
  const pixels = profile === 'Eco' ? 200 : profile === 'Balanced' && size === 'hero' ? 500 : { row: 200, card: 500, hero: 1080 }[size]
  const source = cover(item)
  if (!source) return undefined
  return soundcloudImageAt(source, pixels)
}
export const duration = (ms?: number | null) => {
  const seconds = typeof ms === 'number' && Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0
  const minutes = Math.floor(seconds / 60)
  const rest = String(seconds % 60).padStart(2, '0')
  return minutes < 60 ? `${minutes}:${rest}` : `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, '0')}:${rest}`
}

export interface LeaderboardEntry {
  rank: number; anonymous: boolean; isMe: boolean; name: string | null
  ms: number; plays: number; uniqueTracks: number
  artists: { name: string; ms: number }[]
  tracks: { title: string; artist: string; ms: number }[]
}
export interface LeaderboardData { days: number; sort: string; updatedAt: number; entries: LeaderboardEntry[]; mine: LeaderboardEntry | null; participants: number }
