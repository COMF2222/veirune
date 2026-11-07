import { useEffect, useRef } from 'react'
import { useQuery } from '@tanstack/react-query'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { api } from './api'
import type { WebPlaybackView } from './types'
import type Hls from 'hls.js'
import { licenseChallenge, licenseResponse } from './webDrm'

/** A second audio backend, controlled by the same native queue and UI. */
export function useWebPlayback() {
  const { data: view } = useQuery({
    queryKey: ['player'], queryFn: api.player, enabled: !api.preview,
    select: state => state.webPlayback,
    // This bridge also owns playback while the existing window is in the tray.
    refetchInterval: query => query.state.data?.isPlaying || query.state.data?.loading ? 500 : 1500,
    refetchIntervalInBackground: true,
  })
  const latest = useRef<WebPlaybackView | null | undefined>(view)
  latest.current = view
  const active = useRef<{ generation: number; audio: HTMLAudioElement; apply: () => void } | null>(null)

  useEffect(() => {
    if (!view) return
    const generation = view.generation
    const audio = new Audio()
    audio.preload = 'auto'
    let hls: Hls | null = null, disposed = false, terminal = false, seekRevision = -1, lastReport = 0, desiredPlaying: boolean | null = null
    const base = new URL('/', convertFileSrc('playback', 'veirune-media'))
    const url = (path: string) => new URL(path, base).href
    const report = (event: string) => {
      if (disposed || latest.current?.generation !== generation) return
      void invoke('web_playback_report', { generation, event, seekRevision: Math.max(0,seekRevision), positionMs: Number.isFinite(audio.currentTime) ? Math.max(0, Math.round(audio.currentTime * 1000)) : 0 }).catch(() => {})
    }
    const fail = (event = 'failed') => { if (!terminal && !disposed) { terminal = true; audio.pause(); report(event) } }
    const apply = () => {
      const state = latest.current
      if (!state || state.generation !== generation || disposed) return
      audio.volume = Math.max(0, Math.min(1, state.volume))
      audio.playbackRate = Math.max(.5, Math.min(2, state.speed))
      if (audio.readyState >= 1 && seekRevision !== state.seekRevision) {
        seekRevision = state.seekRevision
        if (Math.abs(audio.currentTime * 1000 - state.seekMs) > 200) audio.currentTime = Math.max(0, Math.min(state.seekMs / 1000, audio.duration - .001))
      }
      if (state.playing !== desiredPlaying) {
        desiredPlaying = state.playing
        if (state.playing) void audio.play().catch(error => {
          if (disposed || latest.current?.generation !== generation || error?.name === 'AbortError') return
          if (error?.name === 'NotAllowedError') { desiredPlaying = false; report('blocked') } else fail()
        })
        else audio.pause()
      }
    }
    active.current = { generation, audio, apply }
    audio.addEventListener('loadedmetadata', apply)
    audio.addEventListener('canplay', () => report('ready'))
    audio.addEventListener('playing', () => report('playing'))
    audio.addEventListener('timeupdate', () => {
      if (audio.readyState >= 3 && performance.now() - lastReport >= 250) { lastReport = performance.now(); report('progress') }
    })
    audio.addEventListener('ended', () => { if (!terminal) { terminal = true; report('ended') } })
    audio.addEventListener('error', () => fail())
    void import('hls.js').then(({ default: HlsPlayer }) => {
      if (disposed || latest.current?.generation !== generation) return
      if (!HlsPlayer.isSupported()) { fail(); return }
      const licenses = view.source.license_paths
      const drmSystems = Object.fromEntries(Object.entries(licenses).map(([system, path]) => [system, {
        licenseUrl: url(path), ...(system === 'com.apple.fps' ? { serverCertificateUrl: url(path) } : {}),
      }]))
      hls = new HlsPlayer({ enableWorker: false, emeEnabled: Object.keys(licenses).length > 0, drmSystems,
        maxBufferLength: 20, maxMaxBufferLength: 30, backBufferLength: 10,
        licenseXhrSetup: (xhr, _url, context, challenge) => {
          xhr.setRequestHeader('Content-Type', context.keySystem === 'com.apple.fps' ? 'application/json' : 'application/octet-stream')
          return licenseChallenge(context.keySystem, challenge, context.decryptdata.uri)
        },
        licenseResponseCallback: (xhr, _url, context) => licenseResponse(context.keySystem, xhr.response),
      })
      hls.on(HlsPlayer.Events.ERROR, (_event, details) => {
        if (!details.fatal || disposed) return
        fail(details.response?.code === 429 || details.networkDetails?.status === 429 ? 'rate_limited' : 'failed')
      })
      hls.on(HlsPlayer.Events.MANIFEST_PARSED, () => { desiredPlaying = null; apply() })
      hls.attachMedia(audio)
      hls.loadSource(url(view.source.playlist_path))
      apply()
    }).catch(() => fail())
    return () => {
      disposed = true
      audio.pause()
      hls?.destroy()
      audio.removeAttribute('src')
      audio.load()
      if (active.current?.generation === generation) active.current = null
    }
  }, [view?.generation])

  useEffect(() => { active.current?.apply() }, [view?.generation, view?.playing, view?.volume, view?.speed, view?.seekRevision])
}
