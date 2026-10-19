import { previewChat } from './chatPreview'
import type { ChatAction } from './chatTypes'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { captureTheme, validateTheme, type ThemePreset } from './themePresets'
import type { PersonalCollections, ListeningStatistics, SmartPlaylist, ProblemReport, OfflineCapacity, Comment, Connection, Data, LyricsRecord, LyricTrackMatch, Me, OfflineEntry, PlayerState, Playlist, QuickAccessShortcut, Settings, StorageReport, Track, User, VisualiserFrame, WaveformSamples, WebProfile } from './types'
import { cachedLibraryData, clearLibraryCache, forgetLibraryEntry, setCacheConnection } from './libraryCache'
import { emptySpotifyImport, type SpotifyImportSelection, type SpotifyImportView } from './spotifyImportTypes'
import { releaseNotesUrl } from './releaseNotes'

const preview = !('__TAURI_INTERNALS__' in window)
type BridgePlayerState = Omit<PlayerState, 'queue'> & { queue: Track[] | null; queueRevision: number }
let playerQueue: { revision: number; tracks: Track[] } | null = null

async function fullPlayerState(): Promise<PlayerState> {
  const view = await invoke<BridgePlayerState>('player_state', { knownQueueRevision: null })
  if (view.queue === null) throw new Error('Player queue snapshot is missing')
  if (playerQueue && view.queueRevision < playerQueue.revision) return playerState()
  playerQueue = { revision: view.queueRevision, tracks: view.queue }
  const { queueRevision: _, ...state } = view
  return { ...state, queue: view.queue }
}

async function playerState(): Promise<PlayerState> {
  const view = await invoke<BridgePlayerState>('player_state', { knownQueueRevision: playerQueue?.revision ?? null })
  if (view.queue !== null) {
    if (playerQueue && view.queueRevision < playerQueue.revision) return fullPlayerState()
    playerQueue = { revision: view.queueRevision, tracks: view.queue }
  } else if (playerQueue?.revision !== view.queueRevision) {
    return fullPlayerState()
  }
  const { queueRevision: _, ...state } = view
  return { ...state, queue: playerQueue!.tracks }
}

async function afterChange<T>(request: Promise<T>, ...entries: string[]): Promise<T> {
  const value = await request
  entries.forEach(forgetLibraryEntry)
  return value
}
const demo: Track[] = [
  'Northern Lights', 'Sunset Drive', 'Rain on Glass', 'Neon District', 'Paper Planes',
  'Golden Hour', 'Static Fields', 'Low Tide', 'Concrete Garden', 'Afterglow',
].map((title, index) => ({ id: 1000 + index, title, duration: index === 9 ? 140 * 60000 : 120000 + index * 13000, user: { id: 1, username: 'SoundCloud Demo' }, genre: 'Ambient / Electronic', playback_count: 10000 + index * 2341 }))
let previewPlayer: PlayerState = { canUndoQueue: false, sleepRemainingMs: null, sleepAfterTrack: false, queue: [...demo], current: null, waveActive: false, isPlaying: false, loading: false, positionMs: 0, durationMs: 0, previewFallback: false, volume: .8, playbackSpeed: 1, shuffle: false, repeat: 'Off', abStartMs: null, abEndMs: null, bitrateKbps: 0, sampleRate: 0, error: null }
const previewLikes = new Set<number>()
const previewWaveDislikes = new Set<number>()
const previewFeedbackTracks = new Map<number, Track>()
const previewOffline = new Map<number, OfflineEntry>()
let previewOfflineLikeOrder: number[] = []
const previewPlaylists: Playlist[] = ['Neon Nights', 'Low Tide Radio', 'Concrete Garden Mix'].map((title, i) => ({ id: 2001 + i, title, track_count: [4, 3, 5][i], is_album: i === 0, user: { id: 1, username: 'SoundCloud Demo' } }))
const previewPlaylistTracks = new Map<number, number[]>()
const previewFollowed = new Set<number>()
const previewUsers: User[] = [{ id: 1, username: 'SoundCloud Demo', full_name: 'Veirune Preview', description: 'Здесь можно посмотреть, как будут выглядеть треки, альбомы и плейлисты твоего профиля.', followers_count: 0, track_count: demo.length, public_playlists_count: previewPlaylists.length }]
previewUsers.push({ id: 2, username: 'Luna Waves', full_name: 'Luna', followers_count: 120, track_count: 4 }, { id: 3, username: 'Demo Listener', full_name: 'Алексей', followers_count: 0, track_count: 0 })
const previewSettings: Settings = { wave_mode: 'Balanced', wave_mood: 'Calm', leaderboard_anonymous: true, show_community: true, normalization: false, crossfade_ms: 0, gapless: true, settings_version: 2, music_taste: { discovery: .5, diversity: .5, repeat_days: 2, genres: [] }, theme_presets: [], offline_limit_mb: 0, autoplay: true, compact_rows: false, visualiser: 'Spectrum', theme: 'Dark', language: 'Russian', liked_ids: [], followed_user_ids: [], quick_access: [], inbox: [], mono: false, balance: 0, eq_enabled: false, eq_preamp_db: 0, eq_gains_db: Array(10).fill(0), startup_page: 'Home', main_window_bounds: null, close_to_tray: true, memory_profile: 'Balanced', reduced_motion: false, accent_rgb: [255, 85, 25], background_image: null, interface_font: null, discord_client_id: '', discord_presence: false, background_opacity: .25, background_dim: 0, background_blur: 0, background_overlay: .8, panel_rgb: null, panel_opacity: .85, panel_blur: 12, heading_opacity: null, text_rgb: null, muted_text_rgb: null, interface_text_scale: 1, interface_scale: 1, lyrics_scale: 1, lyrics_blur_past: true, lyrics_auto_scroll: true, show_track_numbers: true, soundcloud_profile_url: null, audio_cache_limit_mb: 2048, eq_auto: false, mini_player_style: 'Airwave', winamp_window: false, winamp_on_top: false, winamp_skin: null, winamp_shade: false, winamp_eq_window: false, winamp_eq_shade: false, winamp_pl_window: false, winamp_pl_shade: false, winamp_pl_rows: 8, winamp_scale: 2 }
const previewText = (ru: string, en: string) => previewSettings.language === 'English' ? en : ru
const previewComments: Comment[] = []
const previewReposts = new Set<number>()
const previewRepostedPlaylists = new Set<number>()
const previewLikedPlaylists = new Set<number>()

function downloadJson(value: unknown, name: string) {
  const url = URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: 'application/json' }))
  const link = document.createElement('a'); link.href = url; link.download = name; link.click()
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

const previewCollections: PersonalCollections = { folders: {}, smartPlaylists: {}, syncStatus: 'demo' }
const previewQueueHistory: PlayerState[] = []
let previewSleepUntil: number | null = null
const previewLikedAt = new Map<number, number>()

const previewEditorial = new Set<number>()
async function communityRequest<T>(action: 'home'|'role'|'pick'|'editor', input: Record<string,unknown> = {}): Promise<T> {
  if (!preview) return invoke<T>('community_request',{action,input})
  if(action === 'pick') { if(input.active) previewEditorial.add(Number(input.trackId)); else previewEditorial.delete(Number(input.trackId)) }
  const role={canEdit:true,isOwner:false,pickedIds:[...previewEditorial]}
  const rows=demo.slice(0,3).map((track,index)=>({trackId:track.id,track,listeners:5-index}))
  return (action === 'home' ? {role,today:rows,yesterday:rows.slice(1),week:rows,editorial:demo.filter(t=>previewEditorial.has(t.id)).map(track=>({trackId:track.id,track,votes:1,pickedByMe:true})),day:'2026-10-19',timezone:'UTC'} : role) as T
}

export const api = {
  community: communityRequest,
  leaderboard: (days: number, sort: string) => preview ? Promise.resolve<import('./types').LeaderboardData>({ days, sort, updatedAt: Date.now() / 1000, entries: [], mine: null, participants: 0 }) : invoke<import('./types').LeaderboardData>('leaderboard', { days, sort }),
  leaderboardAnonymity: (anonymous: boolean) => preview ? (previewSettings.leaderboard_anonymous = anonymous, Promise.resolve()) : invoke<void>('leaderboard_anonymity', { anonymous }),

  chat: <T>(action: ChatAction, input: Record<string, unknown> = {}) => preview ? previewChat<T>(action, input) : invoke<T>('chat_request', { action, input }),
  serverOperations: (serverUrl: string, settings?: Record<string, number>) => preview ? Promise.resolve<import('./ServerOperations').OperationsSnapshot | null>(null) : invoke<import('./ServerOperations').OperationsSnapshot>('server_operations', { serverUrl, settings: settings ?? null }),
  serverIncident: (serverUrl: string, body: Record<string, string | number>) => invoke<unknown>('server_incident', { serverUrl, body }),
  personalCollections: () => preview ? Promise.resolve({ ...previewCollections }) : invoke<PersonalCollections>('personal_collections'),
  listeningStatistics: () => preview ? Promise.resolve<ListeningStatistics>({ totals: { ms: 0, plays: 0 }, daily: [], tracks: [], syncStatus: 'demo' }) : invoke<ListeningStatistics>('listening_statistics'),
  updatePersonalCollection: async (section: 'folders' | 'smartPlaylists', id: string, value: unknown) => {
    if (!preview) return invoke<void>('update_personal_collection', { section, id, value })
    if (value === null) delete previewCollections[section][id]
    else Object.assign(previewCollections[section], { [id]: value })
  },
  smartPlaylistTracks: (id: string) => {
    if (!preview) return invoke<Data<Track[]>>('smart_playlist_tracks', { id })
    const rule = previewCollections.smartPlaylists[id] as SmartPlaylist | undefined
    if (!rule) return Promise.resolve<Data<Track[]>>({ status: 'failed', data: 'Smart playlist not found' })
    const now = Date.now() / 1000
    return Promise.resolve<Data<Track[]>>({ status: 'ready', data: demo.filter(track => previewLikes.has(track.id) && (!rule.genre || track.genre?.toLowerCase().includes(rule.genre.toLowerCase())) && (!rule.addedDays || (previewLikedAt.get(track.id) || 0) >= now - rule.addedDays * 86400)).slice(0, rule.limit) })
  },
  sleepTimer: async (seconds: number | null, afterTrack: boolean) => {
    if (!preview) return invoke<void>('sleep_timer', { seconds, afterTrack })
    previewSleepUntil = seconds == null ? null : Date.now() + seconds * 1000
    previewPlayer.sleepAfterTrack = afterTrack
  },
  syncPersonalData: () => preview ? Promise.resolve() : invoke<void>('sync_personal_data'),
  settingsBackup: async (action: 'export' | 'import'): Promise<Settings> => {
    if (preview) {
      if (action === 'import') throw new Error(previewText('Импорт резервной копии доступен в установленном приложении.', 'Backup import is available in the installed app.'))
      downloadJson({ version: 1, preferences: { ...captureTheme(previewSettings, 'Backup').values, language: previewSettings.language, music_taste: previewSettings.music_taste, theme_presets: previewSettings.theme_presets, quick_access: previewSettings.quick_access, normalization: previewSettings.normalization, crossfade_ms: previewSettings.crossfade_ms, gapless: previewSettings.gapless, autoplay: previewSettings.autoplay, compact_rows: previewSettings.compact_rows, show_track_numbers: previewSettings.show_track_numbers, lyrics_auto_scroll: previewSettings.lyrics_auto_scroll }, folders: previewCollections.folders, smartPlaylists: previewCollections.smartPlaylists }, 'veirune-settings.json')
      return { ...previewSettings }
    }
    const selected = action === 'export' ? await save({ defaultPath: 'veirune-settings.json', filters: [{ name: 'JSON', extensions: ['json'] }] }) : await open({ multiple: false, filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (typeof selected !== 'string') return api.settings()
    return invoke<Settings>('settings_backup', { action, path: selected })
  },
  themeAction: async (action: 'apply' | 'save' | 'delete' | 'export' | 'import', name?: string, preset?: ThemePreset): Promise<Settings> => {
    let path: string | null = null
    if (action === 'export') {
      if (preview) { downloadJson(captureTheme(previewSettings, name || 'Veirune theme'), 'veirune-theme.json'); return { ...previewSettings } }
      path = await save({ defaultPath: 'veirune-theme.json', filters: [{ name: 'JSON', extensions: ['json'] }] })
      if (!path) return api.settings()
    }
    if (action === 'import') {
      if (preview) throw new Error(previewText('Импорт файла доступен в установленном приложении.', 'File import is available in the installed app.'))
      const chosen = await open({ multiple: false, filters: [{ name: 'Veirune theme', extensions: ['json'] }] })
      path = typeof chosen === 'string' ? chosen : null
      if (!path) return api.settings()
    }
    if (!preview) return invoke<Settings>('theme_action', { action, name, preset, path })
    if (action === 'apply' && preset) Object.assign(previewSettings, validateTheme(preset).values)
    if (action === 'save') {
      const theme = validateTheme(captureTheme(previewSettings, name || ''))
      const others = previewSettings.theme_presets.filter(item => item.name !== theme.name)
      if (others.length >= 20) throw new Error('You can save up to 20 themes')
      previewSettings.theme_presets = [...others, theme]
    }
    if (action === 'delete') previewSettings.theme_presets = previewSettings.theme_presets.filter(item => item.name !== name)
    return { ...previewSettings }
  },
  problemReport: () => preview ? Promise.resolve<ProblemReport>({ version: '0.3.0', os: 'preview', architecture: 'browser', connection: 'demo', queueLength: previewPlayer.queue.length, playing: previewPlayer.isPlaying, loading: previewPlayer.loading, errorStage: 'none', sampleRate: previewPlayer.sampleRate, bitrateKbps: previewPlayer.bitrateKbps, settingsVersion: 2 }) : invoke<ProblemReport>('problem_report'),
  saveProblemReport: async (report: ProblemReport) => {
    if (preview) { downloadJson(report, 'veirune-report.json'); return }
    const path = await save({ defaultPath: 'veirune-report.json', filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (path) await invoke('save_problem_report', { path, report })
  },
  offlineCapacity: () => preview ? Promise.resolve<OfflineCapacity>({ usedBytes: [...previewOffline.values()].reduce((sum, entry) => sum + entry.bytes, 0), freeBytes: 10 * 1024 ** 3, limitBytes: previewSettings.offline_limit_mb * 1048576 }) : invoke<OfflineCapacity>('offline_capacity'),
  pinOfflineTrack: (trackId: number, pinned: boolean) => {
    if (!preview) return invoke<void>('pin_offline_track', { trackId, pinned })
    const entry = previewOffline.get(trackId)
    if (entry) previewOffline.set(trackId, { ...entry, pinned })
    return Promise.resolve()
  },
  clearUnpinnedTracks: () => {
    if (!preview) return invoke<void>('clear_unpinned_tracks')
    for (const [id, entry] of previewOffline) if (!entry.pinned && id !== previewPlayer.queue[previewPlayer.current ?? -1]?.id) previewOffline.delete(id)
    return Promise.resolve()
  },

  connection: async () => {
    const value = preview ? { status: 'demo' } as Connection : await invoke<Connection>('connection')
    setCacheConnection(value.status)
    return value
  },
  myProfile: () => preview ? Promise.resolve<Data<Me>>({ status: 'ready', data: { id: 1, username: 'SoundCloud Demo', followers_count: 0 } }) : cachedLibraryData('profile', () => invoke<Data<Me>>('my_profile')),
  takePendingLink: () => preview ? Promise.resolve<string | null>(null) : invoke<string | null>('take_pending_link'),
  player: () => {
    if (!preview) return playerState()
    if (previewSleepUntil !== null && previewSleepUntil <= Date.now()) { previewPlayer.isPlaying = false; previewSleepUntil = null }
    return Promise.resolve({ ...previewPlayer, canUndoQueue: previewQueueHistory.length > 0, sleepRemainingMs: previewSleepUntil === null ? null : Math.max(0, previewSleepUntil - Date.now()) })
  },
  imageData: async (url: string) => {
    if (preview) return url
    const source = await invoke<string>('image_data', { url })
    // A disk-write failure still displays the image through the bounded fallback.
    return source.startsWith('data:') ? source : convertFileSrc(source)
  },
  waveformSamples: (url: string) => preview ? Promise.reject<WaveformSamples>(new Error('Waveform unavailable in preview')) : invoke<WaveformSamples>('waveform_samples', { url }),
  offlineTracks: () => preview ? Promise.resolve([...previewOffline.values()].sort((a, b) => {
    const left = previewOfflineLikeOrder.indexOf(a.track.id)
    const right = previewOfflineLikeOrder.indexOf(b.track.id)
    return (left < 0 ? Number.MAX_SAFE_INTEGER : left) - (right < 0 ? Number.MAX_SAFE_INTEGER : right)
  })) : invoke<OfflineEntry[]>('offline_tracks'),
  setOfflineLikeOrder: (trackIds: number[]) => {
    if (!preview) return invoke<void>('set_offline_like_order', { trackIds })
    previewOfflineLikeOrder = [...trackIds]
    return Promise.resolve()
  },
  myWave: (likedTracks: Track[], variation: number) => {
    if (!preview) return invoke<Track[]>('my_wave', { likedTracks: likedTracks.slice(0, 500), variation })
    if (!likedTracks.length) return Promise.reject(new Error('Like a few tracks to start My Wave'))
    const allowed = likedTracks.filter(track => !previewWaveDislikes.has(track.id))
    if (!allowed.length) return Promise.reject(new Error('No playable liked tracks are available for My Wave'))
    const seed = allowed[variation % allowed.length]
    return Promise.resolve([seed, ...demo.filter(track => !likedTracks.some(liked => liked.id === track.id) && !previewWaveDislikes.has(track.id))])
  },
  station: async (tracks: Track[], kind: 'track' | 'collection' | 'artist') => {
    if (!preview) return invoke<void>('start_station', { tracks: tracks.slice(0, 5000), kind })
    const allowed = tracks.filter(track => !previewWaveDislikes.has(track.id) && track.access !== 'preview' && track.access !== 'blocked' && !['SNIP', 'BLOCK'].includes(track.policy || ''))
    if (!allowed.length) throw new Error('No playable radio seeds')
    await api.play([allowed[kind === 'track' ? 0 : Math.floor(Math.random() * allowed.length)], ...demo.filter(track => !previewWaveDislikes.has(track.id))], 0, true)
  },
  waveDisliked: (trackId: number) => preview ? Promise.resolve(previewWaveDislikes.has(trackId)) : invoke<boolean>('wave_disliked', { trackId }),
  waveDislikes: () => preview ? Promise.resolve([...previewWaveDislikes].flatMap(id => { const track = previewFeedbackTracks.get(id); return track ? [track] : [] })) : invoke<Track[]>('wave_dislikes'),
  waveDislike: (track: Track, disliked: boolean) => {
    if (!preview) return invoke<void>('wave_dislike', { track, disliked })
    previewFeedbackTracks.set(track.id, track)
    if (disliked) {
      previewWaveDislikes.add(track.id)
      const current = previewPlayer.queue[previewPlayer.current ?? -1]
      const next = current?.id === track.id
        ? previewPlayer.queue.slice((previewPlayer.current ?? -1) + 1).find(item => !previewWaveDislikes.has(item.id))
        : current
      const queue = previewPlayer.queue.filter(item => !previewWaveDislikes.has(item.id))
      const index = next ? queue.findIndex(item => item.id === next.id) : -1
      previewPlayer = { ...previewPlayer, queue, current: index >= 0 ? index : null, isPlaying: index >= 0 && previewPlayer.isPlaying,
        ...(current?.id === track.id ? { positionMs: 0, durationMs: next?.duration || 0 } : {}) }
    } else previewWaveDislikes.delete(track.id)
    return Promise.resolve()
  },
  downloadOfflineTrack: (track: Track) => {
    if (!preview) return invoke<OfflineEntry>('download_offline_track', { track })
    const entry = { track, bytes: 4_000_000, pinned: false }
    previewOffline.set(track.id, entry)
    return Promise.resolve(entry)
  },
  removeOfflineTrack: (trackId: number) => {
    if (!preview) return invoke<void>('remove_offline_track', { trackId })
    previewOffline.delete(trackId)
    return Promise.resolve()
  },
  clearOfflineTracks: () => {
    if (!preview) return invoke<void>('clear_offline_tracks')
    previewOffline.clear()
    return Promise.resolve()
  },
  storageReport: () => preview ? Promise.resolve<StorageReport>({ installationBytes: 0, clapModelBytes: 0, clapRuntimeBytes: 0, clapPreparationBytes: 0, offlineBytes: [...previewOffline.values()].reduce((sum, item) => sum + item.bytes, 0), audioCacheBytes: 0, artworkCacheBytes: 0, otherDataBytes: 0, otherCacheBytes: 0, extraAppDataBytes: 0, installationPath: '', dataPath: '', cachePath: '', extraAppDataPath: '' }) : invoke<StorageReport>('storage_report'),
  clearArtworkCache: () => preview ? Promise.resolve() : invoke<void>('clear_artwork_cache'),
  clearClapPreparation: () => preview ? Promise.resolve() : invoke<void>('clear_clap_preparation'),
  tracks: async (view: string, query?: string, id?: number) => {
    const result = preview
    ? { status: 'ready', data: view === 'search' ? demo.filter(t => t.title.toLowerCase().includes((query || '').toLowerCase())) : view === 'likes' ? demo.filter(t => previewLikes.has(t.id)) : view === 'reposts' ? demo.filter(t => previewReposts.has(t.id)) : view === 'feed' ? [1001, 1003, 1005, 1007, 1000].map(trackId => demo.find(t => t.id === trackId)).filter((t): t is Track => !!t).map(t => ({ ...t, feed_reposted: t.id === 1003 })) : view === 'uploads' ? [] : view === 'playlist' && id ? previewPlaylistTracks.has(id) ? (previewPlaylistTracks.get(id) || []).map(trackId => demo.find(t => t.id === trackId)).filter((t): t is Track => !!t) : demo.slice(0, previewPlaylists.find(p => p.id === id)?.track_count || 0) : view === 'artist' ? demo.filter(t => t.user?.id === id) : view === 'related' ? demo.filter(t => t.id !== id) : demo } as Data<Track[]>
    : !query && id == null && ['likes', 'uploads', 'reposts'].includes(view)
      ? cachedLibraryData(`tracks:${view}`, () => invoke<Data<Track[]>>('tracks', { view, query, id }))
      : invoke<Data<Track[]>>('tracks', { view, query, id })
    const data = await result
    return preview && ['discover', 'related', 'recommended_genre'].includes(view) && data.status === 'ready'
      ? { ...data, data: data.data.filter(track => !previewWaveDislikes.has(track.id)) } : data
  },
  playlists: (view: string, query?: string) => preview
    ? Promise.resolve<Data<Playlist[]>>({ status: 'ready', data: previewPlaylists.filter(list => view === 'feed' ? false : view === 'liked' ? previewLikedPlaylists.has(list.id) : view === 'reposts' ? previewRepostedPlaylists.has(list.id) : view === 'search' ? list.title.toLowerCase().includes((query || '').toLowerCase()) : view.startsWith('artist') ? false : true) })
    : !query && ['liked', 'mine', 'reposts'].includes(view)
      ? cachedLibraryData(`playlists:${view}`, () => invoke<Data<Playlist[]>>('playlists', { view, query }))
      : invoke<Data<Playlist[]>>('playlists', { view, query }),
  catalogReleases: (day: number) => preview ? Promise.resolve<Data<Playlist[]>>({ status: 'ready', data: previewPlaylists }) : invoke<Data<Playlist[]>>('catalog_releases', { day }),
  users: (query: string) => preview ? Promise.resolve<Data<User[]>>({ status: 'ready', data: previewUsers.filter(user => user.username.toLowerCase().includes(query.toLowerCase())) }) : invoke<Data<User[]>>('users', { query }),
  track: (id: number) => preview ? Promise.resolve<Data<Track>>(demo.find(track => track.id === id) ? { status: 'ready', data: demo.find(track => track.id === id)! } : { status: 'failed', data: previewText('Трек не найден', 'Track not found') }) : invoke<Data<Track>>('track_detail', { id }),
  user: (id: number) => preview ? Promise.resolve<Data<User>>(previewUsers.find(user => user.id === id) ? { status: 'ready', data: { ...previewUsers.find(user => user.id === id)!, description: previewText('Здесь можно посмотреть, как будут выглядеть треки, альбомы и плейлисты твоего профиля.', 'Preview how tracks, albums and playlists will look on your profile.') } } : { status: 'failed', data: previewText('Автор не найден', 'Artist not found') }) : invoke<Data<User>>('user_detail', { id }),
  playlist: (id: number) => preview ? Promise.resolve<Data<Playlist>>(previewPlaylists.find(list => list.id === id) ? { status: 'ready', data: previewPlaylists.find(list => list.id === id)! } : { status: 'failed', data: previewText('Плейлист не найден', 'Playlist not found') }) : invoke<Data<Playlist>>('playlist_detail', { id }),
  comments: (id: number) => preview ? Promise.resolve<Data<Comment[]>>({ status: 'ready', data: previewComments.filter(comment => comment.id === id) }) : invoke<Data<Comment[]>>('comments', { id }),
  trackLyrics: (artist: string, title: string, durationMs: number, albumName?: string | null, isrc?: string | null) => preview ? Promise.resolve<LyricsRecord>({
    id: 0, trackName: title, artistName: artist, source: 'Veirune Demo', instrumental: false,
    syncedLyrics: '[00:00.00]Огни за окном\n[00:08.00]Город дышит тишиной\n[00:16.00]Музыка рядом\n[00:24.00]И дорога ведёт домой\n[00:32.00]Новый день впереди\n[00:40.00]Мы оставим свет за собой\n[00:48.00]Слушай этот момент\n[00:56.00]Он останется с тобой',
  }) : invoke<LyricsRecord | null>('track_lyrics', { artist, title, durationMs, albumName: albumName || null, isrc: isrc || null }),
  openLyricsSource: (raw: string) => preview ? Promise.resolve(void window.open(raw, '_blank', 'noopener,noreferrer')) : invoke<void>('open_lyrics_source', { raw }),
  searchLyrics: (query: string) => preview ? Promise.resolve<LyricsRecord[]>([]) : invoke<LyricsRecord[]>('search_lyrics', { query }),
  lyricTracks: (query: string) => preview ? Promise.reject<LyricTrackMatch[]>(new Error('PREVIEW_SEARCH')) : invoke<LyricTrackMatch[]>('lyric_tracks', { query }),
  postComment: (id: number, body: string, timestampMs?: number) => preview ? Promise.resolve(void previewComments.push({ id, body, timestamp_ms: timestampMs, user: { id: 1, username: previewText('Вы', 'You') } })) : invoke<void>('post_comment', { id, body, timestampMs }),
  userProfiles: (id: number) => preview ? Promise.resolve<Data<WebProfile[]>>({ status: 'ready', data: [] }) : invoke<Data<WebProfile[]>>('user_profiles', { id }),
  relatedUsers: (id: number) => preview ? Promise.resolve<Data<User[]>>({ status: 'ready', data: previewUsers.filter(user => user.id !== id) }) : invoke<Data<User[]>>('related_users', { id }),
  openLink: (raw: string) => preview ? Promise.reject<{ kind: string; id: number; title: string }>(new Error(previewText('Ссылки SoundCloud открываются в приложении', 'SoundCloud links open in the desktop app'))) : invoke<{ kind: 'track' | 'playlist' | 'user'; id: number; title: string }>('open_link', { raw }),
  openSoundCloud: (url: string) => preview ? Promise.reject<void>(new Error(previewText('Открой ссылку в установленном приложении', 'Open this link in the installed app'))) : invoke<void>('open_soundcloud_url', { raw: url }),
  openReleaseNotes: (version: string, english: boolean) => {
    const raw = releaseNotesUrl(version, english)
    return preview ? (window.open(raw, '_blank', 'noopener,noreferrer'), Promise.resolve()) : invoke<void>('open_release_notes', { raw })
  },
  vibeSearch: (query: string) => preview ? Promise.reject<Track[]>(new Error('PREVIEW_SEARCH')) : invoke<Track[]>('vibe_search', { query }),
  repost: (kind: 'track' | 'playlist', id: number, active: boolean) => {
    if (!preview) return afterChange(invoke<void>('repost', { kind, id, active }), kind === 'track' ? 'tracks:reposts' : 'playlists:reposts')
    if (kind === 'track') { if (active) previewReposts.add(id); else previewReposts.delete(id) }
    else { if (active) previewRepostedPlaylists.add(id); else previewRepostedPlaylists.delete(id) }
    return Promise.resolve()
  },
  likePlaylist: (id: number, liked: boolean) => {
    if (!preview) return afterChange(invoke<void>('like_playlist', { id, liked }), 'playlists:liked')
    if (liked) previewLikedPlaylists.add(id); else previewLikedPlaylists.delete(id)
    return Promise.resolve()
  },
  following: () => preview ? Promise.resolve<Data<User[]>>({ status: 'ready', data: previewUsers.filter(user => previewFollowed.has(user.id)) }) : invoke<Data<User[]>>('following'),
  refreshFollowing: () => preview ? Promise.resolve() : invoke<void>('refresh_following'),
  setFollowed: (userId: number, followed: boolean) => {
    if (!preview) return invoke<void>('set_followed', { userId, followed })
    if (followed) previewFollowed.add(userId); else previewFollowed.delete(userId)
    return Promise.resolve()
  },
  saveQueuePlaylist: async (title: string, trackIds: number[]) => {
    if (!preview) return afterChange(invoke<Playlist>('create_playlist', { title, trackIds }), 'playlists:mine')
    const item = await api.createPlaylist(title)
    await api.addTracksToPlaylist(item.id, trackIds)
    return item
  },
  createPlaylist: (title: string, trackId?: number) => {
    if (!preview) return afterChange(invoke<Playlist>('create_playlist', { title, trackId }), 'playlists:mine')
    const item: Playlist = { id: Math.max(8999, ...previewPlaylists.map(p => p.id)) + 1, title: title.trim(), track_count: trackId ? 1 : 0 }
    previewPlaylists.push(item)
    previewPlaylistTracks.set(item.id, trackId ? [trackId] : [])
    return Promise.resolve(item)
  },
  addToPlaylist: (playlistId: number, trackId: number) => {
    if (!preview) return afterChange(invoke<void>('add_to_playlist', { playlistId, trackId }), 'playlists:mine')
    const ids = previewPlaylistTracks.get(playlistId) || []
    if (!ids.includes(trackId)) ids.push(trackId)
    previewPlaylistTracks.set(playlistId, ids)
    const index = previewPlaylists.findIndex(p => p.id === playlistId)
    if (index >= 0) previewPlaylists[index] = { ...previewPlaylists[index], track_count: ids.length }
    return Promise.resolve()
  },
  addTracksToPlaylist: (playlistId: number, trackIds: number[]) => {
    if (!preview) return afterChange(invoke<void>('add_tracks_to_playlist', { playlistId, trackIds }), 'playlists:mine')
    const ids = previewPlaylistTracks.get(playlistId) || []
    for (const trackId of trackIds) if (!ids.includes(trackId)) ids.push(trackId)
    previewPlaylistTracks.set(playlistId, ids)
    const index = previewPlaylists.findIndex(p => p.id === playlistId)
    if (index >= 0) previewPlaylists[index] = { ...previewPlaylists[index], track_count: ids.length }
    return Promise.resolve()
  },
  removeFromPlaylist: (playlistId: number, trackId: number) => {
    if (!preview) return afterChange(invoke<void>('remove_from_playlist', { playlistId, trackId }), 'playlists:mine')
    const ids = (previewPlaylistTracks.get(playlistId) || []).filter(id => id !== trackId)
    previewPlaylistTracks.set(playlistId, ids)
    const index = previewPlaylists.findIndex(p => p.id === playlistId)
    if (index >= 0) previewPlaylists[index] = { ...previewPlaylists[index], track_count: ids.length }
    return Promise.resolve()
  },
  moveInPlaylist: (playlistId: number, from: number, to: number) => {
    if (!preview) return afterChange(invoke<void>('move_in_playlist', { playlistId, from, to }), 'playlists:mine')
    const ids = previewPlaylistTracks.get(playlistId)
    if (!ids || from < 0 || to < 0 || from >= ids.length || to >= ids.length) return Promise.reject(new Error(previewText('Невозможно переставить трек', 'Cannot reorder track')))
    const [id] = ids.splice(from, 1)
    ids.splice(to, 0, id)
    return Promise.resolve()
  },
  renamePlaylist: (playlistId: number, title: string) => {
    if (!preview) return afterChange(invoke<void>('rename_playlist', { playlistId, title }), 'playlists:mine')
    const index = previewPlaylists.findIndex(p => p.id === playlistId)
    if (index >= 0) previewPlaylists[index] = { ...previewPlaylists[index], title: title.trim() }
    return Promise.resolve()
  },
  deletePlaylist: (playlistId: number) => {
    if (!preview) return afterChange(invoke<void>('delete_playlist', { playlistId }), 'playlists:mine')
    const index = previewPlaylists.findIndex(p => p.id === playlistId)
    if (index >= 0) previewPlaylists.splice(index, 1)
    previewPlaylistTracks.delete(playlistId)
    return Promise.resolve()
  },
  transport: (action: string, value?: number, index?: number, target?: number) => {
    if (!preview) return invoke<void>('transport', { action, value, index, target })
    if (['remove', 'move', 'clear_played', 'clear_upcoming', 'clear_queue'].includes(action)) {
      previewQueueHistory.push(structuredClone(previewPlayer)); if (previewQueueHistory.length > 5) previewQueueHistory.shift()
    }
    if (action === 'undo_queue') {
      const saved = previewQueueHistory.pop()
      if (saved) {
        const id = previewPlayer.queue[previewPlayer.current ?? -1]?.id
        const current = saved.queue.findIndex(track => track.id === id)
        previewPlayer = current >= 0 ? { ...previewPlayer, queue: saved.queue, current } : saved
      }
    }
    const moveTo = (next: number) => {
      previewPlayer.current = previewPlayer.queue[next] ? next : null
      previewPlayer.positionMs = 0
      previewPlayer.durationMs = previewPlayer.queue[next]?.full_duration_ms || previewPlayer.queue[next]?.duration || 0
    }
    if (action === 'toggle') previewPlayer.isPlaying = !previewPlayer.isPlaying
    if (action === 'next') moveTo(Math.min((previewPlayer.current ?? -1) + 1, previewPlayer.queue.length - 1))
    if (action === 'previous') moveTo(Math.max((previewPlayer.current ?? 0) - 1, 0))
    if (action === 'volume') previewPlayer.volume = value || 0
    if (action === 'speed') previewPlayer.playbackSpeed = Math.max(.5, Math.min(2, value || 1))
    if (action === 'seek') previewPlayer.positionMs = value || 0
    if (action === 'shuffle') previewPlayer.shuffle = !previewPlayer.shuffle
    if (action === 'repeat') previewPlayer.repeat = previewPlayer.repeat === 'Off' ? 'All' : previewPlayer.repeat === 'All' ? 'One' : 'Off'
    if (action === 'ab_loop') {
      if (previewPlayer.abEndMs != null) { previewPlayer.abStartMs = null; previewPlayer.abEndMs = null }
      else if (previewPlayer.abStartMs == null) previewPlayer.abStartMs = previewPlayer.positionMs
      else if (previewPlayer.positionMs >= previewPlayer.abStartMs + 500) previewPlayer.abEndMs = previewPlayer.positionMs
    }
    if (action === 'ab_clear') { previewPlayer.abStartMs = null; previewPlayer.abEndMs = null }
    if (action === 'skip_to') moveTo(index ?? 0)
    if (action === 'remove' && index != null) {
      previewPlayer.queue = previewPlayer.queue.filter((_, i) => i !== index)
      if (previewPlayer.current != null) previewPlayer.current = index < previewPlayer.current ? previewPlayer.current - 1 : index === previewPlayer.current ? null : previewPlayer.current
    }
    if (action === 'move' && index != null && target != null && index >= 0 && target >= 0 && index < previewPlayer.queue.length && target < previewPlayer.queue.length) {
      const queue = [...previewPlayer.queue]; const [item] = queue.splice(index, 1); queue.splice(target, 0, item); previewPlayer.queue = queue
      if (previewPlayer.current === index) previewPlayer.current = target
      else if (previewPlayer.current != null && index < target && previewPlayer.current > index && previewPlayer.current <= target) previewPlayer.current--
      else if (previewPlayer.current != null && index > target && previewPlayer.current >= target && previewPlayer.current < index) previewPlayer.current++
    }
    if (action === 'retry') { previewPlayer.error = null; previewPlayer.isPlaying = true }
    if (action === 'clear_played' && previewPlayer.current != null) { previewPlayer.queue = previewPlayer.queue.slice(previewPlayer.current); previewPlayer.current = 0 }
    if (action === 'clear_upcoming' && previewPlayer.current != null) { previewPlayer.queue = [previewPlayer.queue[previewPlayer.current]]; previewPlayer.current = 0 }
    if (action === 'clear_queue') previewPlayer = { ...previewPlayer, queue: [], current: null, isPlaying: false }
    if (action === 'stop') previewPlayer.isPlaying = false
    return Promise.resolve()
  },
  play: (tracks: Track[], index: number, recommended = false) => {
    if (!preview) return invoke<void>('play_tracks', { tracks, index, recommended })
    if (recommended) {
      const selected = tracks[index]?.id
      tracks = tracks.filter(track => !previewWaveDislikes.has(track.id))
      index = tracks.findIndex(track => track.id === selected)
      if (index < 0) return Promise.reject(new Error('Track was excluded from recommendations'))
    }
    previewPlayer = { ...previewPlayer, queue: [...tracks], current: index, isPlaying: true, durationMs: tracks[index]?.duration || 0, positionMs: 0, abStartMs: null, abEndMs: null }
    return Promise.resolve()
  },
  enqueue: (track: Track, next = false) => {
    if (!preview) return invoke<void>('enqueue', { track, next })
    previewPlayer.queue = [...previewPlayer.queue]
    previewPlayer.queue.splice(next ? (previewPlayer.current ?? -1) + 1 : previewPlayer.queue.length, 0, track)
    return Promise.resolve()
  },
  enqueueTracks: (tracks: Track[], next = false) => {
    if (!preview) return invoke<void>('enqueue_tracks', { tracks, next })
    previewPlayer.queue = [...previewPlayer.queue]
    previewPlayer.queue.splice(next ? (previewPlayer.current ?? -1) + 1 : previewPlayer.queue.length, 0, ...tracks)
    return Promise.resolve()
  },
  settings: () => preview ? Promise.resolve<Settings>({ ...previewSettings, liked_ids: [...previewLikes], followed_user_ids: [...previewFollowed] }) : invoke<Settings>('settings'),
  toggleQuickAccess: (item: QuickAccessShortcut) => {
    if (!preview) return invoke<boolean>('toggle_quick_access', { item })
    const target = typeof item === 'string' ? item : 'track' in item ? `track:${item.track.id}` : 'playlist' in item ? `playlist:${item.playlist.id}` : `album:${item.album.id}`
    const index = previewSettings.quick_access.findIndex(candidate => (typeof candidate === 'string' ? candidate : 'track' in candidate ? `track:${candidate.track.id}` : 'playlist' in candidate ? `playlist:${candidate.playlist.id}` : `album:${candidate.album.id}`) === target)
    if (index >= 0) { previewSettings.quick_access = previewSettings.quick_access.filter((_, i) => i !== index); return Promise.resolve(false) }
    previewSettings.quick_access = [item, ...previewSettings.quick_access]
    return Promise.resolve(true)
  },
  pickBackground: () => preview ? Promise.resolve<string | null>(null) : open({ multiple: false, directory: false, filters: [{ name: previewText('Изображения', 'Images'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'] }] }),
  saveBackground: (path: string) => preview ? Promise.reject<string>(new Error(previewText('Локальный фон доступен в приложении', 'Local backgrounds are available in the desktop app'))) : invoke<string>('save_background', { path }),
  pickFont: () => preview ? Promise.resolve<string | null>(null) : open({ multiple: false, directory: false, filters: [{ name: previewText('Шрифты', 'Fonts'), extensions: ['ttf', 'otf', 'ttc', 'otc', 'woff', 'woff2'] }] }),
  saveFont: (path: string) => preview ? Promise.reject<string>(new Error(previewText('Свой шрифт доступен в приложении', 'Custom fonts are available in the desktop app'))) : invoke<string>('save_font', { path }),
  visualiserFrame: () => preview ? Promise.resolve<VisualiserFrame>({ bars: Array(19).fill(0), peaks: Array(19).fill(null), scope: Array(75).fill(7) }) : invoke<VisualiserFrame>('visualiser_frame'),
  pickAudioFile: () => preview ? Promise.resolve<string | null>(null) : open({ multiple: false, directory: false, filters: [{ name: previewText('Аудио', 'Audio'), extensions: ['mp3', 'm4a', 'wav', 'flac', 'ogg', 'aac'] }] }),
  setSetting: (key: string, value: unknown) => {
    if (!preview) return invoke<void>('set_setting', { key, value })
    Object.assign(previewSettings, { [key]: value })
    return Promise.resolve()
  },
  audioCache: (clear = false) => preview ? Promise.resolve(0) : invoke<number>('audio_cache', { clear }),
  eqPreset: (clear = false) => preview ? Promise.resolve() : invoke<void>('eq_preset', { clear }),
  importStatus: () => preview ? Promise.resolve({ running: false, current: 0, total: 0, matched: 0, title: '', message: '' }) : invoke<{ running: boolean; current: number; total: number; matched: number; title: string; message: string }>('import_status'),
  spotifyImportStatus: () => preview ? Promise.resolve(emptySpotifyImport) : invoke<SpotifyImportView>('spotify_import_status'),
  pickSpotifyExport: () => preview ? Promise.resolve<string[] | null>(null) : open({ multiple: true, directory: false, filters: [{ name: 'Spotify export', extensions: ['csv', 'json', 'zip'] }] }),
  previewSpotifyImport: (paths: string[]) => invoke<SpotifyImportView>('preview_spotify_import', { paths }),
  startSpotifyImport: (selections: SpotifyImportSelection[]) => invoke<void>('start_spotify_import', { selections }),
  resumeImport: (retryMissing = false, token: string | null = null) => invoke<void>('resume_import', { retryMissing, token }),
  discardImport: () => invoke<void>('discard_import'),
  cancelSpotifyImport: () => invoke<void>('cancel_spotify_import'),
  openSpotifyExport: () => preview ? (window.open('https://exportify.net/', '_blank', 'noopener,noreferrer'), Promise.resolve()) : invoke<void>('open_spotify_export'),
  checkYandexToken: (token: string) => preview ? Promise.reject<number>(new Error(previewText('Проверка доступна в приложении', 'Verification is available in the desktop app'))) : invoke<number>('check_yandex_token', { token }),
  startYandexImport: (token: string) => preview ? Promise.reject(new Error(previewText('Импорт доступен после подключения SoundCloud', 'Import is available after connecting SoundCloud'))) : invoke<void>('start_yandex_import', { token }),
  uploadTrack: (input: { path: string; title: string; artist: string; description: string; genre: string; tags: string; public: boolean }) => preview ? Promise.reject<Track>(new Error(previewText('Загрузка доступна после входа в SoundCloud', 'Upload is available after signing in to SoundCloud'))) : afterChange(invoke<Track>('upload_track', input), 'tracks:uploads'),
  editTrack: (id: number, title: string, artist: string, description: string) => preview ? Promise.reject<Track>(new Error(previewText('Редактирование доступно после входа в SoundCloud', 'Editing is available after signing in to SoundCloud'))) : invoke<Track>('edit_track', { id, title, artist, description }),
  deleteTrack: (id: number) => preview ? Promise.reject<void>(new Error(previewText('Удаление доступно после входа в SoundCloud', 'Deleting is available after signing in to SoundCloud'))) : afterChange(invoke<void>('delete_track', { id }), 'tracks:uploads'),
  saveStorefront: (input: { id: number; title: string; kind: string; link: string; linkTitle: string; description: string; price: string }) => preview ? Promise.reject<void>(new Error(previewText('Витрина доступна после входа в SoundCloud', 'Storefront is available after signing in to SoundCloud'))) : invoke<void>('save_storefront', input),
  setLiked: (trackId: number, liked: boolean) => {
    if (!preview) return afterChange(invoke<void>('set_liked', { trackId, liked }), 'tracks:likes')
    if (liked) { previewLikes.add(trackId); previewWaveDislikes.delete(trackId); previewLikedAt.set(trackId, Date.now() / 1000) } else previewLikes.delete(trackId)
    return Promise.resolve()
  },
  connect: async () => { if (!preview) { clearLibraryCache(); await invoke<void>('connect_account') } },
  signIn: async () => { if (!preview) { clearLibraryCache(); await invoke<void>('sign_in') } },
  signOut: async () => { if (!preview) { await invoke<void>('sign_out'); clearLibraryCache() } },
  approvalServerUrl: () => preview ? Promise.resolve<string | null>(null) : invoke<string | null>('approval_server_url'),
  connectServer: async (serverUrl: string) => { if (!preview) { await invoke<void>('connect_server', { serverUrl }); clearLibraryCache() } },
  serverSession: () => preview ? Promise.resolve<ServerSession>({ user_id: 0, admin: false }) : invoke<ServerSession>('server_session'),
  pairMobile: (code: string) => preview ? Promise.reject(new Error('Available in the installed app')) : invoke<{ user_id: number; username: string }>('pair_mobile', { code }),
  approvalUsers: (serverUrl: string) => preview ? Promise.resolve<ApprovalUser[]>([]) : invoke<ApprovalUser[]>('approval_users', { serverUrl }),
  approvalSetUser: (serverUrl: string, userId: number, status: 'approved' | 'denied' | 'pending') =>
    preview ? Promise.resolve() : invoke<void>('approval_set_user', { serverUrl, userId, status }),
  approvalSettings: (serverUrl: string, required: boolean | null = null) =>
    preview ? Promise.resolve<ApprovalSettings>({ approval_required: false }) : invoke<ApprovalSettings>('approval_settings', { serverUrl, required }),
  approvalMedia: (serverUrl: string) =>
    preview ? Promise.resolve<MediaStats | null>(null) : invoke<MediaStats>('approval_media', { serverUrl }),
  preview,
}

export type ServerSession = { user_id: number; admin: boolean }
export type ApprovalUser = { id: number; username: string; status: 'pending' | 'approved' | 'denied'; updated_at: number; last_seen: number; online?: boolean | null; stream_requests_today?: number | null; usage_day?: string | null; editor?: boolean }
export type ApprovalSettings = { approval_required: boolean }
export type MediaStats = { cache_bytes: number; cache_limit_bytes: number; cache_hits: number; cache_misses: number; active_downloads: number; peak_downloads: number; month_served_bytes: number; traffic_month: string }
