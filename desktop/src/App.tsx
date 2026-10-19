import { listenerLabel } from './communityLabels'
import { CommunityHomeSection, EditorialButton } from './CommunityDiscovery'
import { Fragment, createContext, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import * as Dialog from '@radix-ui/react-dialog'
import * as Slider from '@radix-ui/react-slider'
import { availableMonitors, currentMonitor, getCurrentWindow, LogicalSize, PhysicalPosition, PhysicalSize } from '@tauri-apps/api/window'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { convertFileSrc } from '@tauri-apps/api/core'
import {
  ArrowLeft, ArrowRight, AudioLines, ChartColumn, Trophy, Folder, Check, ChevronDown, ChevronRight, Clock3, Compass, Disc3,
  Heart, Home, Image, Library, ListMusic, LoaderCircle, MoreHorizontal, Music2, Pause,
  Play, Plus, Repeat2, Search, Settings2, Shuffle, SkipBack, SkipForward, Volume1, Volume2, VolumeX, X, Minimize2, Maximize2, ArrowUp, ArrowDown, Pin, Download, Languages, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, ThumbsDown,
} from 'lucide-react'
import { api } from './api'
import { StationButton } from './StationButton'
import { DislikedTracks } from './TrackFeedback'
import { ShareButton, ShareNotice } from './ShareButton'
import { ChatSidebarButton, FastcloudShareDialog, MessagesPage } from './ChatPages'
import { ServerOperations } from './ServerOperations'
import { ArtistCredits } from './ArtistCredits'
import { ArtistPage, CatalogPage, LibraryExtra, LibraryOverview, PlaylistPage, RepostsPage, TrackComments, TrackCreatorTools } from './ExtraPages'
import { QueuePanel } from './QueuePanel'
import { PlaybackError, TasteSettings } from './AppImprovements'
import { BackupSettings, CollectionsPage, SleepTimerButton, StatisticsPage } from './PersonalPages'
import { SettingsSections, type SettingsSection } from './SettingsSections'
import { NowPlaying } from './NowPlaying'
import { WaveGreeting } from './HomeGreeting'
import { WaveModes, LeaderboardPage, LeaderboardPrivacy } from './Community'
import { Wallpaper } from './Wallpaper'
import { LyricsSidePanel } from './LyricsSidePanel'
import { QuickAccessList } from './QuickAccessList'
import { GenreCarousel, musicGenres } from './GenreCarousel'
import { sameGenre } from './genres'
import { favouriteGenres, searchRecommendations } from './searchRecommendations'
import { RemoteImage } from './RemoteImage'
import { shuffleTracks } from './shuffle'
import { useVirtualRows } from './useVirtualRows'
import { applyThemeCustomization, interfaceLayoutScale, interfaceRowHeight } from './theme'
import { useSeekSlider } from './useSeekSlider'
import { UpdateNotice, UpdateSettingsCard, UpdateSidebarButton } from './Updater'
import { LoginGate } from './LoginGate'
import { FASTCLOUD_SERVER_URL } from './server'
import { hasRestoredNavigation, useApp, type Page } from './store'
import { libraryCollections, artist, artistCredit, artworkForSize, cover, duration, quickAccessTarget, sameArtistName, type Data, type LyricTrackMatch, type MainWindowBounds, type Playlist, type QuickAccessShortcut, type Settings, type Track, type User } from './types'

const sidebar: { page: Page; label: string; english: string; icon: typeof Home }[] = [
  { page: 'home', label: 'Главная', english: 'Home', icon: Home },
  { page: 'search', label: 'Поиск', english: 'Search', icon: Search },
  { page: 'catalog', label: 'Каталог', english: 'Catalog', icon: Disc3 },
]
const title: Record<Page, string> = { home: 'Главная', discover: 'Обзор', catalog: 'Каталог', search: 'Результаты поиска', feed: 'Лента', library: 'Библиотека', offline: 'Офлайн', likes: 'Мне нравится', history: 'История', reposts: 'Репосты', inbox: 'Входящие', settings: 'Настройки', playlist: 'Плейлист', artist: 'Автор', track: 'Трек', statistics: 'Статистика', leaderboard: 'Лидерборд', collections: 'Папки и подборки', messages: 'Сообщения' }
const englishTitle: Record<Page, string> = { home: 'Home', discover: 'Discover', catalog: 'Catalog', search: 'Search results', feed: 'Feed', library: 'Library', offline: 'Offline', likes: 'Likes', history: 'History', reposts: 'Reposts', inbox: 'Inbox', settings: 'Settings', playlist: 'Playlist', artist: 'Artist', track: 'Track', statistics: 'Statistics', leaderboard: 'Leaderboard', collections: 'Folders & collections', messages: 'Messages' }
const MemoryProfile = createContext<Settings['memory_profile']>('Balanced')
const useEnglish = () => useQuery({ queryKey: ['settings'], queryFn: api.settings }).data?.language === 'English'
const windowBoundsKey = 'fastcloud:main-window-bounds-v2'
const savedWindowBounds = (): MainWindowBounds | null => {
  try {
    const saved = JSON.parse(localStorage.getItem(windowBoundsKey) || localStorage.getItem('fastcloud:main-window-bounds') || 'null') as MainWindowBounds | null
    return saved && [saved.x, saved.y, saved.width, saved.height].every(Number.isFinite) && saved.width >= 850 && saved.height >= 580 ? saved : null
  } catch { return null }
}

function dataRefreshInterval(value?: { status?: string }, readyInterval: number | false = false): number | false {
  return value?.status === 'loading' || value?.status === 'unavailable' ? 1200
    : value?.status === 'failed' ? 6000 : readyInterval
}

function useTracks(view: string, query?: string, id?: number, enabled = true) {
  return useQuery({ queryKey: ['tracks', view, query, id], queryFn: () => api.tracks(view, query, id), enabled: enabled && !!view && (view !== 'search' || !!query?.trim()), refetchInterval: result => dataRefreshInterval(result.state.data, 30000) })
}

export function Artwork({ item, size = 'row' }: { item: Track | Playlist; size?: 'row' | 'card' | 'hero' }) {
  const profile = useContext(MemoryProfile)
  if ('track_count' in item && !cover(item)) return <MissingPlaylistArtwork item={item} size={size} profile={profile} />
  return <ArtworkFrame item={item} size={size} profile={profile} />
}

function MissingPlaylistArtwork({ item, size, profile }: { item: Playlist; size: 'row' | 'card' | 'hero'; profile: Settings['memory_profile'] }) {
  const { data: coverDetail } = useQuery({ queryKey: ['playlist', item.id], queryFn: () => api.playlist(item.id), staleTime: 5 * 60_000, retry: false })
  const { data: coverTracks } = useQuery({ queryKey: ['tracks', 'playlist', item.id], queryFn: () => api.tracks('playlist', undefined, item.id), enabled: coverDetail?.status === 'ready' && !cover(coverDetail.data), staleTime: 5 * 60_000, retry: false })
  const firstTrackWithArt = coverTracks?.status === 'ready' ? coverTracks.data.find(track => !!cover(track)) : undefined
  const source = coverDetail?.status === 'ready' && cover(coverDetail.data) ? coverDetail.data : firstTrackWithArt || item
  return <ArtworkFrame item={source} size={size} profile={profile} />
}

function ArtworkFrame({ item, size, profile }: { item: Track | Playlist; size: 'row' | 'card' | 'hero'; profile: Settings['memory_profile'] }) {
  const image = artworkForSize(item, size, profile)
  const previewImage = size === 'hero' ? artworkForSize(item, 'row', profile) : undefined
  const hue = (item.id * 47) % 360
  return <div className={`art art-${size}`} style={{ '--art-hue': hue } as React.CSSProperties}>
    {image ? <RemoteImage src={image} previewSrc={previewImage} fallback={cover(item)} alt="" loading={size === 'hero' ? 'eager' : 'lazy'} /> : <Music2 size={size === 'row' ? 20 : 38} strokeWidth={1.5} />}
  </div>
}

export function Empty({ message, detail }: { message: string; detail?: string }) {
  return <div className="empty"><Disc3 size={34} strokeWidth={1.3} /><strong>{message}</strong>{detail && <span>{detail}</span>}</div>
}

export function Status<T>({ value, children }: { value?: Data<T>; children: (rows: T) => React.ReactNode }) {
  const english = useEnglish()
  if (!value || value.status === 'loading') return <div className="status"><LoaderCircle className="spin" size={22} /> {english ? 'Loading music…' : 'Загружаем музыку…'}</div>
  if (value.status === 'unavailable') return <Empty message={english ? 'Sign-in required' : 'Нужен вход в аккаунт'} detail={english ? 'Connect SoundCloud to open your collection.' : 'Подключите SoundCloud, чтобы открыть личную коллекцию.'} />
  if (value.status === 'failed') return <Empty message={english ? 'Could not load' : 'Не удалось загрузить'} detail={String(value.data)} />
  return <>{children(value.data)}</>
}

export function TrackRows({ tracks, listenerCounts, compact = false, activeIndex, onPlay, recommended = false }: { tracks: Track[]; listenerCounts?: Record<number,number>; compact?: boolean; activeIndex?: number; onPlay?: (index: number) => Promise<void>; recommended?: boolean }) {
  const queryClient = useQueryClient()
  const { data: playback } = useQuery({
    queryKey: ['player'], queryFn: api.player,
    select: state => ({ id: state.current == null ? undefined : state.queue[state.current]?.id, playing: state.isPlaying, loading: state.loading }),
  })
  const [menu, setMenu] = useState<number | null>(null)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<Set<number>>(() => new Set())
  const [bulkBusy, setBulkBusy] = useState(false)
  const [bulkPlaylistOpen, setBulkPlaylistOpen] = useState(false)
  const selectionAnchor = useRef<number | null>(null)
  const [playlistMenu, setPlaylistMenu] = useState<number | null>(null)
  const [newPlaylist, setNewPlaylist] = useState('')
  const { data: ownPlaylists } = useQuery({ queryKey: ['playlists', 'mine'], queryFn: () => api.playlists('mine'), enabled: playlistMenu !== null || bulkPlaylistOpen, refetchInterval: result => dataRefreshInterval(result.state.data) })
  const page = useApp(s => s.page)
  const playlistId = useApp(s => s.playlistId)
  const trackIds = useMemo(() => tracks.map(track => track.id).join(','), [tracks])
  useEffect(() => { setSelected(new Set()); selectionAnchor.current = null; setBulkPlaylistOpen(false) }, [page, playlistId, trackIds])
  useEffect(() => {
    const clear = () => setSelected(new Set())
    window.addEventListener('fastcloud:clear-selection', clear)
    return () => window.removeEventListener('fastcloud:clear-selection', clear)
  }, [])
  const { data: pagePlaylist } = useQuery({ queryKey: ['playlists', 'mine'], queryFn: () => api.playlists('mine'), enabled: page === 'playlist', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: connection } = useQuery({ queryKey: ['connection'], queryFn: api.connection, enabled: page === 'playlist' })
  const editablePlaylistId = page === 'playlist' && playlistId !== null && pagePlaylist?.status === 'ready' && pagePlaylist.data.some(list => list.id === playlistId) && (connection?.status === 'signed_in' || (connection?.status === 'demo' && playlistId >= 9000)) ? playlistId : null
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const { data: likedTracks } = useQuery({ queryKey: ['tracks', 'likes', undefined, undefined], queryFn: () => api.tracks('likes'), enabled: connection?.status === 'signed_in', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: repostedTracks } = useQuery({ queryKey: ['tracks', 'reposts'], queryFn: () => api.tracks('reposts'), enabled: menu !== null, refetchInterval: result => dataRefreshInterval(result.state.data) })
  const likedIds = useMemo(() => new Set(likedTracks?.status === 'ready' ? likedTracks.data.map(item => item.id) : settings?.liked_ids || []), [likedTracks, settings?.liked_ids])
  const isLiked = (id: number) => likedIds.has(id)
  const play = async (index: number) => { try { if (playback?.id === tracks[index].id) await api.transport('play_pause'); else if (onPlay) await onPlay(index); else await api.play(tracks, index, recommended); await queryClient.invalidateQueries({ queryKey: ['player'] }) } catch (cause) { setError(String(cause)) } }
  const like = async (track: Track) => {
    const liked = !isLiked(track.id)
    try {
      await api.setLiked(track.id, liked)
      await queryClient.invalidateQueries({ queryKey: ['wave-dislikes'] })
      await queryClient.invalidateQueries({ queryKey: ['wave-disliked'] })
      await Promise.all([queryClient.invalidateQueries({ queryKey: ['settings'] }), queryClient.invalidateQueries({ queryKey: ['tracks', 'likes'] })])
    } catch (cause) { setError(String(cause)) }
  }
  const toggleRepost = async (track: Track) => {
    const active = !(repostedTracks?.status === 'ready' && repostedTracks.data.some(item => item.id === track.id))
    try { await api.repost('track', track.id, active); setMenu(null); await queryClient.invalidateQueries({ queryKey: ['tracks', 'reposts'] }) }
    catch (cause) { setError(String(cause)) }
  }
  const addToPlaylist = async (track: Track, playlistId?: number) => {
    try {
      if (playlistId) await api.addToPlaylist(playlistId, track.id)
      else {
        const created = await api.createPlaylist(newPlaylist.trim(), track.id)
        useApp.getState().openPlaylist(created.id, created.title)
      }
      setPlaylistMenu(null); setMenu(null); setNewPlaylist('')
      await queryClient.invalidateQueries({ queryKey: ['playlists'] })
      await queryClient.invalidateQueries({ queryKey: ['tracks', 'playlist'] })
    } catch (cause) { setError(String(cause)) }
  }
  const removeFromPlaylist = async (track: Track) => {
    if (!editablePlaylistId) return
    try {
      await api.removeFromPlaylist(editablePlaylistId, track.id)
      setMenu(null)
      await queryClient.invalidateQueries({ queryKey: ['playlists'] })
      await queryClient.invalidateQueries({ queryKey: ['tracks', 'playlist'] })
    } catch (cause) { setError(String(cause)) }
  }
  const moveInPlaylist = async (from: number, to: number) => {
    if (!editablePlaylistId) return
    try {
      await api.moveInPlaylist(editablePlaylistId, from, to)
      await queryClient.invalidateQueries({ queryKey: ['tracks', 'playlist', editablePlaylistId] })
    } catch (cause) { setError(String(cause)) }
  }
  const pick = (index: number, shift: boolean) => {
    setSelected(current => {
      const next = new Set(current)
      if (shift && selectionAnchor.current !== null) {
        const start = Math.min(selectionAnchor.current, index)
        const end = Math.max(selectionAnchor.current, index)
        for (let i = start; i <= end; i++) next.add(tracks[i].id)
      } else if (next.has(tracks[index].id)) next.delete(tracks[index].id)
      else next.add(tracks[index].id)
      return next
    })
    selectionAnchor.current = index
  }
  const pickedTracks = useMemo(() => tracks.filter(track => selected.has(track.id)), [tracks, selected])
  const rowHeight = interfaceRowHeight(settings)
  const virtual = useVirtualRows(tracks.length, rowHeight, tracks.length > 200, Number(pickedTracks.length > 0))
  useLayoutEffect(() => {
    if (activeIndex === undefined || activeIndex < 0 || activeIndex >= tracks.length) return
    const list = virtual.ref.current
    const scroller = list?.closest<HTMLElement>('.scroll-area')
    if (!list || !scroller) return
    if (tracks.length <= 200) {
      list.querySelectorAll('.track-row')[activeIndex]?.scrollIntoView({ block: 'nearest' })
      return
    }
    const listTop = list.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop
    const top = listTop + 1 + activeIndex * rowHeight
    if (top < scroller.scrollTop) scroller.scrollTop = top
    else if (top + rowHeight > scroller.scrollTop + scroller.clientHeight) scroller.scrollTop = top + rowHeight - scroller.clientHeight
  }, [activeIndex, tracks.length, virtual.ref, rowHeight])
  const bulk = async (action: 'play' | 'next' | 'queue' | 'like' | 'unlike' | 'playlist', targetId?: number) => {
    if (!pickedTracks.length) return
    setBulkBusy(true); setError('')
    try {
      if (action === 'play') await api.play(pickedTracks, 0, recommended)
      else if (action === 'next' || action === 'queue') await api.enqueueTracks(pickedTracks, action === 'next')
      else if (action === 'playlist' && targetId) {
        await api.addTracksToPlaylist(targetId, pickedTracks.map(track => track.id))
        await Promise.all([queryClient.invalidateQueries({ queryKey: ['playlists'] }), queryClient.invalidateQueries({ queryKey: ['tracks', 'playlist', targetId] })])
      } else if (action === 'like' || action === 'unlike') {
        for (const track of pickedTracks) if (isLiked(track.id) !== (action === 'like')) await api.setLiked(track.id, action === 'like')
        await Promise.all([queryClient.invalidateQueries({ queryKey: ['settings'] }), queryClient.invalidateQueries({ queryKey: ['tracks', 'likes'] })])
      }
      await queryClient.invalidateQueries({ queryKey: ['player'] })
      setSelected(new Set()); setBulkPlaylistOpen(false)
    } catch (cause) { setError(String(cause)) }
    finally { setBulkBusy(false) }
  }
  if (!tracks.length) return <Empty message={t('Здесь пока пусто', 'Nothing here yet')} detail={t('Попробуйте другой раздел или поисковый запрос.', 'Try another section or search term.')} />
  return <>{error && <p className="list-error error-text" role="alert">{error}</p>}{pickedTracks.length > 0 && <div className="selection-toolbar"><strong>{t('Выбрано', 'Selected')}: {pickedTracks.length}</strong><button disabled={bulkBusy} onClick={() => void bulk('play')}>{t('Слушать', 'Play')}</button><button disabled={bulkBusy} onClick={() => void bulk('next')}>{t('Следующими', 'Play next')}</button><button disabled={bulkBusy} onClick={() => void bulk('queue')}>{t('В очередь', 'Add to queue')}</button><button disabled={bulkBusy} onClick={() => void bulk(pickedTracks.every(track => isLiked(track.id)) ? 'unlike' : 'like')}>{pickedTracks.every(track => isLiked(track.id)) ? t('Убрать лайки', 'Unlike') : t('Лайкнуть', 'Like')}</button><button disabled={bulkBusy} onClick={() => setBulkPlaylistOpen(!bulkPlaylistOpen)}>{t('В плейлист', 'Add to playlist')}</button><button disabled={bulkBusy} onClick={() => setSelected(new Set())}>{t('Снять выбор', 'Clear selection')}</button>{bulkPlaylistOpen && <div className="selection-playlists">{ownPlaylists?.status === 'ready' ? ownPlaylists.data.filter(list => !(list.is_album || list.playlist_type === 'album' || list.set_type === 'album')).map(list => <button key={list.id} disabled={bulkBusy} onClick={() => void bulk('playlist', list.id)}>{list.title}</button>) : <span>{t('Загружаем плейлисты…', 'Loading playlists…')}</span>}</div>}</div>}<div ref={virtual.ref} className={`track-list ${compact ? 'compact' : ''} ${tracks.length > 200 ? 'virtual-track-list' : ''}`}>
    {virtual.start > 0 && <div className="virtual-spacer" style={{ height: virtual.start * rowHeight }} aria-hidden="true" />}
    {tracks.slice(virtual.start, virtual.end).map((track, offset) => { const index = virtual.start + offset; const current = playback?.id === track.id; return <div aria-current={current ? 'true' : undefined} className={`track-row ${current ? 'is-current' : ''} ${editablePlaylistId ? 'editable' : ''} ${selected.has(track.id) ? 'selected' : ''} ${activeIndex === index ? 'keyboard-active' : ''}`} key={track.id} onClick={event => { if ((event.target as HTMLElement).closest('button, a, input, select, textarea, [role="button"]')) return; if (event.ctrlKey || event.metaKey || event.shiftKey) pick(index, event.shiftKey); else void play(index) }}>
      <div className="track-main">
        <button className="track-cover-button" onClick={event => { if (event.ctrlKey || event.metaKey || event.shiftKey) pick(index, event.shiftKey); else void play(index) }} aria-label={`${current ? playback?.playing ? t('Пауза', 'Pause') : t('Продолжить', 'Resume') : t('Воспроизвести', 'Play')} ${track.title}`}>{current ? <span className={`track-playback-mark ${playback?.playing ? 'is-playing' : ''}`} title={playback?.playing ? t('Сейчас играет', 'Now playing') : t('На паузе', 'Paused')}>{playback?.loading ? <LoaderCircle size={14} className="spin" /> : playback?.playing ? <><i /><i /><i /></> : <Pause size={14} />}</span> : settings?.show_track_numbers !== false && <span className="track-num">{String(index + 1).padStart(2, '0')}</span>}{!compact && <Artwork item={track} />}</button>
        <span className="track-info"><button className="track-title-button" onClick={event => { if (event.ctrlKey || event.metaKey || event.shiftKey) pick(index, event.shiftKey); else void play(index) }}>{track.title}</button><ArtistCredits track={track} className="track-artist-button" english={english} />{listenerCounts?.[track.id] != null && <small className="community-listeners">{listenerLabel(listenerCounts[track.id],!!english)}</small>}</span>
      </div>
      <span className="track-genre">{track.genre || t('Трек', 'Track')}</span>
      <span className="track-duration">{duration(track.full_duration_ms || track.duration)}</span>
      <div className="track-actions">
        {editablePlaylistId && <div className="queue-order"><button className="icon-button" disabled={index === 0} aria-label={`${t('Поднять', 'Move up')} ${track.title}`} onClick={() => void moveInPlaylist(index, index - 1)}><ArrowUp size={14} /></button><button className="icon-button" disabled={index === tracks.length - 1} aria-label={`${t('Опустить', 'Move down')} ${track.title}`} onClick={() => void moveInPlaylist(index, index + 1)}><ArrowDown size={14} /></button></div>}
        <button className={`icon-button ${isLiked(track.id) ? 'on' : ''}`} aria-label={isLiked(track.id) ? `${t('Убрать лайк', 'Unlike')}: ${track.title}` : `${t('Нравится', 'Like')}: ${track.title}`} title={isLiked(track.id) ? t('Убрать из понравившегося', 'Remove from likes') : t('Добавить в понравившееся', 'Add to likes')} onClick={() => void like(track)}><Heart size={17} fill={isLiked(track.id) ? 'currentColor' : 'none'} /></button>
        <ShareButton item={track} english={english} />
        <button className="icon-button" aria-label={`${t('Действия с', 'Actions for')} ${track.title}`} onClick={() => setMenu(menu === track.id ? null : track.id)}><MoreHorizontal size={19} /></button>
        {menu === track.id && <div className="row-menu"><button onClick={() => { setMenu(null); void api.station([track], 'track').then(() => queryClient.invalidateQueries({ queryKey: ['player'] })).catch(() => setError(t('Не удалось запустить волну', 'Could not start radio'))) }}><Disc3 size={15} /> {t('Волна по треку', 'Track radio')}</button><button onClick={() => { useApp.getState().openTrack(track.id, track.title); setMenu(null) }}><Music2 size={15} /> {t('Открыть трек', 'Open track')}</button><button onClick={() => { void api.enqueue(track); setMenu(null); void queryClient.invalidateQueries({ queryKey: ['player'] }) }}><Plus size={15} /> {t('В конец очереди', 'Add to queue')}</button><button onClick={() => { void api.enqueue(track, true); setMenu(null); void queryClient.invalidateQueries({ queryKey: ['player'] }) }}><ListMusic size={15} /> {t('Следующим', 'Play next')}</button><button onClick={() => void toggleRepost(track)} disabled={repostedTracks?.status !== 'ready'}><Repeat2 size={15} /> {repostedTracks?.status === 'ready' && repostedTracks.data.some(item => item.id === track.id) ? t('Убрать репост', 'Remove repost') : t('Репост', 'Repost')}</button><button onClick={() => setPlaylistMenu(playlistMenu === track.id ? null : track.id)}><Plus size={15} /> {t('Добавить в плейлист', 'Add to playlist')}</button>{playlistMenu === track.id && <div className="menu-subsection">{ownPlaylists?.status === 'ready' && ownPlaylists.data.map(list => <button key={list.id} onClick={() => void addToPlaylist(track, list.id)}>{list.title}</button>)}<form onSubmit={event => { event.preventDefault(); if (newPlaylist.trim()) void addToPlaylist(track) }}><input aria-label={t('Новый плейлист', 'New playlist')} placeholder={t('Новый плейлист', 'New playlist')} value={newPlaylist} onChange={event => setNewPlaylist(event.target.value)} /><button type="submit" disabled={!newPlaylist.trim()}>{t('Создать', 'Create')}</button></form></div>}{editablePlaylistId && <button onClick={() => void removeFromPlaylist(track)}><X size={15} /> {t('Удалить из плейлиста', 'Remove from playlist')}</button>}</div>}
      </div>
    </div> })}
    {virtual.end < tracks.length && <div className="virtual-spacer" style={{ height: (tracks.length - virtual.end) * rowHeight }} aria-hidden="true" />}
  </div></>
}

function VirtualCardGrid<T extends { id: number }>({ items, className = '', renderCard }: { items: T[]; className?: string; renderCard: (item: T, index: number) => ReactNode }) {
  const [layout, setLayout] = useState({ columns: 5, height: 250, gap: 15 })
  const rowCount = Math.ceil(items.length / layout.columns)
  const virtual = useVirtualRows(rowCount, layout.height + layout.gap, true, layout.columns)
  useLayoutEffect(() => {
    const grid = virtual.ref.current
    if (!grid) return
    const measure = () => {
      const style = getComputedStyle(grid)
      const columns = style.gridTemplateColumns.split(' ').length
      const height = grid.querySelector('.card')?.getBoundingClientRect().height
      const gap = Number.parseFloat(style.rowGap) || 0
      if (!height || !columns) return
      setLayout(current => current.columns === columns && current.height === height && current.gap === gap ? current : { columns, height, gap })
    }
    const observer = new ResizeObserver(measure)
    observer.observe(grid)
    measure()
    return () => observer.disconnect()
  }, [virtual.ref])
  const start = virtual.start * layout.columns
  const end = Math.min(items.length, virtual.end * layout.columns)
  const step = layout.height + layout.gap
  return <div ref={virtual.ref} className={`cards ${className} virtual-card-grid`}>
    {virtual.start > 0 && <div className="virtual-grid-spacer" style={{ height: virtual.start * step - layout.gap }} aria-hidden="true" />}
    {items.slice(start, end).map((item, offset) => <Fragment key={item.id}>{renderCard(item, start + offset)}</Fragment>)}
    {virtual.end < rowCount && <div className="virtual-grid-spacer" style={{ height: (rowCount - virtual.end) * step - layout.gap }} aria-hidden="true" />}
  </div>
}

export function PlaylistCards({ playlists }: { playlists: Playlist[] }) {
  const open = useApp(s => s.openPlaylist)
  const english = useEnglish()
  if (!playlists.length) return <Empty message={english ? 'No playlists yet' : 'Плейлистов пока нет'} />
  const renderCard = (list: Playlist) => <article className="card share-card"><button className="card-open" onClick={() => open(list.id, list.title)}>
    <Artwork item={list} size="card" /><span className="card-kind">{list.is_album || list.playlist_type?.toLowerCase() === 'album' || list.set_type?.toLowerCase() === 'album' ? english ? 'ALBUM' : 'АЛЬБОМ' : english ? 'PLAYLIST' : 'ПЛЕЙЛИСТ'}</span><strong>{list.title}</strong><small>{list.user?.username ? `${list.user.username} · ` : ''}{list.track_count || 0} {english ? 'tracks' : 'треков'}</small>
  </button><ShareButton item={list} kind="playlist" english={english} className="card-share" /></article>
  return playlists.length > 200 ? <VirtualCardGrid items={playlists} renderCard={renderCard} /> : <div className="cards">{playlists.map(list => <Fragment key={list.id}>{renderCard(list)}</Fragment>)}</div>
}

export type LibraryView = 'grid' | 'list'
export function LibraryTracks({ tracks, filter, view }: { tracks: Track[]; filter: string; view: LibraryView }) {
  const queryClient = useQueryClient()
  const english = useEnglish()
  const search = filter.trim().toLocaleLowerCase()
  const visible = useMemo(() => search ? tracks.filter(track => [track.title, artist(track), track.genre || ''].some(value => value.toLocaleLowerCase().includes(search))) : tracks, [tracks, search])
  if (!visible.length) return <Empty message={search ? english ? 'Nothing found' : 'Ничего не найдено' : english ? 'Nothing here yet' : 'Здесь пока пусто'} />
  if (view === 'list') return <TrackRows tracks={visible} />
  const renderCard = (track: Track, index: number) => <article className="card share-card"><button className="card-open" onClick={() => void api.play(visible, index).then(() => queryClient.invalidateQueries({ queryKey: ['player'] }))}><Artwork item={track} size="card" /><strong>{track.title}</strong><small>{artist(track)}</small></button><ShareButton item={track} english={english} className="card-share" /></article>
  return visible.length > 200 ? <VirtualCardGrid items={visible} className="library-grid" renderCard={renderCard} /> : <div className="cards library-grid">{visible.map((track, index) => <Fragment key={track.id}>{renderCard(track, index)}</Fragment>)}</div>
}

function PlaylistListRow({ list, english }: { list: Playlist; english: boolean }) {
  return <article className="library-playlist-row"><button className="playlist-list-open" onClick={() => useApp.getState().openPlaylist(list.id, list.title)}><Artwork item={list} /><span><strong>{list.title}</strong><small>{list.user?.username || ''} · {list.track_count || 0} {english ? 'tracks' : 'треков'}</small></span><ChevronRight size={16} /></button><ShareButton item={list} kind="playlist" english={english} /></article>
}

function VirtualPlaylistList({ playlists, english }: { playlists: Playlist[]; english: boolean }) {
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const rowHeight = 60 * interfaceLayoutScale(settings)
  const step = rowHeight + 5
  const virtual = useVirtualRows(playlists.length, step, true)
  return <div ref={virtual.ref} className="library-playlist-list virtual-library-playlist-list">
    {virtual.start > 0 && <div className="virtual-spacer" style={{ height: virtual.start * step - 5 }} aria-hidden="true" />}
    {playlists.slice(virtual.start, virtual.end).map(list => <PlaylistListRow key={list.id} list={list} english={english} />)}
    {virtual.end < playlists.length && <div className="virtual-spacer" style={{ height: (playlists.length - virtual.end) * step - 5 }} aria-hidden="true" />}
  </div>
}

export function LibraryPlaylists({ playlists, filter, view }: { playlists: Playlist[]; filter: string; view: LibraryView }) {
  const english = useEnglish()
  const search = filter.trim().toLocaleLowerCase()
  const visible = useMemo(() => search ? playlists.filter(list => [list.title, list.user?.username || ''].some(value => value.toLocaleLowerCase().includes(search))) : playlists, [playlists, search])
  if (!visible.length) return <Empty message={search ? english ? 'Nothing found' : 'Ничего не найдено' : english ? 'No playlists yet' : 'Плейлистов пока нет'} />
  if (view === 'grid') return <PlaylistCards playlists={visible} />
  if (visible.length > 200) return <VirtualPlaylistList playlists={visible} english={english} />
  return <div className="library-playlist-list">{visible.map(list => <PlaylistListRow key={list.id} list={list} english={english} />)}</div>
}

function HomePage() {
  const queryClient = useQueryClient()
  const { data } = useTracks('discover')
  const { data: likes } = useTracks('likes')
  const { data: history } = useTracks('history')
  const { data: fresh } = useTracks('following')
  const { data: playlists } = useQuery({ queryKey: ['playlists', 'liked'], queryFn: () => api.playlists('liked'), refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: ownPlaylists } = useQuery({ queryKey: ['playlists', 'mine'], queryFn: () => api.playlists('mine'), refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const likedTracks = likes?.status === 'ready' ? likes.data : []
  const historyTracks = history?.status === 'ready' ? history.data : []
  const freshTracks = fresh?.status === 'ready' ? fresh.data : []
  const quickPicks = (settings?.quick_access || []).flatMap(item => {
    const target = quickAccessTarget(item)
    return target ? [target] : []
  })
  const [waveTracks, setWaveTracks] = useState<Track[]>([])
  const { data: wavePlayer } = useQuery({ queryKey: ['player'], queryFn: api.player })
  const displayedWave = wavePlayer?.waveActive
    ? wavePlayer.queue.slice(wavePlayer.current ?? 0, (wavePlayer.current ?? 0) + 6)
    : waveTracks.slice(0, 6)
  const [waveBusy, setWaveBusy] = useState(false)
  const [waveError, setWaveError] = useState('')
  const waveVariation = useRef(Math.floor(Date.now() / 60_000))
  const startWave = async () => {
    if (!likedTracks.length || waveBusy) return
    setWaveBusy(true); setWaveError('')
    try {
      const tracks = await api.myWave(likedTracks, waveVariation.current++)
      setWaveTracks(tracks)
      if (api.preview) await api.play(tracks, 0)
      await queryClient.invalidateQueries({ queryKey: ['player'] })
    } catch (cause) { setWaveError(String(cause)) }
    finally { setWaveBusy(false) }
  }
  const openLibrary = (tab: 'tracks' | 'playlists' | 'albums' | 'history') => { if (tab === 'history') { useApp.getState().setPage('history'); return }; useApp.getState().setLibraryTab(tab); useApp.getState().setPage('library') }
  return <div className="page-content">
    <section className="quick-picks" aria-label={english ? 'Quick picks' : 'Быстрый выбор'}><div className="quick-picks-heading"><h2>{english ? 'Quick picks' : 'Быстрый выбор'}</h2><p>{english ? 'Your pinned music, in the same order as the sidebar.' : 'Твои закрепления — в том же порядке, что и в сайдбаре.'}</p></div>{quickPicks.length ? <div className="quick-picks-grid">{quickPicks.map(target => <button key={`${target.kind}-${target.id}`} className="quick-pick" onClick={() => target.kind === 'track' ? useApp.getState().openTrack(target.id, target.title) : useApp.getState().openPlaylist(target.id, target.title)}><Artwork item={{ id: target.id, title: target.title, artwork_url: target.artwork_url }} /><span><strong>{target.title}</strong><small>{target.artist} · {target.kind === 'track' ? english ? 'Song' : 'Песня' : target.kind === 'album' ? english ? 'Album' : 'Альбом' : english ? 'Playlist' : 'Плейлист'}</small></span><ChevronRight size={17} /></button>)}</div> : <p className="quick-picks-empty">{english ? 'Pin a song, album or playlist to keep it here.' : 'Закрепи песню, альбом или плейлист — они появятся здесь.'}</p>}</section>
    <div className="my-wave"><div className="my-wave-copy"><span className="page-kicker">VEIRUNE / {english ? 'FOR YOU' : 'ДЛЯ ТЕБЯ'}</span><WaveGreeting english={!!english} /><p>{english ? 'Learns from your likes, listening, skips and the time of day. The queue keeps playing.' : 'Учитывает лайки, прослушивания, пропуски и время суток. Очередь пополняется сама.'}</p>{settings && <WaveModes settings={settings} english={!!english} />}{wavePlayer?.waveActive && <small>{english ? 'Playing your wave' : 'Сейчас играет твоя волна'}</small>}</div><button className="primary-button" disabled={waveBusy || !likedTracks.length} onClick={() => void startWave()}>{waveBusy ? <LoaderCircle className="spin" size={17} /> : <Play size={17} fill="currentColor" />}<span>{waveBusy ? english ? 'Finding tracks…' : 'Подбираем треки…' : wavePlayer?.waveActive || waveTracks.length ? english ? 'New wave' : 'Новая волна' : english ? 'Start My Wave' : 'Включить мою волну'}</span></button></div>
    {!likedTracks.length && likes?.status === 'ready' && <p className="muted">{english ? 'Like a few tracks to start your wave.' : 'Лайкни несколько треков, чтобы запустить волну.'}</p>}
    {waveError && <p className="error-text" role="alert">{waveError}</p>}
    {displayedWave.length > 0 && <TrackRows tracks={displayedWave} onPlay={index => wavePlayer?.waveActive
      ? api.transport('skip_to', undefined, (wavePlayer.current ?? 0) + index)
      : api.play(waveTracks, index, true)} />}
    {settings?.show_community !== false && <CommunityHomeSection english={!!english} />}
    {historyTracks.length > 0 && <><SectionTitle title={english ? 'Continue listening' : 'Продолжить слушать'} subtitle={english ? 'Recently played' : 'Недавно прослушанное'} action={english ? 'History' : 'Вся история'} onAction={() => openLibrary('history')} /><TrackRows tracks={historyTracks.slice(0, 6)} /></>}
    {freshTracks.length > 0 && <><SectionTitle title={english ? 'From your artists' : 'Новое от подписок'} subtitle={english ? 'Tracks from artists you follow' : 'Треки авторов, на которых ты подписан'} /><TrackRows tracks={freshTracks.slice(0, 6)} /></>}
    <SectionTitle title={english ? 'On rotation' : 'Сейчас в эфире'} subtitle={english ? 'A good place to start' : 'Подборка для хорошего начала'} />
    <Status value={data}>{tracks => <TrackRows tracks={tracks.slice(0, 6)} recommended />}</Status>
    {likedTracks.length > 0 && <><SectionTitle title={english ? 'Your likes' : 'Твои лайки'} action={english ? 'All likes' : 'Все лайки'} onAction={() => openLibrary('tracks')} /><TrackRows tracks={likedTracks.slice(0, 6)} /></>}
    {(playlists?.status !== 'ready' || playlists.data.some(item => !(item.is_album || item.playlist_type === 'album' || item.set_type === 'album'))) && <><SectionTitle title={english ? 'Saved playlists' : 'Сохранённые плейлисты'} subtitle={english ? 'All in one place' : 'Собранное в одном месте'} action={english ? 'Open library' : 'Все плейлисты'} onAction={() => openLibrary('playlists')} /><Status value={playlists}>{items => <PlaylistCards playlists={items.filter(item => !(item.is_album || item.playlist_type === 'album' || item.set_type === 'album')).slice(0, 5)} />}</Status></>}
    {ownPlaylists?.status === 'ready' && ownPlaylists.data.some(item => !(item.is_album || item.playlist_type === 'album' || item.set_type === 'album')) && <><SectionTitle title={english ? 'Created by you' : 'Создано тобой'} action={english ? 'All playlists' : 'Все плейлисты'} onAction={() => openLibrary('playlists')} /><PlaylistCards playlists={ownPlaylists.data.filter(item => !(item.is_album || item.playlist_type === 'album' || item.set_type === 'album')).slice(0, 5)} /></>}
    {ownPlaylists?.status === 'ready' && ownPlaylists.data.some(item => item.is_album || item.playlist_type === 'album' || item.set_type === 'album') && <><SectionTitle title={english ? 'Your albums' : 'Твои альбомы'} action={english ? 'All albums' : 'Все альбомы'} onAction={() => openLibrary('albums')} /><PlaylistCards playlists={ownPlaylists.data.filter(item => item.is_album || item.playlist_type === 'album' || item.set_type === 'album').slice(0, 5)} /></>}
    {playlists?.status === 'ready' && playlists.data.some(item => item.is_album || item.playlist_type === 'album' || item.set_type === 'album') && <><SectionTitle title={english ? 'Saved albums' : 'Сохранённые альбомы'} action={english ? 'All albums' : 'Все альбомы'} onAction={() => openLibrary('albums')} /><PlaylistCards playlists={playlists.data.filter(item => item.is_album || item.playlist_type === 'album' || item.set_type === 'album').slice(0, 5)} /></>}
  </div>
}

export function SectionTitle({ title, subtitle, action, onAction }: { title: string; subtitle?: string; action?: string; onAction?: () => void }) {
  return <div className="section-title"><div><h2>{title}</h2>{subtitle && <p>{subtitle}</p>}</div>{action && <button className="text-button" onClick={onAction}>{action}<ArrowRight size={16} /></button>}</div>
}

function LibraryPage() {
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const tab = useApp(s => s.libraryTab)
  const [name, setName] = useState('')
  const [error, setError] = useState('')
  const [filter, setFilter] = useState('')
  const [view, setView] = useState<LibraryView>('list')
  const queryClient = useQueryClient()
  const { data: tracks } = useTracks('likes', undefined, undefined, tab === 'tracks')
  const { data: likedLists } = useQuery({ queryKey: ['playlists', 'liked'], queryFn: () => api.playlists('liked'), enabled: tab === 'playlists', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: lists } = useQuery({ queryKey: ['playlists', 'mine'], queryFn: () => api.playlists('mine'), enabled: tab === 'playlists', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: followed } = useQuery({ queryKey: ['following'], queryFn: api.following, enabled: tab === 'artists', refetchInterval: result => dataRefreshInterval(result.state.data) })
  useEffect(() => {
    if (tab !== 'artists') return
    void api.refreshFollowing().then(() => queryClient.invalidateQueries({ queryKey: ['following'] })).catch(cause => setError(String(cause)))
  }, [tab, queryClient])
  const create = async () => {
    try { const item = await api.createPlaylist(name); setName(''); await queryClient.invalidateQueries({ queryKey: ['playlists'] }); useApp.getState().openPlaylist(item.id, item.title) }
    catch (cause) { setError(String(cause)) }
  }
  const { data: collectionPlayer } = useQuery({ queryKey: ['player'], queryFn: api.player,
    select: state => ({ id: state.current == null ? undefined : state.queue[state.current]?.id, playing: state.isPlaying }) })
  const songs = tracks?.status === 'ready' ? tracks.data : []
  const playingSongs = songs.some(track => track.id === collectionPlayer?.id)
  const playSongs = async (shuffled = false) => {
    if (!songs.length) return
    try {
      if (!shuffled && playingSongs) await api.transport('play_pause')
      else await api.play(shuffled ? shuffleTracks(songs) : songs, 0)
      await queryClient.invalidateQueries({ queryKey: ['player'] })
    } catch (cause) { setError(String(cause)) }
  }
  const labels = { dislikes: t('Дизлайки', 'Dislikes'), overview: t('Библиотека', 'Library'), tracks: t('Песни', 'Songs'), playlists: t('Плейлисты', 'Playlists'), liked_playlists: t('Любимые плейлисты', 'Liked playlists'), albums: t('Альбомы', 'Albums'), artists: t('Исполнители', 'Artists'), stations: t('Станции', 'Stations'), uploads: t('Мои загрузки', 'My uploads'), history: t('История', 'History') }
  useEffect(() => { setFilter(''); setView(tab === 'tracks' || tab === 'history' || tab === 'uploads' ? 'list' : 'grid') }, [tab])
  return <div className="page-content library-section">{tab === 'tracks' ? <div className="library-songs-hero"><span className="library-songs-cover"><Heart size={54} fill="currentColor" strokeWidth={1.5} /></span><div><small>{t('ТВОЯ КОЛЛЕКЦИЯ', 'YOUR COLLECTION')}</small><h1>{t('Любимые песни', 'Favorite songs')}</h1><p>{songs.length} {t('треков', 'songs')} · {duration(songs.reduce((total, track) => total + (track.full_duration_ms || track.duration || 0), 0))}</p><div className="library-songs-actions"><button className="primary-button" disabled={!songs.length} onClick={() => void playSongs()}>{playingSongs && collectionPlayer?.playing ? <Pause size={16} /> : <Play size={16} />}{playingSongs ? collectionPlayer?.playing ? t('Пауза', 'Pause') : t('Продолжить', 'Resume') : t('Слушать', 'Play')}</button><button className="secondary-button" disabled={!songs.length} title={t('Перемешать всё', 'Shuffle all')} aria-label={t('Перемешать все любимые песни', 'Shuffle all favorite songs')} onClick={() => void playSongs(true)}><Shuffle size={16} /></button></div></div></div> : <div className="library-section-heading"><span>{t('Твоя библиотека', 'Your Library')}</span><h1>{labels[tab]}</h1></div>}{tab !== 'overview' && tab !== 'stations' && <div className="library-toolbar"><input aria-label={t('Фильтр медиатеки', 'Filter library')} placeholder={t('Найти в этом разделе…', 'Find in this section…')} value={filter} onChange={event => setFilter(event.target.value)} />{tab !== 'dislikes' && <div role="group" aria-label={t('Вид медиатеки', 'Library view')}><button className={view === 'list' ? 'active' : ''} onClick={() => setView('list')}>{t('Список', 'List')}</button><button className={view === 'grid' ? 'active' : ''} onClick={() => setView('grid')}>{t('Сетка', 'Grid')}</button></div>}</div>}{tab === 'overview' ? <LibraryOverview /> : tab === 'tracks' ? <Status value={tracks}>{items => <LibraryTracks tracks={items} filter={filter} view={view} />}</Status> : tab === 'playlists' ? <><form className="playlist-create" onSubmit={event => { event.preventDefault(); void create() }}><input aria-label={t('Название нового плейлиста', 'New playlist name')} placeholder={t('Название нового плейлиста', 'New playlist name')} value={name} onChange={event => setName(event.target.value)} /><button className="secondary-button" disabled={!name.trim()} type="submit"><Plus size={16} /> {t('Создать', 'Create')}</button></form>{error && <p className="error-text">{error}</p>}<Status value={libraryCollections([likedLists, lists], false)}>{items => <LibraryPlaylists playlists={items} filter={filter} view={view} />}</Status></> : tab === 'dislikes' ? <DislikedTracks filter={filter} /> : tab === 'artists' ? <Status value={followed}>{items => { const visible = items.filter(user => user.username.toLocaleLowerCase().includes(filter.trim().toLocaleLowerCase())); return visible.length ? <div className="artist-grid">{visible.map(user => <button className="artist-card" key={user.id} onClick={() => useApp.getState().openArtist(user.id, user.username)}>{user.avatar_url ? <RemoteImage className="artist-avatar" src={user.avatar_url} pixels={160} alt="" loading="lazy" /> : <span className="artist-avatar">{user.username.slice(0, 1).toUpperCase()}</span>}<strong>{user.username}</strong><small>{user.followers_count} {t('подписчиков', 'followers')}</small></button>)}</div> : <Empty message={filter ? t('Ничего не найдено', 'Nothing found') : t('Подписок пока нет', 'No followed artists yet')} /> }}</Status> : <LibraryExtra tab={tab} filter={filter} view={view} />}</div>
}

const genres = ['Electronic', 'Hip-hop & Rap', 'Alternative Rock', 'Ambient'] as const
function DiscoverPage() {
  const english = useEnglish()
  const t = (ru: string, en: string) => english ? en : ru
  const [genre, setGenre] = useState<(typeof genres)[number]>('Electronic')
  const { data } = useTracks('genre', genre)
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  return <div className="page-content"><div className="page-intro"><span className="page-kicker">VEIRUNE / {t('ОБЗОР', 'DISCOVER')}</span><h1>{t('Найди новое звучание', 'Find a new sound')}</h1><p>{t('Выбери настроение и слушай треки из каталога SoundCloud.', 'Choose a mood and discover music on SoundCloud.')}</p></div><div className="genre-tabs" role="group" aria-label={t('Жанры', 'Genres')}>{genres.map(item => <button key={item} className={genre === item ? 'active' : ''} onClick={() => setGenre(item)}>{item}</button>)}</div><SectionTitle title={genre} subtitle={t('Треки этого жанра', 'Tracks in this genre')} /><Status value={data}>{items => <TrackRows tracks={items} compact={settings?.compact_rows} />}</Status></div>
}

function FeedPage() {
  const english = useEnglish()
  const t = (ru: string, en: string) => english ? en : ru
  const [showReposts, setShowReposts] = useState(true)
  const { data: tracks } = useTracks('feed')
  const { data: playlists } = useQuery({ queryKey: ['playlists', 'feed'], queryFn: () => api.playlists('feed'), refetchInterval: result => result.state.data?.status === 'loading' ? 900 : 30000 })
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  return <div className="page-content"><div className="page-intro"><span className="page-kicker">VEIRUNE / {t('ЛЕНТА', 'FEED')}</span><h1>{t('Лента', 'Feed')}</h1><p>{t('Свежие публикации авторов, на которых ты подписан.', 'New posts from artists you follow.')}</p><label className="setting-row feed-reposts"><span>{t('Показывать репосты', 'Show reposts')}</span><input type="checkbox" checked={showReposts} onChange={event => setShowReposts(event.target.checked)} /></label></div><Status value={tracks}>{items => <TrackRows tracks={showReposts ? items : items.filter(item => !item.feed_reposted)} compact={settings?.compact_rows} />}</Status><SectionTitle title={t('Плейлисты в ленте', 'Playlists in feed')} /><Status value={playlists}>{items => <PlaylistCards playlists={showReposts ? items : items.filter(item => !item.feed_reposted)} />}</Status></div>
}

function InboxPage({ openLink }: { openLink: (raw: string) => Promise<void> }) {
  const english = useEnglish()
  const t = (ru: string, en: string) => english ? en : ru
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const entries = [...(settings?.inbox || [])].reverse()
  return <div className="page-content"><div className="page-intro"><span className="page-kicker">VEIRUNE / {t('ВХОДЯЩИЕ', 'LINKS')}</span><h1>{t('Открытые ссылки', 'Opened links')}</h1><p>{t('Ссылки SoundCloud, которые ты открывал в Veirune, хранятся локально.', 'SoundCloud links opened in Veirune are stored locally.')}</p></div>{entries.length ? <div className="inbox-list">{entries.map((item, index) => <button key={`${item.link}-${index}`} onClick={() => void openLink(item.link)}><span><strong>{item.label}</strong><small>{item.link}</small></span><time>{new Date(item.at * 1000).toLocaleString(english ? 'en-US' : 'ru-RU')}</time><ArrowRight size={16} /></button>)}</div> : <Empty message={t('Ссылок пока нет', 'No links yet')} detail={t('Вставь ссылку SoundCloud в поиск или открой ссылку fastcloud:.', 'Paste a SoundCloud link in search or open a fastcloud: link.')} />}</div>
}

function LyricSearchResults({ matches, english }: { matches: LyricTrackMatch[]; english: boolean }) {
  const queryClient = useQueryClient()
  const [error, setError] = useState('')
  const play = async (index: number) => {
    try {
      setError('')
      await api.play(matches.map(match => match.track), index)
      await queryClient.invalidateQueries({ queryKey: ['player'] })
    } catch (cause) { setError(String(cause)) }
  }
  return <div className="lyric-search-results">
    {error && <p className="error-text" role="alert">{error}</p>}
    {matches.map(({ track, excerpt }, index) => <article className="lyric-search-result" key={track.id}>
      <button className="lyric-search-cover" aria-label={`${english ? 'Play' : 'Слушать'} ${track.title}`} onClick={() => void play(index)}><Artwork item={track} /><Play size={18} fill="currentColor" /></button>
      <div className="lyric-search-copy"><button className="lyric-search-title" onClick={() => useApp.getState().openTrack(track.id, track.title)}>{track.title}</button><small>{artist(track)}</small><p>«{excerpt}»</p></div>
      <span className="lyric-search-duration">{duration(track.full_duration_ms || track.duration)}</span>
    </article>)}
  </div>
}

function SearchPage({ query }: { query: string }) {
  const english = useEnglish()
  const t = (ru: string, en: string) => english ? en : ru
  const searchKind = useApp(state => state.searchKind)
  const artistLookup = useApp(state => state.artistLookup)
  const [tab, setTab] = useState<'tracks' | 'vibe' | 'lyrics' | 'playlists' | 'albums' | 'artists'>(searchKind)
  const [settledQuery, setSettledQuery] = useState(query.trim())
  useEffect(() => { const timer = window.setTimeout(() => setSettledQuery(query.trim()), 450); return () => window.clearTimeout(timer) }, [query])
  const waitingForInput = query.trim() !== settledQuery
  const [shuffleSeed] = useState(() => Math.floor(Math.random() * 0x100000000))
  const [discoveryDay, setDiscoveryDay] = useState(() => Math.floor(Date.now() / 86400000))
  useEffect(() => { const timer = window.setInterval(() => setDiscoveryDay(Math.floor(Date.now() / 86400000)), 60_000); return () => window.clearInterval(timer) }, [])
  const [selectedIndex, setSelectedIndex] = useState(0)
  const [keyError, setKeyError] = useState('')
  const queryClient = useQueryClient()
  const openArtist = useApp(s => s.openArtist)
  const searching = !!query.trim()
  const genreSearch = musicGenres.some(genre => genre.toLocaleLowerCase() === query.trim().toLocaleLowerCase())
  const { data: tracks } = useTracks(genreSearch ? 'genre' : 'search', query, undefined, searching && tab === 'tracks')
  const { data: likes } = useTracks('likes', undefined, undefined, !searching)
  const { data: discover } = useTracks('discover', undefined, undefined, !searching)
  const { data: recent } = useTracks('history', undefined, undefined, !searching)
  const likedTracks = likes?.status === 'ready' ? likes.data : []
  const dailyGenres = useMemo(() => favouriteGenres(likedTracks, musicGenres, discoveryDay), [likedTracks, discoveryDay])
  const { data: dailyPicks } = useQuery({ queryKey: ['search-daily-picks', discoveryDay, ...dailyGenres], queryFn: async () => {
    const responses = await Promise.allSettled(dailyGenres.map(genre => api.tracks('recommended_genre', genre)))
    return responses.flatMap((result, index) => result.status === 'fulfilled' && result.value.status === 'ready' ? result.value.data.filter(track => sameGenre(track.genre, dailyGenres[index])) : [])
  }, enabled: !searching && likes?.status === 'ready', staleTime: 6 * 60 * 60_000 })
  const suggested = useMemo(() => {
    const pool = [...new Map([...(dailyPicks || []), ...(discover?.status === 'ready' ? discover.data : [])].map(track => [track.id, track])).values()]
    return searchRecommendations(likedTracks, pool, recent?.status === 'ready' ? recent.data : [], shuffleSeed)
  }, [discover, dailyPicks, likedTracks, recent, shuffleSeed])
  const resultTracks = tracks?.status === 'ready' ? genreSearch ? tracks.data.filter(track => sameGenre(track.genre, query)) : tracks.data : []
  useEffect(() => setTab(searchKind), [searchKind])
  useEffect(() => setSelectedIndex(0), [query])
  useEffect(() => {
    if (tab !== 'tracks' || !resultTracks.length) return
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null
      if (event.ctrlKey || event.metaKey || event.altKey || target instanceof HTMLTextAreaElement || target?.isContentEditable) return
      if (target instanceof HTMLInputElement && !['Поиск', 'Search'].includes(target.getAttribute('aria-label') || '')) return
      if (target instanceof HTMLButtonElement) return
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault()
        setSelectedIndex(index => (index + (event.key === 'ArrowDown' ? 1 : resultTracks.length - 1)) % resultTracks.length)
      } else if (event.key === 'Enter') {
        if (/^(https?:\/\/|soundcloud:|fastcloud:|(?:on\.)?soundcloud\.com\/|\d+$)/i.test(query.trim())) return
        event.preventDefault()
        void api.play(resultTracks, Math.min(selectedIndex, resultTracks.length - 1)).then(() => queryClient.invalidateQueries({ queryKey: ['player'] })).catch(cause => setKeyError(String(cause)))
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [tab, resultTracks, selectedIndex, queryClient])
  const { data: lists } = useQuery({ queryKey: ['playlists', 'search', query], queryFn: () => api.playlists('search', query), enabled: !!query, refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: users } = useQuery({ queryKey: ['users', query], queryFn: () => api.users(query), enabled: !!query, refetchInterval: result => dataRefreshInterval(result.state.data) })
  useEffect(() => {
    if (!artistLookup || query !== artistLookup || users?.status !== 'ready') return
    const match = users.data.find(user => sameArtistName(user.username, artistLookup) || (!!user.full_name && sameArtistName(user.full_name, artistLookup)))
    useApp.getState().clearArtistLookup()
    if (match) openArtist(match.id, match.username)
  }, [artistLookup, query, users, openArtist])
  const { data: vibe, isLoading: vibeLoading, error: vibeError } = useQuery({ queryKey: ['vibe', settledQuery], queryFn: () => api.vibeSearch(settledQuery), enabled: tab === 'vibe' && !waitingForInput && settledQuery.length >= 2, staleTime: 5 * 60_000, retry: false })
  const { data: lyricMatches, isLoading: lyricsLoading, error: lyricsError, refetch: retryLyrics } = useQuery({ queryKey: ['lyric-tracks', settledQuery], queryFn: () => api.lyricTracks(settledQuery), enabled: tab === 'lyrics' && !waitingForInput && settledQuery.length >= 4 && settledQuery.length <= 200, staleTime: 5 * 60_000, retry: false })
  if (!searching) return <div className="page-content search-page"><GenreCarousel onSelect={genre => useApp.getState().setSearch(genre)} english={english} /><div className="search-welcome"><span className="page-kicker">VEIRUNE / {t('ПОИСК', 'SEARCH')}</span><h1>{t('С чего начнём?', 'What are we listening to?')}</h1><p>{t('Ищи трек, автора, настроение или строчку песни. А пока — музыка, которая может тебе понравиться.', 'Find a track, artist, mood or lyric. Here is something for you right now.')}</p></div><SectionTitle title={likedTracks.length ? t('На основе твоего вкуса', 'For your taste') : t('С чего начать', 'Start listening')} subtitle={likedTracks.length ? t('Жанры и авторы из твоих лайков', 'Genres and artists from your likes') : t('Треки из каталога', 'Tracks from the catalog')} />{suggested.length ? <TrackRows tracks={suggested} recommended /> : <Status value={discover}>{() => <Empty message={t('Пока нет новых рекомендаций', 'No new recommendations yet')} detail={t('Попробуй открыть поиск позже.', 'Try opening search again later.')} />}</Status>}{recent?.status === 'ready' && recent.data.length > 0 && <><SectionTitle title={t('Недавно слушал', 'Recently played')} /><TrackRows tracks={recent.data.slice(0, 6)} /></>}</div>
  return <div className="page-content search-page"><GenreCarousel selected={query} onSelect={genre => useApp.getState().setSearch(genre)} english={english} /><SectionTitle title={`${t('Результаты', 'Results')}: “${query}”`} subtitle={t('Поиск по SoundCloud', 'Search SoundCloud')} />{keyError && <p className="error-text" role="alert">{keyError}</p>}<div className="tabs"><button className={tab === 'tracks' ? 'active' : ''} onClick={() => setTab('tracks')}>{t('Треки', 'Tracks')}</button><button className={tab === 'vibe' ? 'active' : ''} onClick={() => setTab('vibe')}>{t('По настроению', 'By mood')}</button><button className={tab === 'lyrics' ? 'active' : ''} onClick={() => setTab('lyrics')}>{t('По тексту', 'By lyrics')}</button><button className={tab === 'playlists' ? 'active' : ''} onClick={() => setTab('playlists')}>{t('Плейлисты', 'Playlists')}</button><button className={tab === 'albums' ? 'active' : ''} onClick={() => setTab('albums')}>{t('Альбомы', 'Albums')}</button><button className={tab === 'artists' ? 'active' : ''} onClick={() => setTab('artists')}>{t('Авторы', 'Artists')}</button></div>
    {tab === 'tracks' && <div className="search-results"><Status value={tracks}>{() => <TrackRows tracks={resultTracks} activeIndex={selectedIndex} />}</Status></div>}
    {tab === 'vibe' && <>
      <div className="mood-examples" aria-label={t('Примеры настроений', 'Mood examples')}>{[['Грустное', 'Sad'], ['Весёлое', 'Happy'], ['В дороге', 'Road trip'], ['На тренировке', 'Workout']].map(([ru, en]) => <button className="secondary-button" key={en} onClick={() => useApp.getState().setSearch(english ? en : ru)}>{t(ru, en)}</button>)}</div>
      {query.trim().length < 2 ? <Empty message={t('Опиши настроение', 'Describe a mood')} detail={t('Например: спокойная ночная музыка', 'For example: calm music for the night')} /> : waitingForInput || vibeLoading ? <div className="status" role="status"><LoaderCircle className="spin" /> {t('Ищем подходящее звучание…', 'Finding the right sound…')}</div> : vibeError ? <Empty message={String(vibeError).includes('PREVIEW_SEARCH') ? t('Поиск доступен в приложении', 'Search is available in the app') : t('Поиск не удался', 'Search failed')} detail={String(vibeError).includes('PREVIEW_SEARCH') ? undefined : String(vibeError)} /> : <TrackRows tracks={vibe || []} recommended />}
    </>}
    {tab === 'lyrics' && <>
      <p className="search-mode-hint">{t('Введи запомнившиеся слова в строку поиска. Самое точное совпадение будет первым.', 'Type the words you remember in the search bar. The closest match appears first.')}</p>
      {query.trim().length < 4 ? <Empty message={t('Вспомни строчку песни', 'Remember a lyric')} detail={t('Лучше несколько слов подряд.', 'A few consecutive words work best.')} /> : query.trim().length > 200 ? <Empty message={t('Сократи строчку до 200 символов', 'Shorten the line to 200 characters')} /> : waitingForInput || lyricsLoading ? <div className="status" role="status"><LoaderCircle className="spin" /> {t('Ищем песню по словам…', 'Finding the song by its lyrics…')}</div> : lyricsError ? <>
        <Empty message={String(lyricsError).includes('PREVIEW_SEARCH') ? t('Поиск доступен в приложении', 'Search is available in the app') : t('Поиск по тексту временно недоступен', 'Lyric search is temporarily unavailable')} detail={t('Попробуй позже или найди трек по названию.', 'Try again later or search by the track title.')} />
        <button className="secondary-button" onClick={() => void retryLyrics()}>{t('Повторить', 'Retry')}</button>
      </> : lyricMatches?.length ? <LyricSearchResults matches={lyricMatches} english={english} /> : <Empty message={t('Совпадений не найдено', 'No matches found')} detail={t('Попробуй другую строчку или добавь ещё несколько слов.', 'Try another line or add a few more words.')} />}
    </>}
    {tab === 'playlists' && <Status value={lists}>{items => <PlaylistCards playlists={items.filter(item => !(item.is_album || item.playlist_type === 'album' || item.set_type === 'album'))} />}</Status>}
    {tab === 'albums' && <Status value={lists}>{items => <PlaylistCards playlists={items.filter(item => item.is_album || item.playlist_type === 'album' || item.set_type === 'album')} />}</Status>}
    {tab === 'artists' && <Status value={users}>{items => items.length ? <div className="artist-grid">{items.map(user => <button className="artist-card" key={user.id} onClick={() => openArtist(user.id, user.username)}>{user.avatar_url ? <RemoteImage className="artist-avatar" src={user.avatar_url} pixels={160} alt="" loading="lazy" /> : <span className="artist-avatar">{user.username.slice(0, 1).toUpperCase()}</span>}<strong>{user.username}</strong><small>{user.followers_count} {t('подписчиков', 'followers')}</small></button>)}</div> : <Empty message={t('Авторы не найдены', 'No artists found')} />}</Status>}
  </div>
}

function TrackTimeline({ track }: { track: Track }) {
  const queryClient = useQueryClient()
  const english = useEnglish()
  const { data: state } = useQuery({ queryKey: ['player'], queryFn: api.player, refetchInterval: 500 })
  const { data: waveform } = useQuery({ queryKey: ['waveform', track.waveform_url], queryFn: () => api.waveformSamples(track.waveform_url!), enabled: !!track.waveform_url, staleTime: 30 * 60_000, retry: false })
  const active = state?.current != null && state.queue[state.current]?.id === track.id
  const total = Math.max(1, active && state?.previewFallback ? state.durationMs : track.full_duration_ms || track.duration || 1)
  const progress = active ? Math.min(1, (state.positionMs || 0) / total) : 0
  const bars = useMemo(() => Array.from({ length: 72 }, (_, index) => {
    if (waveform?.values.length) {
      const from = Math.floor(index * waveform.values.length / 72)
      const to = Math.max(from + 1, Math.floor((index + 1) * waveform.values.length / 72))
      const sample = waveform.values.slice(from, to)
      return Math.max(10, Math.min(100, Math.round(sample.reduce((sum, value) => sum + value, 0) / sample.length / waveform.height * 100)))
    }
    const noise = Math.abs(Math.sin(track.id * .017 + index * 3.71) * Math.cos(index * .83 + track.id * .003))
    return 25 + Math.round(noise * 65)
  }), [track.id, waveform])
  const seek = async (fraction: number) => {
    if (!active) await api.play([track], 0)
    await api.transport('seek', Math.round(total * Math.max(0, Math.min(1, fraction))))
    await queryClient.invalidateQueries({ queryKey: ['player'] })
  }
  return <div className="detail-timeline"><button type="button" className="detail-timeline-bars" aria-label={english ? 'Seek track' : 'Перемотать трек'} onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); void seek((event.clientX - rect.left) / rect.width) }}>{bars.map((height, index) => <span key={index} className={index / bars.length <= progress ? 'played' : ''} style={{ height: height + '%' }} />)}</button><div className="detail-timeline-times"><span>{duration(active ? state.positionMs : 0)}</span><span>{duration(total)}</span></div></div>
}

function SimilarTrackShelf({ tracks, title, subtitle }: { tracks: Track[]; title: string; subtitle: string }) {
  const queryClient = useQueryClient()
  const english = useEnglish()
  const [error, setError] = useState('')
  if (!tracks.length) return null
  return <div className="similar-shelf"><div className="similar-shelf-heading"><div><h3>{title}</h3><p>{subtitle}</p></div><span>{tracks.length}</span></div>{error && <p className="error-text" role="alert">{error}</p>}<div className="similar-track-strip">{tracks.slice(0, 16).map((track, index) => <article className="similar-track-card" key={track.id}><button className="similar-track-cover" aria-label={`${english ? 'Play' : 'Слушать'} ${track.title}`} onClick={() => void api.play(tracks, index, true).then(() => queryClient.invalidateQueries({ queryKey: ['player'] })).catch(cause => setError(String(cause)))}><Artwork item={track} size="card" /><span><Play size={19} fill="currentColor" /></span></button><button className="similar-track-title" onClick={() => useApp.getState().openTrack(track.id, track.title)}>{track.title}</button><small>{artist(track)}</small>{track.playback_count != null && <small>{track.playback_count.toLocaleString(english ? 'en-US' : 'ru-RU')} {english ? 'plays' : 'прослушиваний'}</small>}</article>)}</div></div>
}

function TrackLinerNotes({ track, english }: { track: Track; english: boolean }) {
  const [expanded, setExpanded] = useState(false)
  const t = (ru: string, en: string) => english ? en : ru
  const published = track.created_at && !Number.isNaN(Date.parse(track.created_at))
    ? new Intl.DateTimeFormat(english ? 'en-US' : 'ru-RU', { day: 'numeric', month: 'long', year: 'numeric' }).format(new Date(track.created_at))
    : null
  const description = track.description?.trim()
  const lengthy = !!description && description.length > 420
  return <section className="detail-content-panel track-liner-notes">
    <div className="track-liner-stats">
      {track.playback_count != null && <span><strong>{track.playback_count.toLocaleString(english ? 'en-US' : 'ru-RU')}</strong>{t('прослушиваний', 'plays')}</span>}
      {track.likes_count != null && <span><strong>{track.likes_count.toLocaleString(english ? 'en-US' : 'ru-RU')}</strong>{t('лайков', 'likes')}</span>}
      {track.comment_count != null && <span><strong>{track.comment_count.toLocaleString(english ? 'en-US' : 'ru-RU')}</strong>{t('комментариев', 'comments')}</span>}
    </div>
    <div className="track-liner-body">
      <div className="track-liner-description"><h2>{t('О треке', 'About this track')}</h2>{description ? <><p className={!expanded && lengthy ? 'is-collapsed' : ''}>{description}</p>{lengthy && <button className="text-link" onClick={() => setExpanded(!expanded)}>{expanded ? t('Свернуть', 'Show less') : t('Читать полностью', 'Read more')}</button>}</> : <p className="muted">{t('Автор не добавил описание.', 'The artist has not added a description.')}</p>}</div>
      <dl className="track-liner-facts"><div><dt>{t('Длительность', 'Duration')}</dt><dd>{duration(track.full_duration_ms || track.duration)}</dd></div>{published && <div><dt>{t('Опубликовано', 'Published')}</dt><dd>{published}</dd></div>}{track.genre && <div><dt>{t('Жанр', 'Genre')}</dt><dd>{track.genre}</dd></div>}</dl>
    </div>
  </section>
}

function TrackPage({ id }: { id: number }) {
  const queryClient = useQueryClient()
  const { data } = useQuery({ queryKey: ['track', id], queryFn: () => api.track(id), refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: related } = useTracks('related', undefined, id)
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const { data: likedTracks } = useQuery({ queryKey: ['tracks', 'likes', undefined, undefined], queryFn: () => api.tracks('likes'), refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: repostedTracks } = useQuery({ queryKey: ['tracks', 'reposts'], queryFn: () => api.tracks('reposts'), refetchInterval: result => dataRefreshInterval(result.state.data) })
  const [error, setError] = useState('')
  const play = async (track: Track) => { try { await api.play([track], 0); await queryClient.invalidateQueries({ queryKey: ['player'] }) } catch (cause) { setError(String(cause)) } }
  const isLiked = likedTracks?.status === 'ready' ? likedTracks.data.some(track => track.id === id) : !!settings?.liked_ids.includes(id)
  const isReposted = repostedTracks?.status === 'ready' && repostedTracks.data.some(track => track.id === id)
  const pinned = !!settings?.quick_access.some(item => { const target = quickAccessTarget(item); return target?.kind === 'track' && target.id === id })
  const like = async (track: Track) => { try { await api.setLiked(track.id, !isLiked); await queryClient.invalidateQueries({ queryKey: ['settings'] }); await queryClient.invalidateQueries({ queryKey: ['tracks', 'likes'] }) } catch (cause) { setError(String(cause)) } }
  const repost = async (track: Track) => { try { await api.repost('track', track.id, !isReposted); await queryClient.invalidateQueries({ queryKey: ['tracks', 'reposts'] }) } catch (cause) { setError(String(cause)) } }
  const pin = async (track: Track) => { try { await api.toggleQuickAccess({ track: { id: track.id, title: track.title, artist: artist(track), artwork_url: cover(track) || null } }); await queryClient.invalidateQueries({ queryKey: ['settings'] }) } catch (cause) { setError(String(cause)) } }
  return <div className="page-content detail-page"><Status value={data}>{track => <>
    <section className="track-detail detail-hero">
      <div className="detail-cover"><Artwork item={track} size="hero" /></div>
      <div className="track-detail-copy">
        <span className="detail-kicker">{t('ТРЕК', 'TRACK')}{track.genre ? ' · ' + track.genre : ''}</span>
        <h1>{track.title}</h1>
        <div className="detail-artist-line">
          {track.user?.avatar_url && sameArtistName(artist(track), track.user.username) && <RemoteImage src={track.user.avatar_url} pixels={160} alt="" loading="lazy" />}
          <ArtistCredits track={track} className="track-artist-button" english={english} />
        </div>
        <div className="track-detail-stats">
          <span>{duration(track.full_duration_ms || track.duration)}</span>
          {track.playback_count != null && <span>{track.playback_count.toLocaleString(english ? 'en-US' : 'ru-RU')} {t('прослушиваний', 'plays')}</span>}
          {track.likes_count != null && <span>{track.likes_count.toLocaleString(english ? 'en-US' : 'ru-RU')} {t('лайков', 'likes')}</span>}
        </div>
        <div className="track-detail-buttons">
          <button className="primary-button" onClick={() => void play(track)}><Play size={16} fill="currentColor" /> {t('Слушать', 'Play')}</button>
          <StationButton kind="track" tracks={[track]} english={english} />
          <button className="secondary-button" onClick={() => void like(track)}><Heart size={16} fill={isLiked ? 'currentColor' : 'none'} /> {isLiked ? t('Убрать лайк', 'Unlike') : t('Нравится', 'Like')}</button>
          <button className="secondary-button" disabled={repostedTracks?.status !== 'ready'} onClick={() => void repost(track)}><Repeat2 size={16} /> {isReposted ? t('Убрать репост', 'Remove repost') : t('Репост', 'Repost')}</button>
          <button className="secondary-button" onClick={() => void pin(track)}><Pin size={16} fill={pinned ? 'currentColor' : 'none'} /> {pinned ? t('Открепить', 'Unpin') : t('Быстрый доступ', 'Pin to quick access')}</button>
          <ShareButton item={track} english={english} label />
        </div>
      </div>
      <TrackTimeline track={track} />
    </section>
    <TrackLinerNotes key={track.id} track={track} english={english} />
    {error && <p className="error-text">{error}</p>}
    <section className="detail-content-panel detail-similar"><div className="track-similar-heading"><SectionTitle title={t('Похожее', 'Similar')} subtitle={t('Музыка рядом по звучанию и настроению', 'Music with a similar sound and mood')} />{related?.status === 'ready' && related.data.some(item => item.id !== track.id) && <button className="secondary-button" onClick={() => void api.play(related.data.filter(item => item.id !== track.id), 0, true).then(() => queryClient.invalidateQueries({ queryKey: ['player'] })).catch(cause => setError(String(cause)))}><Play size={14} fill="currentColor" /> {t('Слушать всё', 'Play all')}</button>}</div><Status value={related}>{items => { const candidates = items.filter(item => item.id !== track.id); const sameArtist = candidates.filter(item => (track.user?.id != null && item.user?.id === track.user.id) || sameArtistName(artist(item), artist(track))); const otherArtists = candidates.filter(item => !sameArtist.includes(item)); return candidates.length ? <><SimilarTrackShelf tracks={sameArtist.length ? sameArtist : candidates} title={sameArtist.length ? t('Этого артиста', 'From this artist') : t('Продолжить слушать', 'Keep listening')} subtitle={sameArtist.length ? t('Другие треки автора', 'More from the same artist') : t('Треки рядом по настроению', 'More music in this mood')} /><SimilarTrackShelf tracks={otherArtists} title={t('Другие авторы', 'Other artists')} subtitle={t('Ещё может понравиться', 'More to explore')} /></> : <Empty message={t('Похожих треков пока нет', 'No similar tracks yet')} /> }}</Status></section>
    <div className="track-community-grid"><div className="track-community-main"><TrackComments track={track} /><TrackCreatorTools track={track} /></div><aside className="track-community-side"><section className="track-uploader-card"><span className="detail-kicker">{t('ОПУБЛИКОВАЛ', 'UPLOADED BY')}</span><button className="track-uploader-person" onClick={() => track.user?.id && useApp.getState().openArtist(track.user.id, track.user.username)}>{track.user?.avatar_url ? <RemoteImage src={track.user.avatar_url} pixels={160} alt="" loading="lazy" /> : <span className="track-uploader-avatar">{(track.user?.username || '?').slice(0, 1).toUpperCase()}</span>}<strong>{track.user?.username || t('Неизвестный автор', 'Unknown artist')}</strong><ChevronRight size={17} /></button></section>{related?.status === 'ready' && related.data.some(item => item.id !== track.id) && <section className="track-side-related"><h3>{t('Дальше слушать', 'Up next')}</h3>{related.data.filter(item => item.id !== track.id).slice(0, 5).map(item => <button key={item.id} className="track-side-row" onClick={() => useApp.getState().openTrack(item.id, item.title)}><Artwork item={item} size="row" /><span><strong>{item.title}</strong><small>{artist(item)}</small></span><ChevronRight size={14} /></button>)}</section>}</aside></div>
  </>}</Status></div>
}

const eqLabels = ['31 Гц', '62 Гц', '125 Гц', '250 Гц', '500 Гц', '1 кГц', '2 кГц', '4 кГц', '8 кГц', '16 кГц']
function Equalizer({ settings, update }: { settings: Settings; update: (key: string, value: unknown) => Promise<void> }) {
  const english = settings.language === 'English'
  const [gains, setGains] = useState(settings.eq_gains_db)
  const [preamp, setPreamp] = useState(settings.eq_preamp_db)
  const [balance, setBalance] = useState(settings.balance)
  useEffect(() => { setGains(settings.eq_gains_db); setPreamp(settings.eq_preamp_db); setBalance(settings.balance) }, [settings])
  const presets = [
    { name: english ? 'Flat' : 'Ровно', values: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
    { name: english ? 'Bass' : 'Бас', values: [5, 4, 3, 1, 0, 0, 0, 0, 0, 0] },
    { name: english ? 'Vocals' : 'Вокал', values: [-2, -1, 0, 1, 3, 4, 3, 1, 0, -1] },
    { name: english ? 'Bright' : 'Яркость', values: [-1, -1, 0, 0, 0, 1, 2, 3, 4, 3] },
  ]
  return <div className="settings-card equalizer-card"><div className="equalizer-heading"><div><h3>{english ? 'Equalizer' : 'Эквалайзер'}</h3><p>{english ? 'Adjust frequencies from −12 to +12 dB' : 'Настройка частот от −12 до +12 дБ'}</p></div><label className="eq-power"><input type="checkbox" checked={settings.eq_enabled} onChange={event => void update('eq_enabled', event.target.checked)} /> {english ? 'On' : 'Включён'}</label></div>
    <div className="eq-presets">{presets.map(preset => <button key={preset.name} onClick={() => { setGains(preset.values); void update('eq_gains_db', preset.values) }}>{preset.name}</button>)}</div>
    <div className={settings.eq_enabled ? 'eq-console' : 'eq-console disabled'}><div className="eq-scale"><span>+12</span><span>0</span><span>−12</span></div>{[-1, ...eqLabels.map((_, index) => index)].map(index => { const value = index < 0 ? preamp : gains[index] || 0; const label = index < 0 ? 'Preamp' : eqLabels[index]; const commit = (next: number) => void update(index < 0 ? 'eq_preamp_db' : 'eq_gains_db', index < 0 ? next : gains.map((gain, at) => at === index ? next : gain)); return <label className="eq-column" key={label}><output>{value > 0 ? '+' : ''}{value.toFixed(1)}</output><input type="range" min={-12} max={12} step={0.5} value={value} aria-label={label} onChange={event => { const next = Number(event.target.value); if (index < 0) setPreamp(next); else setGains(current => current.map((gain, at) => at === index ? next : gain)) }} onPointerUp={event => commit(Number(event.currentTarget.value))} onKeyUp={event => { if (['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) commit(Number(event.currentTarget.value)) }} /><span>{label}</span></label> })}</div>
    <div className="eq-balance"><span>{english ? 'Left' : 'Левый'}</span><input type="range" min={-1} max={1} step={0.01} value={balance} aria-label={english ? 'Balance' : 'Баланс'} onChange={event => setBalance(Number(event.target.value))} onPointerUp={event => void update('balance', Number(event.currentTarget.value))} /><span>{english ? 'Right' : 'Правый'}</span><output>{balance === 0 ? english ? 'Center' : 'Центр' : `${Math.round(balance * 100)}%`}</output></div>
  </div>
}

function OfflinePage() {
  const english = useEnglish()
  const t = (ru: string, en: string) => english ? en : ru
  const queryClient = useQueryClient()
  const { data: saved = [], refetch } = useQuery({ queryKey: ['offline-tracks'], queryFn: api.offlineTracks })
  const { data: capacity, refetch: refreshCapacity } = useQuery({ queryKey: ['offline-capacity'], queryFn: api.offlineCapacity, refetchInterval: 10_000 })
  const { data: offlineSettings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const estimate = (tracks: Track[]) => tracks.reduce((sum, track) => sum + Math.max(track.full_duration_ms || track.duration || 0, 30_000) * 24 * 1.15 + 65_536, 0)
  const { data: liked } = useTracks('likes')
  const downloadEstimate = estimate(liked?.status === 'ready' ? liked.data.filter(track => !saved.some(entry => entry.track.id === track.id)) : [])
  const [busy, setBusy] = useState<number | null>(null)
  const [progress, setProgress] = useState('')
  const [error, setError] = useState('')
  const savedIds = new Set(saved.map(entry => entry.track.id))
  const likedTracks = liked?.status === 'ready' ? liked.data : []
  useEffect(() => {
    if (liked?.status !== 'ready') return
    void api.setOfflineLikeOrder(liked.data.map(track => track.id))
      .then(() => refetch())
      .catch(cause => setError(String(cause)))
  }, [liked, refetch])
  const likedPositions = new Map(likedTracks.map((track, index) => [track.id, index]))
  const orderedSaved = [...saved].sort((a, b) =>
    (likedPositions.get(a.track.id) ?? Number.MAX_SAFE_INTEGER)
    - (likedPositions.get(b.track.id) ?? Number.MAX_SAFE_INTEGER))
  const download = async (track: Track) => {
    setBusy(track.id); setError('')
    try { await api.downloadOfflineTrack(track); await refetch() }
    catch (cause) { setError(`${track.title}: ${String(cause)}`) }
    finally { setBusy(null) }
  }
  const downloadAll = async () => {
    const pending = likedTracks.filter(track => !savedIds.has(track.id))
    setError('')
    for (const [index, track] of pending.entries()) {
      setProgress(`${index + 1} / ${pending.length}: ${track.title}`)
      setBusy(track.id)
      try { await api.downloadOfflineTrack(track); await refetch() }
      catch (cause) { setError(`${track.title}: ${String(cause)}`) }
    }
    setBusy(null); setProgress('')
  }
  const play = async (index: number) => {
    try { await api.play(orderedSaved.map(entry => entry.track), index); await queryClient.invalidateQueries({ queryKey: ['player'] }) }
    catch (cause) { setError(String(cause)) }
  }
  return <div className="page-content offline-page"><SectionTitle title={t('Офлайн', 'Offline')} subtitle={t('Сохрани лайкнутые треки и слушай без сети', 'Save liked tracks and listen without a connection')} />
    <div className="offline-summary"><div><strong>{saved.length}</strong><span>{t('сохранено треков', 'saved tracks')}</span></div><div><strong>{(saved.reduce((sum, entry) => sum + entry.bytes, 0) / 1048576).toFixed(1)} MiB</strong><span>{t('занято на диске', 'disk space used')}</span></div><button className="secondary-button" disabled={!orderedSaved.length} onClick={() => void play(0)}><Play size={16} /> {t('Слушать всё', 'Play all')}</button><button className="primary-button" disabled={busy !== null || !likedTracks.some(track => !savedIds.has(track.id))} onClick={() => void downloadAll()}><Download size={16} /> {t('Скачать все лайки', 'Download all likes')}</button><button className="secondary-button danger-button" disabled={busy !== null || !saved.length} onClick={() => { if (!window.confirm(t(`Удалить все ${saved.length} офлайн-треков?`, `Remove all ${saved.length} offline tracks?`))) return; void api.clearOfflineTracks().then(async () => { await refetch(); await queryClient.invalidateQueries({ queryKey: ['storage-report'] }) }).catch(cause => setError(String(cause))) }}>{t('Удалить все загрузки', 'Remove all downloads')}</button></div>
    <div className="settings-card offline-capacity"><p>{t('Для недостающих лайков потребуется примерно', 'Missing likes need approximately')} {(downloadEstimate / 1048576).toFixed(1)} MiB · {t('Свободно на диске', 'Free disk space')}: {capacity ? (capacity.freeBytes / 1073741824).toFixed(1) + ' GiB' : '…'}</p><label className="field-label">{t('Лимит офлайн-хранилища', 'Offline storage limit')}<select value={offlineSettings?.offline_limit_mb || 0} onChange={e => void api.setSetting('offline_limit_mb', Number(e.target.value)).then(async () => { await queryClient.invalidateQueries({ queryKey: ['settings'] }); await refreshCapacity() }).catch(e => setError(String(e)))}>{[0, 1024, 5120, 10240, 51200].map(mb => <option value={mb} key={mb}>{mb ? `${mb / 1024} GiB` : t('Без лимита', 'No limit')}</option>)}</select></label><button disabled={busy !== null || !saved.some(entry => !entry.pinned)} onClick={() => { if (!window.confirm(t('Удалить незакреплённые загрузки? Текущий трек останется.', 'Remove unpinned downloads? The current track will stay.'))) return; void api.clearUnpinnedTracks().then(async () => { await refetch(); await refreshCapacity() }).catch(e => setError(String(e))) }}>{t('Очистить незакреплённое', 'Clear unpinned')}</button></div>
    {progress && <p role="status">{t('Загрузка', 'Downloading')} {progress}</p>}{error && <p className="error-text" role="alert">{error}</p>}
    <SectionTitle title={t('На устройстве', 'On this device')} subtitle={t('Эти треки доступны без подключения', 'These tracks are available offline')} />
    {orderedSaved.length ? <div className="offline-list">{orderedSaved.map(({ track, pinned }, index) => <div className="offline-item" key={track.id}><Artwork item={track} /><div><strong>{track.title}</strong><small>{artist(track)}</small></div><button className="secondary-button" onClick={() => void play(index)}><Play size={14} /> {t('Слушать', 'Play')}</button><button className="secondary-button" aria-pressed={pinned} onClick={() => void api.pinOfflineTrack(track.id, !pinned).then(() => refetch()).catch(cause => setError(String(cause)))}>{pinned ? t('Закреплён', 'Pinned') : t('Закрепить', 'Pin')}</button><button className="icon-button" title={t('Удалить загрузку', 'Remove download')} aria-label={`${t('Удалить', 'Remove')} ${track.title}`} onClick={() => void api.removeOfflineTrack(track.id).then(() => refetch()).catch(cause => setError(String(cause)))}><X size={16} /></button></div>)}</div> : <Empty message={t('Пока ничего не скачано', 'Nothing downloaded yet')} detail={t('Скачай лайки или отдельные треки ниже.', 'Download your likes or individual tracks below.')} />}
    {liked?.status === 'ready' && <><SectionTitle title={t('Твои лайки', 'Your likes')} subtitle={t('Выбери, что сохранить', 'Choose what to save')} /><div className="offline-list">{liked.data.map(track => <div className="offline-item" key={track.id}><Artwork item={track} /><div><strong>{track.title}</strong><small>{artist(track)}</small></div><button className="secondary-button" disabled={busy !== null || savedIds.has(track.id)} onClick={() => void download(track)}>{savedIds.has(track.id) ? t('Сохранён', 'Saved') : busy === track.id ? t('Загрузка…', 'Downloading…') : t('Скачать', 'Download')}</button></div>)}</div></>}
    {liked?.status === 'unavailable' && <p className="muted">{t('Войди в SoundCloud, чтобы загрузить свои лайки. Уже сохранённые треки доступны выше.', 'Sign in to SoundCloud to download your likes. Tracks saved earlier remain available above.')}</p>}
  </div>
}

function SettingsPage() {
  const queryClient = useQueryClient()
  const [section, setSection] = useState<SettingsSection>('general')
  const accountSettingsRequested = useApp(state => state.accountSettingsRequested)
  useEffect(() => {
    if (accountSettingsRequested) { setSection('account'); useApp.getState().clearAccountSettingsRequest() }
  }, [accountSettingsRequested])
  const [settingsSearch, setSettingsSearch] = useState('')
  const { data } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const { data: connection, refetch: refetchConnection } = useQuery({ queryKey: ['connection'], queryFn: api.connection, refetchInterval: 2000 })
  const { data: account } = useQuery({ queryKey: ['my-profile'], queryFn: api.myProfile, enabled: connection?.status === 'signed_in' })
  const [error, setError] = useState('')
  const [accountBusy, setAccountBusy] = useState(false)
  const [phoneCode, setPhoneCode] = useState('')
  const [phoneMessage, setPhoneMessage] = useState('')
  const ownerId = account?.status === 'ready' ? account.data.id : null
  const { data: serverSession } = useQuery({
    queryKey: ['server-session', ownerId], queryFn: api.serverSession,
    enabled: connection?.status === 'signed_in' && ownerId !== null && !!FASTCLOUD_SERVER_URL && !api.preview,
    retry: false, staleTime: 30_000,
  })
  const isAdmin = connection?.status === 'signed_in' && ownerId !== null && serverSession?.user_id === ownerId && serverSession.admin
  const { data: approvalUsers, isPending: usersPending, error: usersError, refetch: refreshApprovals } = useQuery({
    queryKey: ['approval-users', account?.status === 'ready' ? account.data.id : null],
    queryFn: () => api.approvalUsers(FASTCLOUD_SERVER_URL),
    enabled: section === 'users' && isAdmin,
    retry: false,
    staleTime: 0,
    refetchInterval: section === 'users' && isAdmin ? 10_000 : false,
    refetchIntervalInBackground: true,
    refetchOnMount: 'always',
    refetchOnWindowFocus: 'always',
  })
  const setApproval = async (id: number, status: 'approved' | 'denied' | 'pending') => {
    setAccountBusy(true); setError('')
    try { await api.approvalSetUser(FASTCLOUD_SERVER_URL, id, status); await refreshApprovals() }
    catch (cause) { setError(String(cause)) }
    finally { setAccountBusy(false) }
  }
  const adminEnabled = section === 'users' && isAdmin
  const { data: accessSettings, refetch: refreshAccessSettings } = useQuery({
    queryKey: ['approval-settings', ownerId], queryFn: () => api.approvalSettings(FASTCLOUD_SERVER_URL),
    enabled: adminEnabled, retry: false, staleTime: 30_000,
  })
  const { data: mediaStats, refetch: refreshMedia } = useQuery({
    queryKey: ['approval-media', ownerId], queryFn: () => api.approvalMedia(FASTCLOUD_SERVER_URL),
    enabled: adminEnabled, retry: false, staleTime: 0,
    refetchInterval: adminEnabled ? 10_000 : false,
    refetchIntervalInBackground: true,
    refetchOnMount: 'always',
    refetchOnWindowFocus: 'always',
  })
  const setApprovalMode = async (required: boolean) => {
    setAccountBusy(true); setError('')
    try {
      const next = await api.approvalSettings(FASTCLOUD_SERVER_URL, required)
      queryClient.setQueryData(['approval-settings', ownerId], next)
      await refreshApprovals()
    } catch (cause) { setError(String(cause)) }
    finally { setAccountBusy(false) }
  }
  const update = async (key: string, value: unknown) => {
    queryClient.setQueryData<Settings>(['settings'], current => current ? { ...current, [key]: value } : current)
    try { await api.setSetting(key, value); setError('') }
    catch (e) { setError(String(e)); await queryClient.invalidateQueries({ queryKey: ['settings'] }) }
  }
  const accountAction = async (action: () => Promise<void>) => {
    setAccountBusy(true); setError('')
    try { await action(); await refetchConnection() }
    catch (cause) { setError(String(cause)) }
    finally { setAccountBusy(false) }
  }
  const english = data?.language === 'English'
  const sections: { id: SettingsSection; label: string; en: string }[] = [{ id: 'general', label: 'Общее', en: 'General' }, { id: 'appearance', label: 'Оформление', en: 'Appearance' }, { id: 'sound', label: 'Звук', en: 'Sound' }, { id: 'integrations', label: 'Интеграции', en: 'Integrations' }, { id: 'storage', label: 'Хранилище', en: 'Storage' }, { id: 'account', label: 'Аккаунт', en: 'Account' }, ...(isAdmin ? [{ id: 'users' as const, label: 'Управление пользователями', en: 'User management' }] : [])]
  const settingItems: { id: SettingsSection; label: string; en: string; heading: string; terms?: string }[] = [
    { id: 'general', label: 'Язык', en: 'Language', heading: 'Общее' },
    { id: 'general', label: 'Стартовая страница', en: 'Start page', heading: 'Общее' },
    { id: 'general', label: 'Закрывать в трей', en: 'Close to tray', heading: 'Общее' },
    { id: 'general', label: 'Компактный список треков', en: 'Compact track list', heading: 'Общее' },
    { id: 'general', label: 'Музыкальный вкус', en: 'Musical taste', heading: 'Музыкальный вкус', terms: 'жанры предпочтения discovery diversity taste genres' },
    { id: 'general', label: 'Данные аккаунта и резервная копия', en: 'Account data & backup', heading: 'Данные аккаунта и резервная копия', terms: 'синхронизация сервер папки настройки экспорт импорт json sync server folders settings export import' },
    { id: 'appearance', label: 'Тема и акцент', en: 'Theme and accent', heading: 'Тема' },
    { id: 'appearance', label: 'Текст и размеры интерфейса', en: 'Interface text and size', heading: 'Текст и размеры интерфейса', terms: 'шрифт цвет масштаб размер кнопки font scale size colour color' },
    { id: 'appearance', label: 'Подложки', en: 'Surfaces', heading: 'Подложки', terms: 'прозрачность цвет размытие панели сайдбар opacity blur panels sidebar' },
    { id: 'appearance', label: 'Текст песен', en: 'Lyrics', heading: 'Текст песен' },
    { id: 'appearance', label: 'Фоновое изображение', en: 'Background image', heading: 'Фоновое изображение', terms: 'фон wallpaper' },
    { id: 'appearance', label: 'Видимость фона', en: 'Wallpaper visibility', heading: 'Видимость фона', terms: 'подложка затемнение прозрачность overlay dimming' },
    { id: 'appearance', label: 'Производительность', en: 'Performance', heading: 'Производительность', terms: 'eco balanced quality скорость' },
    { id: 'appearance', label: 'Шрифт', en: 'Font', heading: 'Шрифт' },
    { id: 'sound', label: 'Воспроизведение', en: 'Playback', heading: 'Воспроизведение' },
    { id: 'sound', label: 'Громкость и переходы', en: 'Loudness & transitions', heading: 'Громкость и переходы', terms: 'выравнивание нормализация плавный переход кроссфейд паузы normalization crossfade gapless volume' },
    { id: 'sound', label: 'Эквалайзер', en: 'Equalizer', heading: 'Эквалайзер', terms: 'eq' },
    { id: 'sound', label: 'Мини-плеер', en: 'Mini player', heading: 'Мини-плеер' },
    { id: 'integrations', label: 'Discord', en: 'Discord', heading: 'Discord Rich Presence' },
    { id: 'integrations', label: 'Импорт из Spotify', en: 'Spotify import', heading: 'Импорт из Spotify', terms: 'спотифай лайки плейлисты csv json zip exportify импорт' },
    { id: 'integrations', label: 'Импорт из Яндекс Музыки', en: 'Yandex Music import', heading: 'Импорт из Яндекс Музыки', terms: 'oauth токен token' },
    { id: 'storage', label: 'Хранилище и кэш', en: 'Storage and cache', heading: 'Хранилище' },
    { id: 'account', label: 'Анонимность в лидерборде', en: 'Leaderboard privacy', heading: 'Приватность лидерборда', terms: 'анонимность ник приватность рейтинг anonymous name privacy leaderboard' },
    { id: 'account', label: 'Аккаунт SoundCloud', en: 'SoundCloud account', heading: 'Аккаунт SoundCloud' },
    ...(isAdmin ? [{ id: 'users' as const, label: 'Управление пользователями', en: 'User management', heading: 'Управление пользователями', terms: 'заявки онлайн статистика api одобрение администратор сервер кеш users online usage approval administrator server cache' }] : []),
    { id: 'general', label: 'Обновления', en: 'Updates', heading: 'Обновления приложения' },
  ]
  const matchingSettings = settingsSearch.trim() ? settingItems.filter(item => `${item.label} ${item.en} ${item.terms || ''}`.toLocaleLowerCase().includes(settingsSearch.trim().toLocaleLowerCase())) : []
  const openSetting = (item: typeof settingItems[number]) => {
    setSection(item.id)
    setSettingsSearch('')
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const englishHeadings: Record<string, string> = { 'Приватность лидерборда': 'Leaderboard privacy', 'Общее': 'General', 'Тема': 'Theme', 'Текст песен': 'Lyrics', 'Фоновое изображение': 'Background image', 'Видимость фона': 'Wallpaper visibility', 'Производительность': 'Performance', 'Шрифт': 'Font', 'Воспроизведение': 'Playback', 'Эквалайзер': 'Equalizer', 'Мини-плеер': 'Mini player', 'Импорт из Spotify': 'Import from Spotify', 'Импорт из Яндекс Музыки': 'Import from Yandex Music', 'Хранилище': 'Storage', 'Аккаунт SoundCloud': 'SoundCloud account', 'Управление пользователями': 'User management', 'Обновления приложения': 'Application updates' }
      const name = english ? englishHeadings[item.heading] || item.heading : item.heading
      const heading = [...document.querySelectorAll<HTMLElement>('.settings-body h3')].find(node => node.textContent?.toLocaleLowerCase().includes(name.toLocaleLowerCase()))
      heading?.scrollIntoView({ behavior: 'smooth', block: 'start' })
    }))
  }
  return <div className="page-content settings-page"><div className="settings-heading"><SectionTitle title={english ? 'Settings' : 'Настройки'} subtitle={english ? 'Find and adjust Veirune to your liking' : 'Настрой Veirune под себя'} /><label className="settings-search"><Search size={17} /><input value={settingsSearch} onChange={event => setSettingsSearch(event.target.value)} placeholder={english ? 'Search settings…' : 'Поиск по настройкам…'} aria-label={english ? 'Search settings' : 'Поиск по настройкам'} />{settingsSearch && <button aria-label={english ? 'Clear search' : 'Очистить поиск'} onClick={() => setSettingsSearch('')}><X size={16} /></button>}</label></div>{settingsSearch.trim() && <div className="settings-search-results">{matchingSettings.length ? matchingSettings.map(item => <button key={item.id + item.label} onClick={() => openSetting(item)}><Search size={15} /><span>{english ? item.en : item.label}<small>{english ? sections.find(section => section.id === item.id)?.en : sections.find(section => section.id === item.id)?.label}</small></span><ArrowRight size={15} /></button>) : <p>{english ? 'No matching settings' : 'Ничего не найдено'}</p>}</div>}<div className="settings-layout"><nav className="settings-nav" aria-label={english ? 'Settings sections' : 'Разделы настроек'}>{sections.map(item => <button key={item.id} className={section === item.id ? 'active' : ''} onClick={() => { setSection(item.id); setSettingsSearch('') }}>{english ? item.en : item.label}<ChevronRight size={15} /></button>)}</nav><div className="settings-body">
    {section === 'account' && data && <LeaderboardPrivacy settings={data} />}
    {section === 'account' && <div className="settings-card"><h3>{english ? 'SoundCloud account' : 'Аккаунт SoundCloud'}</h3><p>{connection?.status === 'signed_in' ? english ? 'You are signed in to SoundCloud.' : 'Вы вошли в SoundCloud.' : english ? 'Interface preview.' : 'Предпросмотр интерфейса.'}</p>
      {account?.status === 'ready' && <div className="account-details">{account.data.avatar_url ? <RemoteImage src={account.data.avatar_url} pixels={160} alt="" /> : <span className="account-avatar"><Music2 size={22} /></span>}<div><strong>{account.data.username}</strong><span>SoundCloud ID {account.data.id}{account.data.followers_count != null ? ` · ${account.data.followers_count} ${english ? 'followers' : 'подписчиков'}` : ''}</span></div></div>}
      {connection?.status === 'signed_in' && <button className="secondary-button" disabled={accountBusy} onClick={() => void accountAction(api.signOut)}>{english ? 'Sign out' : 'Выйти из аккаунта'}</button>}
    </div>}
    {section === 'account' && connection?.status === 'signed_in' && <div className="settings-card"><h3>{english ? 'Connect your phone' : 'Подключить телефон'}</h3><p>{english ? 'Open Veirune on your phone, choose Connect with PC and enter its code here. Confirm the same account in the browser.' : 'Открой Veirune на телефоне, выбери «Подключить с ПК» и введи здесь его код. Подтверди тот же аккаунт в браузере.'}</p><form className="inline-form" onSubmit={event => { event.preventDefault(); setAccountBusy(true); setError(''); setPhoneMessage(''); void api.pairMobile(phoneCode).then(result => { setPhoneCode(''); setPhoneMessage(english ? `Phone connected: ${result.username}` : `Телефон подключён: ${result.username}`) }).catch(cause => setError(String(cause))).finally(() => setAccountBusy(false)) }}><input aria-label={english ? 'Phone code' : 'Код с телефона'} placeholder="ABCD-EFGH" autoComplete="off" maxLength={9} value={phoneCode} disabled={accountBusy} onChange={event => setPhoneCode(event.target.value.toUpperCase())} /><button className="secondary-button" disabled={accountBusy || phoneCode.replace(/[- ]/g, '').length !== 8}>{accountBusy ? english ? 'Confirm in browser…' : 'Подтверди в браузере…' : english ? 'Connect phone' : 'Подключить телефон'}</button></form>{phoneMessage && <p role="status">{phoneMessage}</p>}</div>}
    {adminEnabled && <div className="settings-card"><h3>{english ? 'User management' : 'Управление пользователями'}</h3>
      <p>{english ? 'Statistics update automatically every 10 seconds while this section is open.' : 'Статистика обновляется автоматически каждые 10 секунд, пока открыт этот раздел.'}</p>
      <p>{english ? 'Audio API requests today (UTC). Cache hits and CDN downloads are excluded; failed API attempts are included. This is not the remaining SoundCloud quota.' : 'Запросы к аудио API за сегодня (UTC). Кеш и загрузки с CDN не учитываются, неудачные обращения к API учитываются. Это не остаток лимита SoundCloud.'}</p>
      <p>{english ? 'Online: the app contacted the server within the last two minutes.' : 'Онлайн: приложение связывалось с сервером в последние две минуты.'}</p>
      {accessSettings && <label className="setting-row"><span>{english ? 'Approve new users manually' : 'Одобрять новых пользователей вручную'}<small>{english ? 'Existing approved users keep access. Blocked users cannot sign in in either mode.' : 'Уже допущенные пользователи сохранят доступ. Заблокированные не смогут войти в любом режиме.'}</small></span><input type="checkbox" checked={accessSettings.approval_required} disabled={accountBusy} onChange={event => void setApprovalMode(event.target.checked)} /></label>}
      <button className="secondary-button" disabled={accountBusy} onClick={() => { void refreshApprovals(); void refreshAccessSettings(); void refreshMedia() }}>{english ? 'Refresh' : 'Обновить'}</button>
      {usersPending && <p>{english ? 'Loading users…' : 'Загрузка пользователей…'}</p>}
      {usersError && <p className="error-text">{english ? 'Could not refresh users. Try Refresh.' : 'Не удалось обновить список. Нажми «Обновить».'}</p>}
      <div className="approval-users">{approvalUsers?.length === 0 ? <p>{english ? 'No users yet.' : 'Пользователей пока нет.'}</p> : approvalUsers?.map(user => <div className="admin-user" key={user.id}>
        <div className="admin-user-identity"><strong>{user.username}{user.id === ownerId ? english ? ' · administrator' : ' · администратор' : ''}</strong><small>{user.status === 'approved' ? english ? 'Access allowed' : 'Доступ разрешён' : user.status === 'denied' ? english ? 'Blocked' : 'Заблокирован' : english ? 'Awaiting approval' : 'Ожидает одобрения'}</small><small>{user.last_seen > 0 ? `${english ? 'Last active' : 'Последняя активность'}: ${new Date(user.last_seen * 1000).toLocaleString(english ? 'en-GB' : 'ru-RU')}` : english ? 'No activity yet' : 'Активности пока нет'}</small></div>
        <span className={`admin-user-presence ${user.online ? 'online' : ''}`}>{user.online == null ? '—' : user.online ? english ? 'Online' : 'Онлайн' : english ? 'Offline' : 'Офлайн'}</span>
        <div className="admin-user-usage"><strong>{user.stream_requests_today == null ? '—' : user.stream_requests_today.toLocaleString(english ? 'en-GB' : 'ru-RU')}</strong><small>{english ? 'API today' : 'API за сегодня'}</small><small>{user.usage_day ? `${user.usage_day} UTC` : english ? 'Update the server' : 'Обнови сервер'}</small></div>
        {user.id !== ownerId && <div className="approval-actions"><button className="secondary-button" disabled={accountBusy || user.status !== 'approved'} aria-pressed={user.editor === true} onClick={() => { setAccountBusy(true); setError(''); void api.community('editor', { userId: user.id, editor: !user.editor }).then(() => refreshApprovals()).catch(() => setError(english ? 'Could not change the editor role.' : 'Не удалось изменить роль редактора.')).finally(() => setAccountBusy(false)) }}>{user.editor ? english ? 'Remove editor' : 'Убрать редактора' : english ? 'Make editor' : 'Назначить редактором'}</button><button className="secondary-button" disabled={accountBusy || user.status === 'approved'} onClick={() => void setApproval(user.id, 'approved')}>{english ? 'Allow access' : 'Разрешить доступ'}</button><button className="secondary-button" disabled={accountBusy || user.status === 'denied'} onClick={() => void setApproval(user.id, 'denied')}>{english ? 'Block' : 'Заблокировать'}</button></div>}
      </div>)}</div>
      {mediaStats && <div className="server-media-stats"><h3>{english ? 'Server audio cache' : 'Серверный кеш музыки'}</h3><p>{english ? 'Cache' : 'Кеш'}: {(mediaStats.cache_bytes / 1024 ** 3).toFixed(2)} / {(mediaStats.cache_limit_bytes / 1024 ** 3).toFixed(0)} GiB · {english ? 'Active downloads' : 'Загружается'}: {mediaStats.active_downloads}</p><p>{english ? 'Audio delivered this month' : 'Отдано аудио за месяц'} ({mediaStats.traffic_month}): {(mediaStats.month_served_bytes / 1024 ** 3).toFixed(2)} GiB</p></div>}
      <ServerOperations english={!!english} serverUrl={FASTCLOUD_SERVER_URL} accountId={ownerId} />
    </div>}
    {section === 'general' && <><UpdateSettingsCard english={english} /><BackupSettings english={!!english} />{data && <TasteSettings settings={data} update={update} />}</>}
    {data && <SettingsSections section={section} settings={data} update={update} showDeveloperSettings={isAdmin} />}
    {section === 'sound' && data && <Equalizer settings={data} update={update} />}
    {error && <p className="error-text">{error}</p>}</div></div></div>
}

export function DetailActions({ page, id, name }: { page: 'playlist' | 'artist'; id: number; name: string }) {
  const queryClient = useQueryClient()
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const { data: detail } = useQuery({ queryKey: ['playlist', id], queryFn: () => api.playlist(id), enabled: page === 'playlist', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: following } = useQuery({ queryKey: ['following'], queryFn: api.following, enabled: page === 'artist', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: own } = useQuery({ queryKey: ['playlists', 'mine'], queryFn: () => api.playlists('mine'), enabled: page === 'playlist', refetchInterval: result => dataRefreshInterval(result.state.data) })
  const { data: connection } = useQuery({ queryKey: ['connection'], queryFn: api.connection, enabled: page === 'playlist' })
  const [error, setError] = useState('')
  const [followBusy, setFollowBusy] = useState(false)
  useEffect(() => {
    if (page !== 'artist') return
    void api.refreshFollowing().then(() => queryClient.invalidateQueries({ queryKey: ['following'] })).catch(cause => setError(String(cause)))
  }, [page, id, queryClient])
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(name)
  const followed = following?.status === 'ready' ? following.data.some(user => user.id === id) : !!settings?.followed_user_ids.includes(id)
  const isOwn = page === 'playlist' && own?.status === 'ready' && own.data.some(list => list.id === id) && (connection?.status === 'signed_in' || (connection?.status === 'demo' && id >= 9000))
  const isPinned = page === 'playlist' && !!settings?.quick_access.some(shortcut => { const target = quickAccessTarget(shortcut); return (target?.kind === 'playlist' || target?.kind === 'album') && target.id === id })
  const pin = async () => {
    const item = detail?.status === 'ready' ? detail.data : null
    const shortcut: QuickAccessShortcut = item?.is_album || item?.playlist_type === 'album' || item?.set_type === 'album' ? { album: { id, title: item?.title || name, artist: item?.user?.username || '', artwork_url: item ? cover(item) || null : null } } : { playlist: { id, title: item?.title || name, artist: item?.user?.username || '', artwork_url: item ? cover(item) || null : null } }
    try { await api.toggleQuickAccess(shortcut); await queryClient.invalidateQueries({ queryKey: ['settings'] }) }
    catch (cause) { setError(String(cause)) }
  }
  const follow = async () => {
    if (followBusy) return
    const next = !followed
    setFollowBusy(true)
    setError('')
    try {
      await api.setFollowed(id, next)
      const cachedUser = queryClient.getQueryData<Data<User>>(['user', id])
      const user = cachedUser?.status === 'ready' ? cachedUser.data : { id, username: name, followers_count: 0 }
      queryClient.setQueryData<Data<User[]>>(['following'], current => ({
        status: 'ready',
        data: next ? [user, ...(current?.status === 'ready' ? current.data : []).filter(item => item.id !== id)] : (current?.status === 'ready' ? current.data : []).filter(item => item.id !== id),
      }))
      queryClient.setQueryData<Settings>(['settings'], current => current && ({ ...current, followed_user_ids: next ? [...new Set([...current.followed_user_ids, id])] : current.followed_user_ids.filter(item => item !== id) }))
      void queryClient.invalidateQueries({ queryKey: ['tracks', 'following'] })
    } catch (cause) { setError(String(cause)) }
    finally { setFollowBusy(false) }
  }
  const remove = async () => {
    if (!window.confirm(t(`Удалить плейлист «${name}»?`, `Delete playlist “${name}”?`))) return
    try { await api.deletePlaylist(id); await queryClient.invalidateQueries({ queryKey: ['playlists'] }); useApp.getState().setPage('library') }
    catch (cause) { setError(String(cause)) }
  }
  const rename = async () => {
    try {
      await api.renamePlaylist(id, draft)
      useApp.getState().renamePlaylist(id, draft.trim())
      await queryClient.invalidateQueries({ queryKey: ['playlists'] })
      setEditing(false)
    } catch (cause) { setError(String(cause)) }
  }
  return <div className="detail-actions">{page === 'artist' && <button className="secondary-button" disabled={followBusy} onClick={() => void follow()}>{followed ? <Check size={16} /> : <Plus size={16} />}{followed ? t('Вы подписаны', 'Following') : t('Подписаться', 'Follow')}</button>}{page === 'playlist' && <button className="secondary-button" onClick={() => void pin()}><Pin size={16} fill={isPinned ? 'currentColor' : 'none'} />{isPinned ? t('Открепить', 'Unpin') : t('Быстрый доступ', 'Pin to quick access')}</button>}{isOwn && <><button className="secondary-button" onClick={() => { setDraft(name); setEditing(!editing) }}>{t('Переименовать', 'Rename')}</button><button className="secondary-button danger-button" onClick={() => void remove()}>{t('Удалить плейлист', 'Delete playlist')}</button></>}{editing && <form className="playlist-create" onSubmit={event => { event.preventDefault(); if (draft.trim()) void rename() }}><input aria-label={t('Новое название плейлиста', 'New playlist name')} value={draft} onChange={event => setDraft(event.target.value)} /><button className="secondary-button" type="submit" disabled={!draft.trim()}>{t('Сохранить', 'Save')}</button></form>}{error && <p className="error-text">{error}</p>}</div>
}

function PlayerBar({ wallpaperUrl }: { wallpaperUrl: string | null }) {
  const { data: state, refetch } = useQuery({ queryKey: ['player'], queryFn: api.player, refetchInterval: 500, retry: false })
  const queueOpen = useApp(s => s.queueOpen)
  const setQueueOpen = useApp(s => s.setQueueOpen)
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings })
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const queryClient = useQueryClient()
  const [volumeValue, setVolumeValue] = useState<number | null>(null)
  const [likeBusy, setLikeBusy] = useState(false)
  const [optimisticLike, setOptimisticLike] = useState<{ id: number; liked: boolean } | null>(null)
  const [playerError, setPlayerError] = useState('')
  const [nowPlayingOpen, setNowPlayingOpen] = useState(false)
  const [soundOpen, setSoundOpen] = useState(false)
  const lastAudibleVolume = useRef(.8)
  useEffect(() => { if (state?.volume && state.volume > 0) lastAudibleVolume.current = state.volume }, [state?.volume])
  useEffect(() => {
    if (!settings?.winamp_window) return
    setNowPlayingOpen(false)
    setSoundOpen(false)
    setQueueOpen(false)
  }, [settings?.winamp_window, setQueueOpen])
  const track = state?.current == null ? null : state.queue[state.current]
  const { data: likedTracks } = useQuery({ queryKey: ['tracks', 'likes', undefined, undefined], queryFn: () => api.tracks('likes'), enabled: !!track, staleTime: 60_000 })
  const trackLiked = !!track && (optimisticLike?.id === track.id ? optimisticLike.liked : likedTracks?.status === 'ready'
    ? likedTracks.data.some(item => item.id === track.id)
    : !!settings?.liked_ids.includes(track.id))
  const toggleLike = async () => {
    if (!track || likeBusy) return
    const currentTrack = track
    const liked = !trackLiked
    setLikeBusy(true)
    setPlayerError('')
    setOptimisticLike({ id: currentTrack.id, liked })
    try {
      await api.setLiked(currentTrack.id, liked)
      await queryClient.invalidateQueries({ queryKey: ['wave-dislikes'] })
      await queryClient.invalidateQueries({ queryKey: ['wave-disliked'] })
      queryClient.setQueryData<Settings>(['settings'], current => current && ({
        ...current,
        liked_ids: liked ? [...new Set([...current.liked_ids, currentTrack.id])] : current.liked_ids.filter(id => id !== currentTrack.id),
      }))
      queryClient.setQueryData<Data<Track[]>>(['tracks', 'likes', undefined, undefined], current => current?.status === 'ready' ? ({
        status: 'ready',
        data: liked ? [currentTrack, ...current.data.filter(item => item.id !== currentTrack.id)] : current.data.filter(item => item.id !== currentTrack.id),
      }) : current)
      setOptimisticLike(null)
      void queryClient.invalidateQueries({ queryKey: ['tracks', 'likes'] })
    } catch (cause) { setPlayerError(String(cause)) }
    finally { setOptimisticLike(null); setLikeBusy(false) }
  }
  const { data: waveDisliked } = useQuery({ queryKey: ['wave-disliked', track?.id], queryFn: () => api.waveDisliked(track!.id), enabled: !!track })
  const [feedbackBusy, setFeedbackBusy] = useState(false)
  const setWaveDisliked = async () => {
    if (!track || feedbackBusy) return
    setFeedbackBusy(true)
    try {
      await api.waveDislike(track, !waveDisliked)
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ['wave-disliked'] }),
        queryClient.invalidateQueries({ queryKey: ['wave-dislikes'] }),
        queryClient.invalidateQueries({ queryKey: ['tracks'], predicate: query => ['discover', 'related', 'recommended_genre'].includes(String(query.queryKey[1])) }),
        queryClient.invalidateQueries({ queryKey: ['search-daily-picks'] }),
        queryClient.invalidateQueries({ queryKey: ['vibe'] }),
        refetch(),
      ])
    } catch (cause) { setPlayerError(String(cause)) }
    finally { setFeedbackBusy(false) }
  }
  const { data: trackDetail } = useQuery({
    queryKey: ['track', track?.id],
    queryFn: () => api.track(track!.id),
    enabled: !!track && !artistCredit(track),
    staleTime: 5 * 60_000,
    refetchInterval: result => result.state.data?.status === 'loading' ? 1200 : result.state.data?.status === 'unavailable' ? 5000 : result.state.data?.status === 'failed' ? 30000 : false,
    retry: false,
  })
  const displayTrack = trackDetail?.status === 'ready' && trackDetail.data.id === track?.id && artistCredit(trackDetail.data) ? trackDetail.data : track
  const artistHint = displayTrack && !artistCredit(displayTrack)
    ? t('SoundCloud не указал отдельного исполнителя; показан профиль загрузившего', 'SoundCloud did not provide a separate artist; showing the uploader profile')
    : displayTrack ? artist(displayTrack) : undefined
  const action = async (name: string, value?: number, index?: number, target?: number) => { await api.transport(name, value, index, target); await refetch() }
  const { position, ...seekHandlers } = useSeekSlider(`${track?.id ?? 0}:${state?.current ?? -1}`, state?.positionMs ?? 0, ms => { void action('seek', ms) })
  const length = Math.max(1, state?.durationMs || track?.full_duration_ms || track?.duration || 1)
  const volumePercent = Math.round(volumeValue ?? (state?.volume ?? 0) * 100)
  const abLabel = state?.abEndMs != null ? 'A–B' : state?.abStartMs != null ? 'B' : 'A'
  const abHint = state?.abEndMs != null
    ? t(`Повтор ${duration(state.abStartMs || 0)}–${duration(state.abEndMs)}; нажать для сброса`, `Loop ${duration(state.abStartMs || 0)}–${duration(state.abEndMs)}; click to clear`)
    : state?.abStartMs != null ? t('Установить точку B', 'Set point B') : t('Установить точку A', 'Set point A')
  const toggleMute = async () => {
    const current = volumeValue != null ? volumeValue / 100 : state?.volume ?? 0
    if (current > 0) lastAudibleVolume.current = current
    setVolumeValue(current > 0 ? 0 : Math.round(lastAudibleVolume.current * 100))
    await action('volume', current > 0 ? 0 : lastAudibleVolume.current)
    setVolumeValue(null)
  }
  const toggleMini = async () => { await api.setSetting('winamp_window', !settings?.winamp_window); await queryClient.invalidateQueries({ queryKey: ['settings'] }) }
  const updateSound = async (key: string, value: unknown) => { await api.setSetting(key, value); await queryClient.invalidateQueries({ queryKey: ['settings'] }) }
  return <><div className="player-bar">
    <div className="player-track">{track ? <><button className="player-cover-open" aria-label={t('Открыть плеер', 'Open player')} onClick={() => setNowPlayingOpen(true)}><Artwork item={track} /></button><div><button className="player-track-title" onClick={() => useApp.getState().openTrack(track.id, track.title)}>{track.title}</button>{state?.error || playerError ? <PlaybackError error={state?.error || playerError || ''} english={!!english} retry={state?.error ? () => void action('retry') : undefined} /> : <ArtistCredits track={displayTrack || track} className="player-artist-button" title={artistHint} english={english} />}</div><div className="player-feedback"><button className={`icon-button ${trackLiked ? 'on' : ''}`} disabled={likeBusy} aria-pressed={trackLiked} aria-label={trackLiked ? t('Убрать лайк у трека', 'Unlike track') : t('Лайкнуть трек', 'Like track')} title={trackLiked ? t('Убрать лайк SoundCloud', 'Remove SoundCloud like') : t('Лайкнуть в SoundCloud', 'Like on SoundCloud')} onClick={() => void toggleLike()}><Heart size={17} fill={trackLiked ? 'currentColor' : 'none'} /></button><button disabled={feedbackBusy} className={`icon-button ${waveDisliked ? 'on' : ''}`} aria-label={waveDisliked ? t('Вернуть в рекомендации', 'Allow in recommendations') : t('Не рекомендовать этот трек', 'Do not recommend this track')} title={waveDisliked ? t('Вернуть в рекомендации', 'Allow in recommendations') : t('Не рекомендовать этот трек', 'Do not recommend this track')} onClick={() => void setWaveDisliked()}><ThumbsDown size={17} fill={waveDisliked ? 'currentColor' : 'none'} /></button><ShareButton item={track} english={!!english} /><EditorialButton id={track.id} english={!!english} /></div></> : <><div className="player-placeholder"><Music2 size={20} /></div><div><strong>{english ? 'Nothing playing' : 'Ничего не играет'}</strong><small>{english ? 'Choose a track to start' : 'Выбери трек, чтобы начать'}</small></div></>}</div>
    <div className="player-center"><div className="transport-controls">
      <button className={`icon-button ${state?.shuffle ? 'on' : ''}`} aria-label={english ? 'Shuffle' : 'Перемешать'} title={english ? 'Shuffle' : 'Перемешать'} onClick={() => void action('shuffle')}><Shuffle size={18} /></button>
      <button className="icon-button" aria-label={english ? 'Previous track' : 'Предыдущий трек'} title={english ? 'Previous track' : 'Предыдущий трек'} onClick={() => void action('previous')}><SkipBack size={20} fill="currentColor" /></button>
      <button className="play-button" aria-label={state?.isPlaying ? english ? 'Pause' : 'Пауза' : english ? 'Play' : 'Воспроизвести'} onClick={() => void action('toggle')}>{state?.loading ? <LoaderCircle className="spin" size={20} /> : state?.isPlaying ? <Pause size={19} fill="currentColor" /> : <Play size={19} fill="currentColor" />}</button>
      <button className="icon-button" aria-label={english ? 'Next track' : 'Следующий трек'} title={english ? 'Next track' : 'Следующий трек'} onClick={() => void action('next')}><SkipForward size={20} fill="currentColor" /></button>
      <button className={`icon-button ${state?.repeat !== 'Off' ? 'on' : ''}`} aria-label={`${english ? 'Repeat' : 'Повтор'}: ${state?.repeat || 'Off'}`} title={`${english ? 'Repeat' : 'Повтор'}: ${state?.repeat || 'Off'}`} onClick={() => void action('repeat')}><Repeat2 size={18} />{state?.repeat === 'One' && <span className="repeat-one">1</span>}</button>
    </div><div className="timeline"><span>{duration(position)}</span><input type="range" className="timeline-seek" min={0} max={length} step={1000} value={Math.min(position, length)} style={{ '--seek-progress': `${Math.min(100, position / length * 100)}%` } as React.CSSProperties} {...seekHandlers} aria-label={t('Позиция трека', 'Track position')} /><span>{duration(length)}</span></div></div>
     <div className="player-extra"><SleepTimerButton state={state} english={!!english} />{state?.previewFallback && <span className="stream-quality" title={t('SoundCloud разрешил воспроизвести только короткий фрагмент', 'SoundCloud made only a short preview available')}>{t('Фрагмент', 'Preview')}</span>}{(state?.bitrateKbps || 0) > 0 && <span className="stream-quality" title={state?.sampleRate ? `${(state.sampleRate / 1000).toFixed(1)} kHz` : undefined}>{state!.bitrateKbps} kbps</span>}<button className={`icon-button ab-loop-button ${state?.abStartMs != null ? 'on' : ''}`} disabled={!track} aria-label={abHint} title={abHint} onClick={() => void action('ab_loop')}>{abLabel}</button><button className="icon-button volume-button" aria-label={volumePercent === 0 ? english ? 'Unmute' : 'Включить звук' : english ? 'Mute' : 'Выключить звук'} title={volumePercent === 0 ? english ? 'Unmute' : 'Включить звук' : english ? 'Mute' : 'Выключить звук'} onClick={() => void toggleMute()}>{volumePercent === 0 ? <VolumeX size={18} /> : volumePercent < 50 ? <Volume1 size={18} /> : <Volume2 size={18} />}</button><Slider.Root className="slider volume-slider" value={[volumePercent]} max={100} step={1} onValueChange={v => { setVolumeValue(v[0]); void api.transport('volume', v[0] / 100) }} onValueCommit={v => { void action('volume', v[0] / 100).finally(() => setVolumeValue(null)) }} aria-label={english ? 'Volume' : 'Громкость'}><Slider.Track className="slider-track"><Slider.Range className="slider-range" /></Slider.Track><Slider.Thumb className="slider-thumb" /></Slider.Root><output className="volume-percent" aria-live="off">{volumePercent}%</output><button className="icon-button sound-controls-trigger" aria-label={t('Настройки звука', 'Sound controls')} title={t('Настройки звука', 'Sound controls')} onClick={() => setSoundOpen(true)}><Settings2 size={18} /></button><button className="icon-button" disabled={!track} aria-label={t('Открыть трек на весь экран', 'Open full screen player')} title={t('На весь экран', 'Full screen')} onClick={() => setNowPlayingOpen(true)}><Maximize2 size={19} /></button><button className={`icon-button queue-trigger ${queueOpen ? 'on' : ''}`} aria-label={english ? 'Open queue' : 'Открыть очередь'} onClick={() => setQueueOpen(true)}><ListMusic size={20} /></button><button className="icon-button" aria-label={settings?.winamp_window ? english ? 'Restore window' : 'Развернуть окно' : english ? 'Mini player' : 'Мини-плеер'} title={settings?.winamp_window ? english ? 'Restore window' : 'Развернуть окно' : english ? 'Mini player' : 'Мини-плеер'} onClick={() => void toggleMini()}>{settings?.winamp_window ? <Maximize2 size={18} /> : <Minimize2 size={18} />}</button></div>
   </div><Dialog.Root open={soundOpen} onOpenChange={setSoundOpen}><Dialog.Portal><Dialog.Overlay className="dialog-overlay sound-controls-overlay" /><Dialog.Content className="sound-controls-panel"><div className="sound-controls-heading"><Dialog.Title>{t('Звук', 'Sound')}</Dialog.Title><Dialog.Close className="icon-button" aria-label={t('Закрыть', 'Close')}><X size={20} /></Dialog.Close></div><div className="sound-speed"><label htmlFor="sound-speed-slider">{t('Скорость', 'Speed')} <output>{(state?.playbackSpeed || 1).toFixed(2)}×</output></label><input id="sound-speed-slider" type="range" min="0.5" max="2" step="0.05" value={state?.playbackSpeed || 1} onChange={event => void action('speed', Number(event.target.value))} /><div className="sound-speed-presets">{[0.75, 1, 1.25, 1.5, 2].map(speed => <button key={speed} className={(state?.playbackSpeed || 1) === speed ? 'active' : ''} onClick={() => void action('speed', speed)}>{speed}×</button>)}</div><small>{t('При изменении скорости меняется и высота звука.', 'Changing speed also changes pitch.')}</small></div>{settings && <Equalizer settings={settings} update={updateSound} />}</Dialog.Content></Dialog.Portal></Dialog.Root><Dialog.Root open={queueOpen} onOpenChange={setQueueOpen}><Dialog.Portal><Dialog.Overlay className="dialog-overlay" /><Dialog.Content className="queue-panel"><QueuePanel state={state} english={!!english} action={action} /></Dialog.Content></Dialog.Portal></Dialog.Root>{nowPlayingOpen && state && track && <NowPlaying state={state} track={track} displayTrack={displayTrack || track} settings={settings} wallpaperUrl={wallpaperUrl} liked={trackLiked} disliked={!!waveDisliked} onLike={() => void toggleLike()} onDislike={() => void setWaveDisliked()} onQueue={() => { setNowPlayingOpen(false); setQueueOpen(true) }} onSound={() => setSoundOpen(true)} onAction={action} onTrack={() => { setNowPlayingOpen(false); useApp.getState().openTrack(track.id, track.title) }} onClose={() => setNowPlayingOpen(false)} />}</>
}

export default function App() {
  const queryClient = useQueryClient()
  const page = useApp(s => s.page)
  const search = useApp(s => s.search)
  const playlistId = useApp(s => s.playlistId)
  const playlistTitle = useApp(s => s.playlistTitle)
  const artistId = useApp(s => s.artistId)
  const artistName = useApp(s => s.artistName)
  const trackId = useApp(s => s.trackId)
  const setPage = useApp(s => s.setPage)
  const setSearch = useApp(s => s.setSearch)
  const searchRef = useRef<HTMLInputElement>(null)
  const [linkError, setLinkError] = useState('')
  const [shortcutsOpen, setShortcutsOpen] = useState(false)
  const [wallpaperOnly, setWallpaperOnly] = useState(false)
  const wallpaperButton = useRef<HTMLButtonElement>(null)
  const wallpaperRestore = useRef<HTMLButtonElement>(null)
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem('fastcloud:sidebar-collapsed') === 'true')
  const [sidebarHidden, setSidebarHidden] = useState(() => localStorage.getItem('fastcloud:sidebar-hidden') === 'true')
  const [libraryOpen, setLibraryOpen] = useState(() => localStorage.getItem('fastcloud:library-open') !== 'false')
  const [lyricsOpen, setLyricsOpen] = useState(() => localStorage.getItem('fastcloud:lyrics-open') === 'true')
  const libraryTab = useApp(s => s.libraryTab)
  useEffect(() => {
    if (page !== 'library') return
    setLibraryOpen(true)
    localStorage.setItem('fastcloud:library-open', 'true')
  }, [page])

  const previousVolume = useRef(.8)
  const { data: connection, error: connectionError } = useQuery({ queryKey: ['connection'], queryFn: api.connection, refetchInterval: result => !result.state.data || ['connecting', 'registering', 'pairing'].includes(result.state.data.status) ? 250 : 2500 })
  const { data: profile } = useQuery({ queryKey: ['my-profile'], queryFn: api.myProfile, enabled: connection?.status === 'signed_in' || api.preview, refetchInterval: result => dataRefreshInterval(result.state.data) })
  useQuery({
    queryKey: ['server-session', profile?.status === 'ready' ? profile.data.id : null], queryFn: api.serverSession,
    enabled: connection?.status === 'signed_in' && profile?.status === 'ready' && !!FASTCLOUD_SERVER_URL && !api.preview,
    retry: false, staleTime: 30_000, refetchInterval: 45_000, refetchIntervalInBackground: true,
  })
  const { data: settings } = useQuery({ queryKey: ['settings'], queryFn: api.settings, refetchInterval: api.preview ? false : 5000 })
  const english = settings?.language === 'English'
  useEffect(() => {
    if (!wallpaperOnly) return
    if (!api.preview && connection?.status !== 'signed_in') { setWallpaperOnly(false); return }
    document.body.classList.add('wallpaper-only')
    wallpaperRestore.current?.focus()
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      event.preventDefault()
      event.stopImmediatePropagation()
      setWallpaperOnly(false)
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.body.classList.remove('wallpaper-only')
      window.removeEventListener('keydown', onKey, true)
      wallpaperButton.current?.focus()
    }
  }, [wallpaperOnly, connection?.status])
  const [fontRevision, setFontRevision] = useState(0)
  const [backgroundRevision, setBackgroundRevision] = useState(0)
  const previousConnection = useRef<string | null>(null)
  const backgroundImage = settings?.background_image
  const localBackground = backgroundImage && (/^[a-zA-Z]:[\\/]/.test(backgroundImage) || backgroundImage.startsWith('/'))
  const wallpaperUrl = backgroundImage?.startsWith('https://') ? backgroundImage : localBackground && !api.preview ? `${convertFileSrc(backgroundImage)}?v=${backgroundRevision}` : null
  const animatedWallpaper = !!backgroundImage && /\.(gif|webp)(?:\?|$)/i.test(backgroundImage)
  useEffect(() => {
    if (!backgroundImage?.startsWith('file://') || api.preview) return
    void api.saveBackground(backgroundImage)
      .then(() => queryClient.invalidateQueries({ queryKey: ['settings'] }))
      .catch(error => setLinkError(String(error)))
  }, [backgroundImage, queryClient])
  useEffect(() => {
    const changed = () => setFontRevision(value => value + 1)
    window.addEventListener('fastcloud:font-changed', changed)
    return () => window.removeEventListener('fastcloud:font-changed', changed)
  }, [])
  useEffect(() => {
    const changed = () => setBackgroundRevision(value => value + 1)
    window.addEventListener('fastcloud:background-changed', changed)
    return () => window.removeEventListener('fastcloud:background-changed', changed)
  }, [])
  useEffect(() => {
    const path = settings?.interface_font
    const root = document.documentElement
    if (!path || api.preview) { root.style.removeProperty('font-family'); return }
    if (!/[/\\]interface-font\.[^/\\]+$/i.test(path)) {
      void api.saveFont(path).then(() => queryClient.invalidateQueries({ queryKey: ['settings'] })).catch(error => setLinkError(String(error)))
      return
    }
    let disposed = false
    const face = new FontFace('FastcloudUser', `url("${convertFileSrc(path).replaceAll('"', '%22')}?v=${fontRevision}")`)
    void face.load().then(loaded => {
      if (disposed) return
      document.fonts.add(loaded)
      root.style.fontFamily = 'FastcloudUser, Inter, ui-sans-serif, system-ui, sans-serif'
    }).catch(error => setLinkError(`${settings?.language === 'English' ? 'Could not load font' : 'Не удалось открыть шрифт'}: ${String(error)}`))
    return () => { disposed = true; document.fonts.delete(face); root.style.removeProperty('font-family') }
  }, [settings?.interface_font, fontRevision, queryClient])
  useEffect(() => {
    const status = connection?.status
    if (!status) return
    const previous = previousConnection.current
    previousConnection.current = status
    if (!api.preview && status !== 'signed_in') {
      queryClient.removeQueries({ queryKey: ['my-profile'] })
      queryClient.removeQueries({ queryKey: ['server-session'] })
      queryClient.removeQueries({ queryKey: ['approval-users'] })
      queryClient.removeQueries({ queryKey: ['approval-settings'] })
      queryClient.removeQueries({ queryKey: ['approval-media'] })
      queryClient.removeQueries({ queryKey: ['server-operations'] })
    }
    if (previous && previous !== status && ['demo', 'public', 'signed_in'].includes(status)) {
      if (status === 'signed_in') void queryClient.invalidateQueries({ predicate: query => query.queryKey[0] !== 'connection' })
      else void queryClient.resetQueries({ predicate: query => query.queryKey[0] !== 'connection' })
    }
  }, [connection?.status, queryClient])
  const startupApplied = useRef(false)
  const previousMini = useRef<boolean | null>(null)
  const mainWindowBounds = useRef<MainWindowBounds | null>(savedWindowBounds())
  const windowLayoutQueue = useRef(Promise.resolve())
  useEffect(() => {
    if (!settings) return
    const root = document.documentElement
    root.dataset.theme = settings.theme === 'System' ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : settings.theme.toLowerCase()
    applyThemeCustomization(settings)
    root.dataset.reducedMotion = String(settings.reduced_motion)
    root.dataset.quality = settings.memory_profile.toLowerCase()
    root.style.setProperty('--accent', `rgb(${settings.accent_rgb.join(',')})`)
    const luminance = settings.accent_rgb
      .map(value => {
        const channel = value / 255
        return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
      })
      .reduce((sum, channel, index) => sum + channel * [0.2126, 0.7152, 0.0722][index], 0)
    root.style.setProperty('--on-accent', luminance > 0.179 ? '#111318' : '#ffffff')
    if (!startupApplied.current) {
      startupApplied.current = true
      const first = { Home: 'home', Search: 'search', Library: 'library', Settings: 'settings' } as const
      const page = first[settings.startup_page]
      if (!hasRestoredNavigation && page !== 'home') useApp.getState().setPage(page)
    }
  }, [settings])
  useEffect(() => {
    if (!settings || api.preview) return
    if (settings.main_window_bounds) mainWindowBounds.current = settings.main_window_bounds
    const mini = settings.winamp_window
    windowLayoutQueue.current = windowLayoutQueue.current.catch(() => {}).then(async () => {
      const appWindow = getCurrentWindow()
      if (mini) {
        const miniWidth = 420
        const miniHeight = 104
        const [physicalSize, scaleFactor] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()])
        const nativeSize = physicalSize.toLogical(scaleFactor)
        const entering = previousMini.current === false || (previousMini.current === null
          && (Math.abs(nativeSize.width - miniWidth) > 4 || Math.abs(nativeSize.height - miniHeight) > 4))
        const monitor = await currentMonitor()
        const size = await appWindow.outerSize()
        if (monitor) {
          const area = monitor.workArea
          const current = await appWindow.outerPosition()
          const x = entering ? area.position.x + area.size.width - size.width - 24 : current.x
          const y = entering ? area.position.y + area.size.height - size.height - 24 : current.y
          await appWindow.setPosition(new PhysicalPosition(
            Math.max(area.position.x, Math.min(x, area.position.x + area.size.width - size.width)),
            Math.max(area.position.y, Math.min(y, area.position.y + area.size.height - size.height)),
          ))
        } else if (entering) await appWindow.center()
      } else {
        if (previousMini.current === true) {
          await appWindow.setAlwaysOnTop(false)
          await appWindow.setMinSize(new LogicalSize(850, 580))
        }
        const migrateLegacyBounds = previousMini.current === null && !settings.main_window_bounds && !!mainWindowBounds.current
        if (previousMini.current === true || migrateLegacyBounds) {
          const bounds = mainWindowBounds.current
          if (bounds) {
            if (await appWindow.isMaximized()) await appWindow.unmaximize()
            await appWindow.setSize(new PhysicalSize(bounds.width, bounds.height))
            const outerSize = await appWindow.outerSize()
            const monitor = (await availableMonitors()).find(candidate => {
              const area = candidate.workArea
              return bounds.x >= area.position.x && bounds.x < area.position.x + area.size.width
                && bounds.y >= area.position.y && bounds.y < area.position.y + area.size.height
            }) || await currentMonitor()
            const area = monitor?.workArea
            const x = area ? Math.max(area.position.x, Math.min(bounds.x, area.position.x + area.size.width - outerSize.width)) : bounds.x
            const y = area ? Math.max(area.position.y, Math.min(bounds.y, area.position.y + area.size.height - outerSize.height)) : bounds.y
            await appWindow.setPosition(new PhysicalPosition(x, y))
            if (bounds.maximized) await appWindow.maximize()
            if (migrateLegacyBounds) await api.setSetting('main_window_bounds', bounds)
          } else if (previousMini.current === true) await appWindow.setSize(new LogicalSize(1280, 800))
        }
      }
      if (mini) await appWindow.setAlwaysOnTop(settings.winamp_on_top)
      previousMini.current = mini
    }).catch(error => console.error('Window layout:', error))
  }, [settings?.winamp_window, settings?.winamp_on_top])
  const view = useMemo(() => page === 'likes' ? 'likes' : page === 'history' ? 'history' : '', [page])
  const { data: pageTracks } = useTracks(view, undefined, page === 'artist' ? artistId || undefined : playlistId || undefined)
  const history = useApp(s => s.history)
  const historyIndex = useApp(s => s.historyIndex)
  const goBack = useApp(s => s.goBack)
  const goForward = useApp(s => s.goForward)
  const openLibraryTab = (tab: typeof libraryTab) => { useApp.getState().setLibraryTab(tab); setPage('library') }
  const saveShortcuts = async (quick_access: QuickAccessShortcut[]) => {
    const previous = queryClient.getQueryData<Settings>(['settings'])
    if (!previous) return
    queryClient.setQueryData<Settings>(['settings'], { ...previous, quick_access })
    try { await api.setSetting('quick_access', quick_access) }
    catch (error) { setLinkError(String(error)); await queryClient.invalidateQueries({ queryKey: ['settings'] }) }
  }
  const reorderShortcut = async (source: number, target: number) => {
    const previous = queryClient.getQueryData<Settings>(['settings'])
    if (!previous || source === target || !previous.quick_access[source] || target < 0 || target >= previous.quick_access.length) return
    const quick_access = [...previous.quick_access]
    quick_access.splice(target, 0, quick_access.splice(source, 1)[0])
    await saveShortcuts(quick_access)
  }
  const openLink = async (raw: string) => {
    try {
      const result = await api.openLink(raw)
      setLinkError('')
      if (result.kind === 'track') useApp.getState().openTrack(result.id, result.title)
      else if (result.kind === 'playlist') useApp.getState().openPlaylist(result.id, result.title)
      else useApp.getState().openArtist(result.id, result.title)
    } catch (cause) { setLinkError(String(cause)) }
  }
  const playLikesShuffled = async () => {
    if (pageTracks?.status !== 'ready' || !pageTracks.data.length) return
    try { await api.play(shuffleTracks(pageTracks.data), 0); await queryClient.invalidateQueries({ queryKey: ['player'] }); setLinkError('') }
    catch (cause) { setLinkError(String(cause)) }
  }
  useEffect(() => {
    if (api.preview || connection?.status === 'signed_in' || !settings?.winamp_window) return
    void api.setSetting('winamp_window', false).then(() => queryClient.invalidateQueries({ queryKey: ['settings'] })).catch(error => setLinkError(String(error)))
  }, [connection?.status, settings?.winamp_window, queryClient])
  useEffect(() => {
    if (!api.preview && connection?.status !== 'signed_in') return
    const check = () => { void api.takePendingLink().then(link => { if (link) void openLink(link) }).catch(error => setLinkError(String(error))) }
    check()
    const interval = window.setInterval(check, 1500)
    return () => window.clearInterval(interval)
  }, [connection?.status])
  useEffect(() => {
    if (api.preview) return
    let disposed = false
    let unlisten: (() => void) | undefined
    void getCurrentWebview().onDragDropEvent(event => {
      if (event.payload.type !== 'drop') return
      const path = event.payload.paths[0]
      if (!path) return
      const extension = path.split('.').pop()?.toLowerCase()
      if (['ttf', 'otf', 'ttc', 'otc', 'woff', 'woff2'].includes(extension || '')) {
        void api.saveFont(path).then(async () => {
          await queryClient.invalidateQueries({ queryKey: ['settings'] })
          window.dispatchEvent(new Event('fastcloud:font-changed'))
        }).catch(error => setLinkError(String(error)))
      } else if (['mp3', 'm4a', 'wav', 'flac', 'ogg', 'aac'].includes(extension || '')) {
        useApp.getState().setUploadPath(path)
        useApp.getState().setLibraryTab('uploads')
        useApp.getState().setPage('library')
      }
    }).then(listener => { if (disposed) listener(); else unlisten = listener }).catch(error => setLinkError(String(error)))
    return () => { disposed = true; unlisten?.() }
  }, [queryClient])
  useEffect(() => {
    if (!api.preview && connection?.status !== 'signed_in') return
    const onKey = (event: KeyboardEvent) => {
      if (wallpaperOnly) return
      const target = event.target as HTMLElement | null
      const typing = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target?.isContentEditable
      if ((event.ctrlKey || event.metaKey) && ['f', 'k'].includes(event.key.toLowerCase())) { event.preventDefault(); searchRef.current?.focus() }
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'm' && settings) {
        event.preventDefault()
        void api.setSetting('winamp_window', !settings.winamp_window)
          .then(() => queryClient.invalidateQueries({ queryKey: ['settings'] }))
          .catch(error => setLinkError(String(error)))
      }
      if (event.key === 'F1') { event.preventDefault(); setShortcutsOpen(open => !open); return }
      if (event.key === 'Escape') { useApp.getState().setQueueOpen(false); window.dispatchEvent(new Event('fastcloud:clear-selection')) }
      if (typing) return
      const currentPlayer = queryClient.getQueryData<Awaited<ReturnType<typeof api.player>>>(['player'])
      let action: Promise<void> | null = null
      if (event.code === 'Space') action = api.transport('toggle')
      else if ((event.ctrlKey || event.metaKey) && event.key === 'ArrowRight') action = api.transport('next')
      else if ((event.ctrlKey || event.metaKey) && event.key === 'ArrowLeft') action = api.transport('previous')
      else if (!event.ctrlKey && !event.metaKey && !event.altKey && event.key === 'ArrowRight' && currentPlayer) action = api.transport('seek', currentPlayer.positionMs + 5000)
      else if (!event.ctrlKey && !event.metaKey && !event.altKey && event.key === 'ArrowLeft' && currentPlayer) action = api.transport('seek', Math.max(0, currentPlayer.positionMs - 5000))
      else if (!event.ctrlKey && !event.metaKey && !event.altKey && event.key.toLowerCase() === 'm' && currentPlayer) {
        if (currentPlayer.volume > .01) previousVolume.current = currentPlayer.volume
        action = api.transport('volume', currentPlayer.volume > .01 ? 0 : previousVolume.current)
      }
      if (action) { event.preventDefault(); void action.then(() => queryClient.invalidateQueries({ queryKey: ['player'] })).catch(error => setLinkError(String(error))) }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [settings, queryClient, connection?.status, wallpaperOnly])

  if (!api.preview && connection?.status !== 'signed_in') return <LoginGate connection={connection} connectionError={connectionError ? String(connectionError) : undefined} english={english} />

  return <MemoryProfile.Provider value={settings?.memory_profile || 'Balanced'}><div className={`app-shell ${wallpaperOnly ? 'wallpaper-only' : ''} ${sidebarCollapsed ? 'sidebar-collapsed' : ''} ${sidebarHidden ? 'sidebar-hidden' : ''} ${lyricsOpen && !settings?.winamp_window ? 'lyrics-open' : ''} ${wallpaperUrl ? 'has-wallpaper' : ''} ${animatedWallpaper ? 'animated-wallpaper' : ''} ${settings?.winamp_window ? 'mini-player' : ''}`} style={wallpaperUrl && settings ? { '--wallpaper-dim': settings.background_dim, '--wallpaper-opacity': settings.background_opacity, '--wallpaper-blur': `${settings.background_blur}px`, '--wallpaper-overlay': settings.background_overlay, '--wallpaper-panel-opacity': Math.min(.97, .18 + settings.background_overlay * .98) } as React.CSSProperties : undefined}>{wallpaperUrl && <Wallpaper url={wallpaperUrl} blur={settings?.memory_profile === 'Eco' ? 0 : settings?.background_blur} />}<button ref={wallpaperRestore} className="wallpaper-restore" hidden={!wallpaperOnly} aria-label={english ? 'Show interface' : 'Вернуть интерфейс'} onClick={() => setWallpaperOnly(false)} /><aside className="sidebar"><div className="sidebar-toolbar"><button className="icon-button" aria-label={english ? 'Hide sidebar' : 'Скрыть сайдбар'} title={english ? 'Hide sidebar' : 'Скрыть сайдбар'} onClick={() => { setSidebarHidden(true); localStorage.setItem('fastcloud:sidebar-hidden', 'true') }}><PanelLeftClose size={19} /></button></div>
    <div className="sidebar-navigation-scroll"><nav aria-label={english ? 'Navigation' : 'Навигация'}>{sidebar.map(item => <button key={item.page} title={settings?.language === 'English' ? item.english : item.label} className={`nav-item ${page === item.page ? 'active' : ''}`} onClick={() => setPage(item.page)}><item.icon size={19} strokeWidth={1.9} /><span>{settings?.language === 'English' ? item.english : item.label}</span>{page === item.page && <span className="nav-marker" />}</button>)}</nav>
    <div className="library-navigation"><button className={`nav-item library-navigation-toggle ${page === 'library' ? 'active' : ''}`} title={english ? 'Your Library' : 'Твоя библиотека'} aria-expanded={libraryOpen} onClick={() => {
      if (sidebarCollapsed || window.matchMedia('(max-width: 850px)').matches) { openLibraryTab('tracks'); return }
      setLibraryOpen(value => { localStorage.setItem('fastcloud:library-open', String(!value)); return !value })
    }}><Library size={19} strokeWidth={1.9} /><span>{english ? 'Your Library' : 'Твоя библиотека'}</span>{libraryOpen ? <ChevronDown className="library-chevron" size={16} /> : <ChevronRight className="library-chevron" size={16} />}</button>{libraryOpen && <nav className="library-subnav" aria-label={english ? 'Library sections' : 'Разделы библиотеки'}>{([['tracks', 'Песни', 'Songs'], ['dislikes', 'Дизлайки', 'Dislikes'], ['albums', 'Альбомы', 'Albums'], ['artists', 'Исполнители', 'Artists'], ['playlists', 'Плейлисты', 'Playlists'], ['liked_playlists', 'Любимые плейлисты', 'Liked playlists'], ['stations', 'Станции', 'Stations']] as const).map(([tab, label, en]) => <button key={tab} className={`library-subitem ${page === 'library' && libraryTab === tab ? 'active' : ''}`} aria-current={page === 'library' && libraryTab === tab ? 'page' : undefined} title={english ? en : label} onClick={() => openLibraryTab(tab)}>{english ? en : label}</button>)}</nav>}</div>
    <nav className="sidebar-secondary" aria-label={english ? 'More sections' : 'Другие разделы'}>{([{ page: 'history' as Page, label: 'История', en: 'History', icon: Clock3 }, { page: 'statistics' as Page, label: 'Статистика', en: 'Statistics', icon: ChartColumn }, { page: 'leaderboard' as Page, label: 'Лидерборд', en: 'Leaderboard', icon: Trophy }, { page: 'collections' as Page, label: 'Папки и подборки', en: 'Folders & collections', icon: Folder }, { page: 'offline' as Page, label: 'Офлайн', en: 'Offline', icon: Download }]).map(item => <button key={item.page} className={`nav-item ${page === item.page ? 'active' : ''}`} title={english ? item.en : item.label} onClick={() => setPage(item.page)}><item.icon size={19} strokeWidth={1.9} /><span>{english ? item.en : item.label}</span>{page === item.page && <span className="nav-marker" />}</button>)}</nav>
    {settings && <QuickAccessList items={settings.quick_access} english={english}
      artwork={target => <Artwork item={{ id: target.id, title: target.title, artwork_url: target.artwork_url }} />}
      open={target => target.kind === 'track' ? useApp.getState().openTrack(target.id, target.title) : useApp.getState().openPlaylist(target.id, target.title)}
      unpin={async item => { try { await api.toggleQuickAccess(item); await queryClient.invalidateQueries({ queryKey: ['settings'] }) } catch (error) { setLinkError(String(error)) } }}
      reorder={reorderShortcut} sort={saveShortcuts} />}
    </div>
    <ChatSidebarButton english={!!english} /><div className="sidebar-bottom"><div className="sidebar-rule" /><UpdateSidebarButton english={english} openSettings={() => setPage('settings')} /><button className="nav-item sidebar-compact-toggle" title={sidebarCollapsed ? english ? 'Expand sidebar' : 'Развернуть сайдбар' : english ? 'Compact sidebar' : 'Сайдбар только с иконками'} aria-label={sidebarCollapsed ? english ? 'Expand sidebar' : 'Развернуть сайдбар' : english ? 'Compact sidebar' : 'Сайдбар только с иконками'} onClick={() => setSidebarCollapsed(value => { localStorage.setItem('fastcloud:sidebar-collapsed', String(!value)); return !value })}>{sidebarCollapsed ? <Maximize2 size={17} /> : <Minimize2 size={17} />}<span>{sidebarCollapsed ? english ? 'Expand sidebar' : 'Развернуть сайдбар' : english ? 'Icons only' : 'Только иконки'}</span></button><button className="nav-item" title={english ? 'Choose language' : 'Выбрать язык'} onClick={() => void api.setSetting('language', english ? 'Russian' : 'English').then(() => queryClient.invalidateQueries({ queryKey: ['settings'] }))}><Languages size={19} /><span>{english ? 'English · Русский' : 'Русский · English'}</span></button><button className={`nav-item ${page === 'settings' ? 'active' : ''}`} title={english ? 'Settings' : 'Настройки'} onClick={() => setPage('settings')}><Settings2 size={19} /><span>{english ? 'Settings' : 'Настройки'}</span></button><button className="account-card" onClick={() => profile?.status === 'ready' ? useApp.getState().openArtist(profile.data.id, profile.data.username) : setPage('settings')} aria-label={profile?.status === 'ready' ? `${english ? 'Open profile' : 'Открыть профиль'} ${profile.data.username}` : english ? 'Open account settings' : 'Открыть настройки аккаунта'}><span className="account-avatar">{profile?.status === 'ready' && profile.data.avatar_url ? <RemoteImage src={profile.data.avatar_url} pixels={100} alt="" /> : <Music2 size={19} />}</span><span><strong>{profile?.status === 'ready' ? profile.data.username : connection?.status === 'signed_in' ? 'SoundCloud' : connection?.status === 'public' ? english ? 'Public catalog' : 'Публичный каталог' : connection?.status === 'demo' ? english ? 'Demo mode' : 'Демо-режим' : english ? 'Connecting to SoundCloud…' : 'Подключаем SoundCloud…'}</strong><small>{api.preview ? english ? 'Interface preview' : 'Предпросмотр интерфейса' : connection?.status === 'signed_in' ? english ? 'My profile' : 'Мой профиль' : connection?.status === 'connecting' ? english ? 'Restoring sign-in' : 'Восстанавливаем вход' : english ? 'Connect account' : 'Подключить аккаунт'}</small></span><ChevronRight size={16} /></button></div></aside>
     <main className="main"><header className="topbar">{sidebarHidden && <button className="icon-button sidebar-visibility-toggle" aria-label={sidebarHidden ? english ? 'Show sidebar' : 'Показать сайдбар' : english ? 'Hide sidebar' : 'Скрыть сайдбар'} title={sidebarHidden ? english ? 'Show sidebar' : 'Показать сайдбар' : english ? 'Hide sidebar' : 'Скрыть сайдбар'} aria-expanded={!sidebarHidden} onClick={() => setSidebarHidden(value => { localStorage.setItem('fastcloud:sidebar-hidden', String(!value)); return !value })}>{sidebarHidden ? <PanelLeftOpen size={19} /> : <PanelLeftClose size={19} />}</button>}<div className="history-buttons"><button className="icon-button" aria-label={english ? 'Back' : 'Назад'} disabled={historyIndex <= 0} onClick={goBack}><ArrowLeft size={19} /></button><button className="icon-button" aria-label={english ? 'Forward' : 'Вперёд'} disabled={historyIndex >= history.length - 1} onClick={goForward}><ArrowRight size={19} /></button></div><label className="search-box"><Search size={19} /><input ref={searchRef} value={search} onChange={e => setSearch(e.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (/^(https?:\/\/|soundcloud:|fastcloud:|(?:on\.)?soundcloud\.com\/|\d+$)/i.test(search.trim()))) { event.preventDefault(); void openLink(search.trim()) } }} placeholder={english ? 'Search or paste a SoundCloud link…' : 'Поиск или ссылка SoundCloud…'} aria-label={english ? 'Search' : 'Поиск'} />{search && <button aria-label={english ? 'Clear search' : 'Очистить поиск'} onClick={() => setSearch('')}><X size={16} /></button>}</label><button ref={wallpaperButton} className="icon-button wallpaper-toggle" aria-label={english ? 'Wallpaper only' : 'Только фон'} title={english ? 'Wallpaper only · click or Esc to return' : 'Только фон · клик или Esc, чтобы вернуться'} onClick={() => setWallpaperOnly(true)}><Image size={19} /></button><span className="topbar-pill"><span /> {connection?.status === 'demo' ? english ? 'Offline demo' : 'Офлайн-демо' : connection?.status === 'signed_in' ? english ? 'Connected' : 'На связи' : 'Veirune'}</span></header><UpdateNotice english={english} />{linkError && <div className="link-error error-text" role="alert">{linkError}</div>}
      <div className="scroll-area" key={page === 'artist' ? 'artist:' + artistId : page === 'playlist' ? 'playlist:' + playlistId : page === 'track' ? 'track:' + trackId : page}>
         {page === 'home' && <HomePage />}
          {page === 'feed' && <FeedPage />}
          {page === 'discover' && <DiscoverPage />}
          {page === 'catalog' && <CatalogPage />}
         {page === 'library' && <LibraryPage />}
         {page === 'statistics' && <StatisticsPage english={!!english} />}
         {page === 'leaderboard' && <LeaderboardPage english={!!english} />}
         {page === 'collections' && <CollectionsPage english={!!english} />}
          {page === 'offline' && <OfflinePage />}
          {page === 'reposts' && <RepostsPage />}
          {page === 'inbox' && <InboxPage openLink={openLink} />}
          {page === 'messages' && <MessagesPage />}
          {page === 'search' && <SearchPage query={search} />}
          {page === 'track' && trackId && <TrackPage id={trackId} />}
          {page === 'artist' && artistId && <ArtistPage id={artistId} name={artistName} />}
          {page === 'playlist' && playlistId && <PlaylistPage id={playlistId} name={playlistTitle} />}
        {page === 'settings' && <SettingsPage />}
         {view && <div className="page-content"><div className="page-intro"><span className="page-kicker">VEIRUNE / {(english ? englishTitle[page] : title[page]).toUpperCase()}</span><h1>{page === 'playlist' ? playlistTitle : page === 'artist' ? artistName : english ? englishTitle[page] : title[page]}</h1><p>{page === 'likes' ? english ? 'Tracks you want to return to.' : 'Треки, к которым хочется возвращаться.' : english ? 'Recently played tracks.' : 'Последнее, что ты слушал.'}</p>{page === 'artist' && artistId && <DetailActions page="artist" id={artistId} name={artistName} />}{page === 'playlist' && playlistId && <DetailActions page="playlist" id={playlistId} name={playlistTitle} />}</div>{page === 'likes' && <div className="collection-actions"><button className="secondary-button" disabled={pageTracks?.status !== 'ready' || !pageTracks.data.length} onClick={() => void playLikesShuffled()}><Shuffle size={16} /> {english ? 'Shuffle all' : 'Перемешать всё'}</button></div>}<Status value={pageTracks}>{items => <TrackRows tracks={items} compact={settings?.compact_rows} />}</Status></div>}
      </div>
    </main>{!settings?.winamp_window && <button className={`icon-button lyrics-toggle ${lyricsOpen ? 'on' : ''}`} aria-expanded={lyricsOpen} aria-controls="lyrics-panel" aria-label={lyricsOpen ? english ? 'Hide lyrics panel' : 'Скрыть текст справа' : english ? 'Show lyrics panel' : 'Показать текст справа'} title={lyricsOpen ? english ? 'Hide lyrics panel' : 'Скрыть текст справа' : english ? 'Show lyrics panel' : 'Показать текст справа'} onClick={() => setLyricsOpen(value => { localStorage.setItem('fastcloud:lyrics-open', String(!value)); return !value })}>{lyricsOpen ? <PanelRightClose size={18} /> : <PanelRightOpen size={18} />}</button>}{lyricsOpen && !settings?.winamp_window && <LyricsSidePanel settings={settings} />}<ShareNotice /><FastcloudShareDialog /><PlayerBar wallpaperUrl={wallpaperUrl} /><Dialog.Root open={shortcutsOpen} onOpenChange={setShortcutsOpen}><Dialog.Portal><Dialog.Overlay className="dialog-overlay" /><Dialog.Content className="shortcuts-panel"><Dialog.Title>{english ? 'Keyboard shortcuts' : 'Горячие клавиши'}</Dialog.Title><Dialog.Description>{english ? 'Control playback and the interface' : 'Управление воспроизведением и интерфейсом'}</Dialog.Description><dl>{(english ? [['Space', 'Play / pause'], ['Ctrl + ← / →', 'Previous / next track'], ['← / →', 'Seek by 5 seconds'], ['M', 'Mute / unmute'], ['Ctrl + F / K', 'Search'], ['Ctrl + M', 'Mini player'], ['Esc', 'Clear selection'], ['F1', 'Open this help']] : [['Пробел', 'Воспроизведение / пауза'], ['Ctrl + ← / →', 'Предыдущий / следующий трек'], ['← / →', 'Перемотка на 5 секунд'], ['M', 'Выключить / включить звук'], ['Ctrl + F / K', 'Поиск'], ['Ctrl + M', 'Мини-плеер'], ['Esc', 'Снять выбор'], ['F1', 'Открыть эту справку']]).map(([key, label]) => <div key={key}><dt>{key}</dt><dd>{label}</dd></div>)}</dl><Dialog.Close className="secondary-button">{english ? 'Close' : 'Закрыть'}</Dialog.Close></Dialog.Content></Dialog.Portal></Dialog.Root></div></MemoryProfile.Provider>
}
