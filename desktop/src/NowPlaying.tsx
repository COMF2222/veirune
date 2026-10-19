import { EditorialButton } from './CommunityDiscovery'
import { useAppIcon } from './AppIconPicker'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { AudioLines, Heart, ListMusic, LoaderCircle, MessageCircle, Music2, Pause, Play, Repeat2, Send, Settings2, Shuffle, SkipBack, SkipForward, ThumbsDown, Volume1, Volume2, VolumeX, X } from 'lucide-react'
import { api } from './api'
import { StationButton } from './StationButton'
import { ArtistCredits } from './ArtistCredits'
import { RemoteImage } from './RemoteImage'
import { Wallpaper } from './Wallpaper'
import { useSeekSlider } from './useSeekSlider'
import { parseLrc } from './lyrics'
import { useLyricsLine, useTrackLyrics } from './useTrackLyrics'
import { LyricsFallback, LyricsSource } from './LyricsFallback'
import { artworkForSize, artist, cover, duration, type PlayerState, type Settings, type Track } from './types'

type Props = {
  state: PlayerState
  track: Track
  displayTrack: Track
  settings?: Settings
  wallpaperUrl: string | null
  liked: boolean
  disliked: boolean
  onLike: () => void
  onDislike: () => void
  onQueue: () => void
  onSound: () => void
  onAction: (name: string, value?: number) => Promise<void>
  onTrack: () => void
  onClose: () => void
}

export function NowPlaying({ state, track, displayTrack, settings, wallpaperUrl, liked, disliked, onLike, onDislike, onQueue, onSound, onAction, onTrack, onClose }: Props) {
  const logo = useAppIcon()
  const english = settings?.language === 'English'
  const t = (ru: string, en: string) => english ? en : ru
  const queryClient = useQueryClient()
  const [mode, setMode] = useState<'cover' | 'player'>(() => localStorage.getItem('fastcloud:now-playing-mode') === 'cover' ? 'cover' : 'player')
  const [panel, setPanel] = useState<'lyrics' | 'comments' | null>(() => localStorage.getItem('fastcloud:now-playing-mode') === 'cover' ? null : 'lyrics')
  const [split, setSplit] = useState(() => {
    const saved = Number(localStorage.getItem('fastcloud:now-playing-split'))
    return Number.isFinite(saved) && saved >= 38 && saved <= 76 ? saved : 66
  })
  const layoutRef = useRef<HTMLDivElement>(null)
  const resizing = useRef(false)
  const [volume, setVolume] = useState<number | null>(null)
  const [body, setBody] = useState('')
  const [atTime, setAtTime] = useState(false)
  const [posting, setPosting] = useState(false)
  const [postError, setPostError] = useState('')
  const activeLine = useRef<HTMLButtonElement>(null)
  const lyricsScroll = useRef<HTMLDivElement>(null)
  const art = artworkForSize(displayTrack, 'hero', settings?.memory_profile || 'Balanced')
  const preview = artworkForSize(displayTrack, 'row', settings?.memory_profile || 'Balanced')
  const { position, ...seekHandlers } = useSeekSlider(`${track.id}:${state.current ?? -1}`, state.positionMs, ms => { void onAction('seek', ms) })
  const length = Math.max(1, state.durationMs || track.full_duration_ms || track.duration || 1)
  const volumePercent = volume ?? Math.round(state.volume * 100)

  useEffect(() => { localStorage.setItem('fastcloud:now-playing-split', String(split)) }, [split])

  useEffect(() => {
    const close = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose() }
    }
    document.addEventListener('keydown', close, true)
    return () => document.removeEventListener('keydown', close, true)
  }, [onClose])
  useEffect(() => {
    setBody(''); setPostError('')
    lyricsScroll.current?.scrollTo({ top: 0 })
  }, [track.id])

  const lyrics = useTrackLyrics(displayTrack, length, panel === 'lyrics')
  const comments = useQuery({
    queryKey: ['comments', track.id],
    queryFn: () => api.comments(track.id),
    enabled: panel === 'comments',
    refetchInterval: result => result.state.data?.status === 'loading' ? 1200 : false,
    retry: false,
  })
  const record = lyrics.data
  const synced = useMemo(() => record?.syncedLyrics ? parseLrc(record.syncedLyrics) : [], [record?.syncedLyrics])
  const currentLine = useLyricsLine(panel === 'lyrics' ? synced : [], state)
  useEffect(() => {
    if (settings?.lyrics_auto_scroll === false) return
    const container = lyricsScroll.current
    const line = activeLine.current
    if (panel !== 'lyrics' || !container || !line) return
    const offset = line.getBoundingClientRect().top - container.getBoundingClientRect().top
    const target = Math.max(0, Math.min(container.scrollHeight - container.clientHeight,
      container.scrollTop + offset - container.clientHeight * .38))
    if (settings?.reduced_motion) {
      container.scrollTop = target
      return
    }
    let frame = 0
    let previous = performance.now()
    const animate = (now: number) => {
      const elapsed = Math.min(64, now - previous)
      previous = now
      const next = container.scrollTop + (target - container.scrollTop) * (1 - Math.exp(-elapsed / 220))
      container.scrollTop = Math.abs(target - next) < .5 ? target : next
      if (container.scrollTop !== target) frame = requestAnimationFrame(animate)
    }
    frame = requestAnimationFrame(animate)
    return () => cancelAnimationFrame(frame)
  }, [currentLine, panel, record?.id, settings?.reduced_motion, settings?.lyrics_auto_scroll])

  const chooseMode = (value: 'cover' | 'player') => { setMode(value); setPanel(value === 'cover' ? null : 'lyrics'); localStorage.setItem('fastcloud:now-playing-mode', value) }
  const updateSplit = (clientX: number) => {
    const bounds = layoutRef.current?.getBoundingClientRect()
    if (!bounds) return
    const maximum = Math.min(76, 100 - 300 / bounds.width * 100)
    setSplit(Math.max(38, Math.min(maximum, (clientX - bounds.left) / bounds.width * 100)))
  }
  const post = async () => {
    if (!body.trim() || posting) return
    setPosting(true); setPostError('')
    try {
      await api.postComment(track.id, body.trim(), atTime ? state.positionMs : undefined)
      setBody('')
      await queryClient.invalidateQueries({ queryKey: ['comments', track.id] })
    } catch (error) { setPostError(String(error)) }
    finally { setPosting(false) }
  }

  return <section className={`now-playing ${panel ? 'with-panel' : ''} ${mode === 'cover' ? 'cover-mode' : ''} ${wallpaperUrl ? 'has-user-wallpaper' : ''} ${settings?.lyrics_blur_past ? 'blur-past-lyrics' : ''}`} style={{ '--lyrics-scale': settings?.lyrics_scale || 1, '--wallpaper-dim': settings?.background_dim ?? 0, '--wallpaper-opacity': settings?.background_opacity ?? .25, '--wallpaper-blur': `${settings?.background_blur || 0}px` } as React.CSSProperties} role="dialog" aria-modal="true" aria-label={t('Трек на весь экран', 'Full screen now playing')}>
    {wallpaperUrl ? <Wallpaper url={wallpaperUrl} blur={settings?.memory_profile === 'Eco' ? 0 : settings?.background_blur} className="now-playing-backdrop now-playing-wallpaper" /> : art && <div className="now-playing-backdrop" aria-hidden="true"><RemoteImage src={art} previewSrc={preview} fallback={cover(displayTrack)} alt="" loading="eager" /></div>}
    <div className="now-playing-shade" aria-hidden="true" />
    <header className="now-playing-header"><span className="now-playing-brand"><img src={logo} alt="" /> veirune <small>/ {t('Сейчас играет', 'Now playing')}</small></span><div className="now-playing-header-actions"><StationButton kind="track" tracks={[track]} english={english} iconOnly /><div className="now-playing-segment" role="group" aria-label={t('Вид', 'View')}><button className={mode === 'cover' ? 'active' : ''} aria-pressed={mode === 'cover'} onClick={() => chooseMode('cover')}>{t('Только обложка', 'Cover only')}</button><button className={mode === 'player' ? 'active' : ''} aria-pressed={mode === 'player'} onClick={() => chooseMode('player')}>{t('Плеер', 'Player')}</button></div><button className="now-playing-round" onClick={onSound} title={t('Настройки звука', 'Sound controls')} aria-label={t('Настройки звука', 'Sound controls')}><Settings2 size={19} /></button>{mode === 'player' && <button className="now-playing-round" onClick={() => setPanel(panel ? null : 'lyrics')} title={panel ? t('Скрыть панель', 'Hide panel') : t('Показать панель', 'Show panel')} aria-label={panel ? t('Скрыть панель', 'Hide panel') : t('Показать панель', 'Show panel')}><ListMusic size={19} /></button>}<button className="now-playing-round now-playing-close" onClick={onClose} title={t('Закрыть', 'Close')} aria-label={t('Закрыть', 'Close')}><X size={20} /></button></div></header>
    <div ref={layoutRef} className="now-playing-layout" style={{ '--np-split': `${split}%` } as React.CSSProperties}><div className="now-playing-main"><div className="now-playing-art" style={{ '--art-hue': (track.id * 47) % 360 } as React.CSSProperties}>{art ? <RemoteImage src={art} previewSrc={preview} fallback={cover(displayTrack)} alt={t('Обложка трека', 'Track cover')} loading="eager" /> : <Music2 size={96} strokeWidth={1} />}</div><div className="now-playing-meta"><span>{t('СЕЙЧАС ИГРАЕТ', 'NOW PLAYING')}{state.bitrateKbps > 0 ? ` · ${state.bitrateKbps} kbps` : ''}</span><h1 title={track.title}><button className="now-playing-track-title" onClick={onTrack}>{track.title}</button></h1><ArtistCredits track={displayTrack} className="now-playing-artist-button" onNavigate={onClose} english={english} /></div>{mode === 'player' && <div className="now-playing-controls"><EditorialButton id={track.id} english={!!english} /><div className="now-playing-progress"><span>{duration(position)}</span><input type="range" min={0} max={length} step={1000} value={Math.min(position, length)} style={{ '--seek-progress': `${Math.min(100, position / length * 100)}%` } as React.CSSProperties} aria-label={t('Позиция трека', 'Track position')} {...seekHandlers} /><span>{duration(length)}</span></div><div className="now-playing-transport"><button className={liked ? 'liked' : ''} aria-label={liked ? t('Убрать лайк', 'Unlike') : t('Лайкнуть', 'Like')} title={liked ? t('Убрать лайк', 'Unlike') : t('Лайкнуть', 'Like')} onClick={onLike}><Heart size={20} fill={liked ? 'currentColor' : 'none'} /></button><button className={state.shuffle ? 'liked' : ''} aria-label={t('Перемешать', 'Shuffle')} aria-pressed={state.shuffle} onClick={() => void onAction('shuffle')}><Shuffle size={20} /></button><button aria-label={t('Предыдущий трек', 'Previous track')} onClick={() => void onAction('previous')}><SkipBack size={24} fill="currentColor" /></button><button className="now-playing-play" aria-label={state.isPlaying ? t('Пауза', 'Pause') : t('Воспроизвести', 'Play')} onClick={() => void onAction('toggle')}>{state.loading ? <LoaderCircle className="spin" size={23} /> : state.isPlaying ? <Pause size={23} fill="currentColor" /> : <Play size={23} fill="currentColor" />}</button><button aria-label={t('Следующий трек', 'Next track')} onClick={() => void onAction('next')}><SkipForward size={24} fill="currentColor" /></button><button className={state.repeat !== 'Off' ? 'liked' : ''} aria-label={t('Повтор', 'Repeat') + ': ' + state.repeat} onClick={() => void onAction('repeat')}><Repeat2 size={20} />{state.repeat === 'One' && <span className="repeat-one">1</span>}</button><button className={state.abStartMs != null ? 'liked now-playing-ab' : 'now-playing-ab'} aria-label={state.abEndMs != null ? t('Сбросить повтор A–B', 'Clear A–B loop') : state.abStartMs != null ? t('Установить точку B', 'Set point B') : t('Установить точку A', 'Set point A')} title={state.abEndMs != null ? `${duration(state.abStartMs)}–${duration(state.abEndMs)}` : undefined} onClick={() => void onAction('ab_loop')}>{state.abEndMs != null ? 'A–B' : state.abStartMs != null ? 'B' : 'A'}</button><button className={disliked ? 'liked' : ''} aria-label={disliked ? t('Вернуть в рекомендации', 'Allow in recommendations') : t('Не рекомендовать этот трек', 'Do not recommend this track')} onClick={onDislike}><ThumbsDown size={19} fill={disliked ? 'currentColor' : 'none'} /></button><button aria-label={t('Открыть очередь', 'Open queue')} onClick={onQueue}><ListMusic size={20} /></button><div className="now-playing-volume"><button aria-label={volumePercent ? t('Выключить звук', 'Mute') : t('Включить звук', 'Unmute')} onClick={() => void onAction('volume', volumePercent ? 0 : .5)}>{volumePercent === 0 ? <VolumeX size={20} /> : volumePercent < 50 ? <Volume1 size={20} /> : <Volume2 size={20} />}</button><input type="range" min={0} max={100} value={volumePercent} aria-label={t('Громкость', 'Volume')} onChange={event => { const next = Number(event.target.value); setVolume(next); void onAction('volume', next / 100) }} onPointerUp={() => setVolume(null)} /><output>{volumePercent}%</output></div></div></div>}</div>
    {panel && <div className="now-playing-resize" role="separator" tabIndex={0} aria-orientation="vertical" aria-label={t('Размер правой панели', 'Right panel width')} aria-valuemin={38} aria-valuemax={76} aria-valuenow={Math.round(split)} onPointerDown={event => { resizing.current = true; event.currentTarget.setPointerCapture(event.pointerId) }} onPointerMove={event => { if (resizing.current) updateSplit(event.clientX) }} onPointerUp={event => { if (!resizing.current) return; resizing.current = false; event.currentTarget.releasePointerCapture(event.pointerId); localStorage.setItem('fastcloud:now-playing-split', String(split)) }} onPointerCancel={() => { resizing.current = false }} onKeyDown={event => { if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return; event.preventDefault(); const next = Math.max(38, Math.min(76, split + (event.key === 'ArrowRight' ? 2 : -2))); setSplit(next); localStorage.setItem('fastcloud:now-playing-split', String(next)) }} />}{panel && <aside className="now-playing-side"><div className="now-playing-tabs" role="tablist" aria-label={t('Дополнительно', 'Details')}><button role="tab" aria-selected={panel === 'lyrics'} className={panel === 'lyrics' ? 'active' : ''} onClick={() => setPanel('lyrics')}><AudioLines size={16} /> {t('Текст', 'Lyrics')}</button><button role="tab" aria-selected={panel === 'comments'} className={panel === 'comments' ? 'active' : ''} onClick={() => setPanel('comments')}><MessageCircle size={16} /> {t('Комментарии', 'Comments')}</button></div>
      {panel === 'lyrics' ? <>{record && <div className="now-playing-lyrics-source"><LyricsSource record={record} english={!!english} /></div>}<div ref={lyricsScroll} className={`now-playing-side-scroll ${synced.length > 0 && settings?.lyrics_auto_scroll !== false ? 'auto-scroll' : ''}`} role="tabpanel">{lyrics.isPending ? <p className="now-playing-message"><LoaderCircle className="spin" size={20} /> {t('Ищем текст…', 'Finding lyrics…')}</p> : lyrics.isError ? <div className="now-playing-message error-text"><p>{t('Источник текста временно недоступен.', 'The lyrics source is temporarily unavailable.')} {String(lyrics.error)}</p><button className="secondary-button" onClick={() => void lyrics.refetch()}>{t('Повторить', 'Retry')}</button></div> : record?.instrumental ? <p className="now-playing-message">{t('Инструментальный трек', 'Instrumental track')}</p> : synced.length ? <div className="now-playing-lyrics-lines">{synced.map((line, index) => <button key={`${line.time}-${index}`} ref={index === currentLine ? activeLine : undefined} className={index === currentLine ? 'active' : index < currentLine ? 'past' : ''} onClick={() => void onAction('seek', line.time)} title={t('Перейти к строке', 'Jump to line')}>{line.text || '♪'}</button>)}</div> : record?.plainLyrics ? <div className="now-playing-lyrics-plain">{record.plainLyrics}</div> : <LyricsFallback english={!!english} />}{record && <p className="now-playing-attribution">{record.trackName} · {record.artistName}</p>}</div></>
      : <div className="now-playing-side-scroll" role="tabpanel"><div className="now-playing-panel-heading"><div><small>SOUNDCLOUD</small><h2>{t('Комментарии', 'Comments')}</h2></div><span className="now-playing-comment-count">{comments.data?.status === 'ready' ? comments.data.data.length : track.comment_count || 0}</span></div><form className="now-playing-comment-form" onSubmit={event => { event.preventDefault(); void post() }}><textarea value={body} onChange={event => setBody(event.target.value)} placeholder={t('О чём думаешь?', 'What do you think?')} aria-label={t('Комментарий', 'Comment')} /><div><label><input type="checkbox" checked={atTime} onChange={event => setAtTime(event.target.checked)} /> {t('На текущей секунде', 'At current time')}</label><button type="submit" disabled={!body.trim() || posting} aria-label={t('Отправить', 'Post')}><Send size={16} /></button></div></form>{postError && <p className="error-text">{postError}</p>}{!comments.data || comments.data.status === 'loading' ? <p className="now-playing-message"><LoaderCircle className="spin" size={20} /> {t('Загружаем комментарии…', 'Loading comments…')}</p> : comments.data.status === 'failed' ? <p className="now-playing-message error-text">{comments.data.data}</p> : comments.data.status === 'unavailable' ? <p className="now-playing-message">{t('Войди в SoundCloud, чтобы читать комментарии.', 'Sign in to SoundCloud to read comments.')}</p> : comments.data.data.length ? <div className="now-playing-comment-list">{comments.data.data.map(comment => <article key={comment.id}><div className="now-playing-comment-avatar">{comment.user?.avatar_url ? <RemoteImage src={comment.user.avatar_url} pixels={200} alt="" loading="lazy" /> : <Music2 size={17} />}</div><div><div className="now-playing-comment-top"><strong>{comment.user?.username || t('Слушатель', 'Listener')}</strong>{comment.timestamp_ms != null && <button onClick={() => void onAction('seek', comment.timestamp_ms!)}>{duration(comment.timestamp_ms)}</button>}</div><p>{comment.body}</p></div></article>)}</div> : <p className="now-playing-message">{t('Комментариев пока нет.', 'No comments yet.')}</p>}</div>}</aside>}
    </div>
  </section>
}
