import { AppIconPicker } from './AppIconPicker'
import { useEffect, useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { api } from './api'
import type { Settings } from './types'
import { ThemePresets } from './AppImprovements'
import { PlaybackOptions } from './PersonalPages'
import { ThemeSettings } from './ThemeSettings'
import { ColorPicker } from './ColorPicker'
import { ImportRecovery } from './ImportRecovery'
import { SpotifyImport } from './SpotifyImport'

const sizeLabel = (bytes: number) => bytes >= 1073741824 ? `${(bytes / 1073741824).toFixed(2)} GiB` : `${(bytes / 1048576).toFixed(1)} MiB`

export type SettingsSection = 'general' | 'appearance' | 'sound' | 'integrations' | 'storage' | 'account' | 'users'

export function SettingsSections({ section, settings, update, showDeveloperSettings }: { section: SettingsSection; settings: Settings; update: (key: string, value: unknown) => Promise<void>; showDeveloperSettings: boolean }) {
  const queryClient = useQueryClient()
  const [background, setBackground] = useState('')
  const [discord, setDiscord] = useState('')
  const [token, setToken] = useState('')
  const [checkingToken, setCheckingToken] = useState(false)
  const [tokenCheck, setTokenCheck] = useState('')
  const [message, setMessage] = useState('')
  const [accentDraft, setAccentDraft] = useState('')
  const lastAccentCommit = useRef('')
  const { data: importState } = useQuery({ queryKey: ['yandex-import'], queryFn: api.importStatus, enabled: section === 'integrations', refetchInterval: section === 'integrations' ? 1000 : false })
  const importWasRunning = useRef(false)
  useEffect(() => {
    if (importWasRunning.current && !importState?.running) {
      void queryClient.invalidateQueries({ queryKey: ['playlists'] })
      void queryClient.invalidateQueries({ queryKey: ['settings'] })
    }
    importWasRunning.current = !!importState?.running
  }, [importState?.running, queryClient])
  const { data: player } = useQuery({ queryKey: ['player'], queryFn: api.player, enabled: section === 'sound', refetchInterval: section === 'sound' ? 1000 : false })
  const { data: offline = [] } = useQuery({ queryKey: ['offline-tracks'], queryFn: api.offlineTracks, enabled: section === 'storage' })
  const { data: storage, refetch: refetchStorage } = useQuery({ queryKey: ['storage-report'], queryFn: api.storageReport, enabled: section === 'storage' })
  useEffect(() => {
    setBackground(settings.background_image?.startsWith('https://') ? settings.background_image : '')
    setDiscord(settings.discord_client_id)
  }, [settings.background_image, settings.discord_client_id])
  const choose = async (kind: 'background' | 'font') => {
    try {
      const path = await ({ background: api.pickBackground, font: api.pickFont }[kind])()
      if (!path) return
      await ({ background: api.saveBackground, font: api.saveFont }[kind])(path)
      await queryClient.invalidateQueries({ queryKey: ['settings'] })
      if (kind === 'font') window.dispatchEvent(new Event('fastcloud:font-changed'))
      if (kind === 'background') window.dispatchEvent(new Event('fastcloud:background-changed'))
      setMessage(settings.language === 'English' ? 'File applied' : 'Файл установлен')
    } catch (error) { setMessage(String(error)) }
  }
  const applyPerformance = async (profile: Settings['memory_profile']) => {
    await update('memory_profile', profile)
    await update('reduced_motion', profile === 'Eco')
  }
  const applyBackground = async () => {
    const raw = background.trim()
    if (!raw) { await update('background_image', null); return }
    try {
      const url = new URL(raw)
      if (url.protocol === 'http:') url.protocol = 'https:'
      if (url.protocol !== 'https:') throw new Error(settings.language === 'English' ? 'HTTPS URL required' : 'Нужна ссылка HTTPS')
      await update('background_image', url.toString())
      setMessage(settings.language === 'English' ? 'Background image applied' : 'Фон по ссылке установлен')
    } catch (error) { setMessage(`${settings.language === 'English' ? 'Could not apply background' : 'Не удалось применить фон'}: ${String(error)}`) }
  }
  const color = `#${settings.accent_rgb.map(value => Math.max(0, Math.min(255, value)).toString(16).padStart(2, '0')).join('')}`
  const pickedColor = accentDraft || color
  const t = (ru: string, en: string) => settings.language === 'English' ? en : ru
  const commitAccent = (picked = accentDraft) => {
    if (picked && picked !== color && picked !== lastAccentCommit.current) {
      lastAccentCommit.current = picked
      void update('accent_rgb', [1, 3, 5].map(index => parseInt(picked.slice(index, index + 2), 16)))
    }
    setAccentDraft('')
  }

  if (section === 'general') return <div className="settings-card"><h3>{t('Общее', 'General')}</h3><label className="setting-row"><span>{t('Показывать популярное и выбор редакции', 'Show popular music and editors’ picks')}</span><input type="checkbox" checked={settings.show_community !== false} onChange={event => void update('show_community', event.target.checked)} /></label><div className="form-grid"><label className="field-label">{t('Язык', 'Language')}<select value={settings.language} onChange={event => void update('language', event.target.value)}><option value="Russian">Русский</option><option value="English">English</option></select></label><label className="field-label">{t('Стартовая страница', 'Start page')}<select value={settings.startup_page} onChange={event => void update('startup_page', event.target.value)}><option value="Home">{t('Главная', 'Home')}</option><option value="Search">{t('Поиск', 'Search')}</option><option value="Library">{t('Библиотека', 'Library')}</option><option value="Settings">{t('Настройки', 'Settings')}</option></select></label></div><label className="setting-row"><span><strong>{t('Закрывать в трей', 'Close to tray')}</strong><small>{t('После нажатия ✕ музыка продолжит играть. Открыть окно или выйти можно через значок в трее.', 'After clicking ✕, music keeps playing. Reopen or quit from the tray icon.')}</small></span><input type="checkbox" checked={settings.close_to_tray} onChange={event => void update('close_to_tray', event.target.checked)} /></label><label className="setting-row"><span><strong>{t('Компактный список треков', 'Compact track list')}</strong></span><input type="checkbox" checked={settings.compact_rows} onChange={event => void update('compact_rows', event.target.checked)} /></label><label className="setting-row"><span><strong>{t('Номера треков', 'Track numbers')}</strong></span><input type="checkbox" checked={settings.show_track_numbers} onChange={event => void update('show_track_numbers', event.target.checked)} /></label></div>

  if (section === 'appearance') return <>
    <AppIconPicker english={settings.language === 'English'} />
    <div className="settings-card"><h3>{t('Тема', 'Theme')}</h3><div className="form-grid"><label className="field-label">{t('Тема', 'Theme')}<select value={settings.theme} onChange={event => void update('theme', event.target.value)}><option value="Dark">{t('Тёмная', 'Dark')}</option><option value="Light">{t('Светлая', 'Light')}</option><option value="System">{t('Как в системе', 'System')}</option></select></label><label className="field-label color-picker-field"><span>{t('Акцент', 'Accent')}</span><ColorPicker value={pickedColor} label={t('Акцент', 'Accent')} english={settings.language === 'English'} onChange={value => { setAccentDraft(value); document.documentElement.style.setProperty('--accent', value) }} onCommit={commitAccent} /></label></div></div>
    <ThemePresets settings={settings} /><ThemeSettings settings={settings} update={update} />
    <div className="settings-card"><h3>{t('Текст песен', 'Lyrics')}</h3><label className="field-label"><span>{t('Размер текста', 'Text size')} · {Math.round(settings.lyrics_scale * 100)}%</span><input type="range" min="0.8" max="1.5" step="0.05" value={settings.lyrics_scale} onChange={event => void update('lyrics_scale', Number(event.target.value))} /></label><label className="setting-row"><span><strong>{t('Размывать неактивные строки', 'Blur inactive lines')}</strong><small>{t('Текущая строка яркая; прошлые и следующие строки затемняются и размываются.', 'Keep the current line bright; dim and blur past and upcoming lines.')}</small></span><input type="checkbox" checked={settings.lyrics_blur_past} onChange={event => void update('lyrics_blur_past', event.target.checked)} /></label><label className="setting-row"><span><strong>{t('Автопрокрутка текста', 'Follow current line')}</strong><small>{t('Текст следует за музыкой. Выключи, чтобы листать его самостоятельно.', 'Lyrics follow the music. Turn off to scroll them yourself.')}</small></span><input type="checkbox" checked={settings.lyrics_auto_scroll} onChange={event => void update('lyrics_auto_scroll', event.target.checked)} /></label></div>
    <div className="settings-card"><h3>{t('Фоновое изображение', 'Background image')}</h3><div className="inline-form"><button className="secondary-button" disabled={api.preview} onClick={() => void choose('background')}>{t('Загрузить файл…', 'Choose file…')}</button>{settings.background_image && <button className="secondary-button" onClick={() => void update('background_image', null)}>{t('Убрать фон', 'Remove background')}</button>}</div><label className="field-label">{t('По ссылке', 'From URL')}<div className="inline-form"><input value={background} onChange={event => setBackground(event.target.value)} placeholder="https://…" aria-label={t('URL фонового изображения', 'Background image URL')} /><button className="secondary-button" onClick={() => void applyBackground()}>{t('Применить', 'Apply')}</button></div></label><p className="muted">{t('Поддерживаются изображения, GIF и анимированные WebP.', 'Supports images, GIFs and animated WebP.')}</p>{settings.background_image && <div className="form-grid"><label className="field-label"><span>{t('Затемнение фона', 'Background dimming')}</span><input type="range" min="0" max="0.85" step="0.05" value={settings.background_dim} onChange={event => void update('background_dim', Number(event.target.value))} /></label><label className="field-label"><span>{t('Затемнение краёв', 'Edge vignette')}</span><input type="range" min="0" max="0.7" step="0.05" value={settings.background_opacity} onChange={event => void update('background_opacity', Number(event.target.value))} /></label><label className="field-label"><span>{t('Размытие', 'Blur')}</span><input type="range" min="0" max="40" step="1" value={settings.background_blur} onChange={event => void update('background_blur', Number(event.target.value))} /></label></div>}</div>
    {settings.background_image && <div className="settings-card"><h3>{t('Видимость фона', 'Wallpaper visibility')}</h3><p>{t('Уменьши подложку, чтобы лучше видеть фон.', 'Reduce the overlay to make the wallpaper more visible.')}</p><label className="field-label"><span>{t('Подложка', 'Overlay')} · {Math.round(settings.background_overlay * 100)}%</span><input type="range" min="0" max="1" step="0.05" value={settings.background_overlay} onChange={event => void update('background_overlay', Number(event.target.value))} /></label><button className="secondary-button" onClick={() => void (async () => { await update('background_overlay', 0); await update('background_dim', 0); await update('background_opacity', 0) })()}>{t('Показать фон без затемнения', 'Show wallpaper without dimming')}</button></div>}
    <div className="settings-card"><h3>{t('Производительность', 'Performance')}</h3><div className="performance-options">{([['Eco', t('Light', 'Light'), t('Меньше эффектов и расхода ресурсов', 'Fewer effects, lower resource use')], ['Balanced', t('Средний', 'Balanced'), t('Баланс эффектов и скорости', 'A balance of effects and speed')], ['Quality', t('Полный', 'Full'), t('Все эффекты и чёткие обложки', 'Full effects and sharp artwork')]] as const).map(([value, label, detail]) => <button key={value} className={settings.memory_profile === value ? 'active' : ''} onClick={() => void applyPerformance(value)}><strong>{label}</strong><small>{detail}</small></button>)}</div></div>
    <div className="settings-card"><h3>{t('Шрифт', 'Font')}</h3><div className="inline-form"><button className="secondary-button" disabled={api.preview} onClick={() => void choose('font')}>{t('Выбрать файл…', 'Choose file…')}</button>{settings.interface_font && <button className="secondary-button" onClick={() => void update('interface_font', null)}>{t('Стандартный шрифт', 'Default font')}</button>}</div></div>{message && <p role="status" className="muted">{message}</p>}
  </>

  if (section === 'sound') return <><PlaybackOptions settings={settings} update={update} />
    <div className="settings-card"><h3>{t('Воспроизведение', 'Playback')}</h3><label className="setting-row"><span><strong>{t('Автовоспроизведение', 'Autoplay')}</strong><small>{t('Подбирать треки после конца очереди', 'Find similar tracks when the queue ends')}</small></span><input type="checkbox" checked={settings.autoplay} onChange={event => void update('autoplay', event.target.checked)} /></label><label className="setting-row"><span><strong>{t('Моно', 'Mono')}</strong></span><input type="checkbox" checked={settings.mono} onChange={event => void update('mono', event.target.checked)} /></label><label className="setting-row"><span><strong>{t('Пресеты EQ для треков', 'Per-track EQ presets')}</strong><small>{t('Использует сохранённый эквалайзер для каждого трека.', 'Uses the saved equalizer preset for each track.')}</small></span><input type="checkbox" checked={settings.eq_auto} onChange={event => void update('eq_auto', event.target.checked)} /></label>{player?.current != null && <div className="inline-form"><button className="secondary-button" onClick={() => void api.eqPreset().then(() => setMessage(t('Пресет сохранён для текущего трека', 'Preset saved for this track'))).catch(error => setMessage(String(error)))}>{t('Сохранить EQ для трека', 'Save EQ for track')}</button><button className="secondary-button" onClick={() => void api.eqPreset(true).then(() => setMessage(t('Пресет трека удалён', 'Track preset removed'))).catch(error => setMessage(String(error)))}>{t('Удалить пресет', 'Remove preset')}</button></div>}</div>
    <div className="settings-card"><h3>{t('Мини-плеер', 'Mini player')}</h3><p className="muted">{t('Компактное окно Airwave. Переключайся кнопкой в плеере или Ctrl+M.', 'Compact Airwave window. Use the player button or Ctrl+M to switch.')}</p><label className="setting-row"><span><strong>{t('Поверх окон', 'Always on top')}</strong></span><input type="checkbox" checked={settings.winamp_on_top} onChange={event => void update('winamp_on_top', event.target.checked)} /></label></div>
    {message && <p role="status" className="muted">{message}</p>}
  </>

  if (section === 'integrations') return <>
    <ImportRecovery english={settings.language === 'English'} />
    <SpotifyImport english={settings.language === 'English'} />
    <div className="settings-card">
      <h3>Discord Rich Presence</h3>
      <p className="muted">{t('Показывай текущий трек в Discord на этом компьютере.', 'Show the current track in Discord on this computer.')}</p>
      <label className="setting-row"><span>{t('Показывать текущий трек', 'Show current track')}</span><input type="checkbox" disabled={!settings.discord_client_id} checked={settings.discord_presence} onChange={event => void update('discord_presence', event.target.checked)} /></label>
      {!settings.discord_client_id && <p className="muted">{t('В этой сборке не настроено подключение к Discord.', 'Discord is not configured in this build.')}</p>}
      {showDeveloperSettings && <details className="advanced-setting"><summary>{t('Настройка для разработчика', 'Developer setting')}</summary><label className="field-label">Application ID<div className="inline-form"><input value={discord} onChange={event => setDiscord(event.target.value)} placeholder="Application ID" /><button className="secondary-button" onClick={() => void update('discord_client_id', discord.trim())}>{t('Сохранить', 'Save')}</button></div></label></details>}
    </div>
    <div className="settings-card">
      <h3>{t('Импорт из Яндекс Музыки', 'Import from Yandex Music')}</h3>
      <p className="muted">{t('Любимые треки ищутся в SoundCloud и собираются в отдельный плейлист.', 'Liked tracks are matched in SoundCloud and added to a separate playlist.')}</p>
      <div className="yandex-token-guide">
        <strong>{t('Как получить токен Яндекс Музыки:', 'How to get a Yandex Music token:')}</strong>
        <p><a href="https://ym.marshal.dev/token/" target="_blank" rel="noreferrer">ym.marshal.dev/token/</a> — {t('открой раздел «Альтернативные способы», выбери подходящий способ и вставь полученный OAuth-токен ниже.', 'open “Alternative methods”, choose a suitable method, then paste the OAuth token below.')}</p>
        <p>{t('Токен даёт доступ к твоей Яндекс Музыке. Вставляй его только в Veirune и никому не отправляй.', 'The token grants access to your Yandex Music account. Paste it only into Veirune and do not share it.')}</p>
      </div>
      <div className="inline-form">
        <input type="password" autoComplete="off" value={token} disabled={checkingToken || importState?.running} onChange={event => { setToken(event.target.value); setTokenCheck('') }} placeholder={t('OAuth токен Яндекс Музыки', 'Yandex Music OAuth token')} aria-label={t('Токен Яндекс Музыки', 'Yandex Music token')} />
        <button className="secondary-button" disabled={!token.trim() || checkingToken || importState?.running} onClick={() => { setCheckingToken(true); setTokenCheck(''); void api.checkYandexToken(token).then(count => setTokenCheck(t(`Токен работает: доступно ${count} лайков.`, `Token works: ${count} liked tracks available.`))).catch(error => setTokenCheck(String(error))).finally(() => setCheckingToken(false)) }}>{checkingToken ? t('Проверяем…', 'Checking…') : t('Проверить токен', 'Check token')}</button>
        <button className="secondary-button" disabled={!token.trim() || checkingToken || importState?.running} onClick={() => void api.startYandexImport(token).then(() => { setToken(''); setTokenCheck(''); setMessage(t('Импорт начался', 'Import started')) }).catch(error => setMessage(String(error)))}>{importState?.running ? t('Импорт идёт…', 'Importing…') : t('Импортировать лайки', 'Import likes')}</button>
      </div>
      {tokenCheck && <p role="status">{tokenCheck}</p>}
      {importState?.running && <p role="status">{importState.current} / {importState.total} · {t('найдено', 'matched')} {importState.matched} · {importState.title}</p>}
      {importState?.message && <p role="status">{importState.message}</p>}
    </div>{message && <p role="status" className="muted">{message}</p>}
  </>

  if (section === 'storage') {
    const reclaimable = (storage?.offlineBytes || 0) + (storage?.audioCacheBytes || 0) + (storage?.artworkCacheBytes || 0) + (storage?.clapPreparationBytes || 0)
    const total = storage ? storage.installationBytes + storage.clapRuntimeBytes + storage.offlineBytes + storage.audioCacheBytes + storage.artworkCacheBytes + storage.otherDataBytes + storage.otherCacheBytes + storage.extraAppDataBytes : 0
    const clearDownloads = async () => {
      if (!window.confirm(t(`Удалить все ${offline.length} офлайн-треков (${sizeLabel(storage?.offlineBytes || 0)})?`, `Remove all ${offline.length} offline tracks (${sizeLabel(storage?.offlineBytes || 0)})?`))) return
      try { await api.clearOfflineTracks(); await queryClient.invalidateQueries({ queryKey: ['offline-tracks'] }); await refetchStorage(); setMessage(t('Офлайн-загрузки удалены.', 'Offline downloads removed.')) }
      catch (error) { setMessage(String(error)) }
    }
    const clearCache = async () => {
      try { await api.audioCache(true); await refetchStorage(); setMessage(t('Временный аудиокэш очищен.', 'Temporary audio cache cleared.')) }
      catch (error) { setMessage(String(error)) }
    }
    const clearArtwork = async () => {
      try { await api.clearArtworkCache(); await refetchStorage(); setMessage(t('Кэш обложек очищен.', 'Artwork cache cleared.')) }
      catch (error) { setMessage(String(error)) }
    }
    const clearClapPreparation = async () => {
      try { await api.clearClapPreparation(); await refetchStorage(); setMessage(t('Временные файлы удалены.', 'Temporary files removed.')) }
      catch (error) { setMessage(String(error)) }
    }
    return <div className="settings-card"><h3>{t('Хранилище', 'Storage')}</h3>
      <div className="storage-stats"><div><strong>{storage ? sizeLabel(total) : '—'}</strong><span>{t('всё, что занимает установленное приложение', 'total installed app footprint')}</span></div><div><strong>{storage ? sizeLabel(reclaimable) : '—'}</strong><span>{t('можно освободить загрузками и кэшем', 'reclaimable downloads and caches')}</span></div><div><strong>{offline.length}</strong><span>{t('офлайн-треков', 'offline tracks')}</span></div></div>
      {storage && <div className="storage-breakdown">{([
        [t('Файлы приложения', 'Application files'), storage.installationBytes - (storage.clapModelBytes - storage.clapRuntimeBytes) - storage.clapPreparationBytes],
        [t('Модель и рабочие файлы CLAP', 'CLAP model and runtime'), storage.clapModelBytes],
        [t('Временные файлы подготовки CLAP', 'CLAP preparation files'), storage.clapPreparationBytes],
        [t('Офлайн-загрузки', 'Offline downloads'), storage.offlineBytes],
        [t('Временный аудиокэш', 'Temporary audio cache'), storage.audioCacheBytes],
        [t('Обложки', 'Artwork cache'), storage.artworkCacheBytes],
        [t('Данные и настройки', 'Data and settings'), storage.otherDataBytes + storage.extraAppDataBytes],
        [t('Другой кэш', 'Other cache'), storage.otherCacheBytes],
      ] as const).map(([label, bytes]) => <div key={label}><span>{label}</span><strong>{sizeLabel(bytes)}</strong></div>)}</div>}
      {storage && <div className="storage-paths"><strong>{t('Где лежат файлы на диске C', 'File locations on drive C')}</strong><span>{t('Установка', 'Installation')}: {storage.installationPath}</span><span>{t('Загрузки и настройки', 'Downloads and settings')}: {storage.dataPath}</span><span>{t('Кэш', 'Cache')}: {storage.cachePath}</span>{storage.extraAppDataBytes > 0 && <span>{t('Дополнительные данные', 'Additional data')}: {storage.extraAppDataPath}</span>}</div>}
      <label className="field-label">{t('Лимит временного аудиокэша', 'Temporary audio cache limit')}<select value={settings.audio_cache_limit_mb} onChange={event => void update('audio_cache_limit_mb', Number(event.target.value))}><option value="0">{t('Выключен', 'Off')}</option><option value="256">256 MiB</option><option value="512">512 MiB</option><option value="1024">1 GiB</option><option value="2048">2 GiB</option><option value="4096">4 GiB</option></select></label>
      <div className="inline-form"><button className="secondary-button" onClick={() => void refetchStorage()}>{t('Обновить размеры', 'Refresh sizes')}</button><button className="secondary-button" disabled={!storage?.audioCacheBytes} onClick={() => void clearCache()}>{t('Очистить аудиокэш', 'Clear audio cache')}</button><button className="secondary-button" disabled={!storage?.artworkCacheBytes} onClick={() => void clearArtwork()}>{t('Очистить обложки', 'Clear artwork')}</button><button className="secondary-button" disabled={!storage?.clapPreparationBytes} onClick={() => void clearClapPreparation()}>{t('Удалить файлы подготовки CLAP', 'Remove CLAP preparation files')}</button><button className="secondary-button danger-button" disabled={!offline.length} onClick={() => void clearDownloads()}>{t('Удалить все загрузки', 'Remove all downloads')}</button></div>
      <p className="muted">{t('Очистка кэша не удаляет скачанные треки. Лайки и плейлисты остаются в SoundCloud.', 'Clearing cache keeps downloaded tracks. Likes and playlists stay on SoundCloud.')}</p>{message && <p role="status" className="error-text">{message}</p>}
    </div>
  }

  return null
}
