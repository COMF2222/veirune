import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Check, Star, Trash2 } from 'lucide-react'
import { api } from './api'
import { TrackRows } from './App'
import type { CommunityRole, CommunityHome } from './communityTypes'

function useIdentity() {
  const {data} = useQuery({queryKey:['my-profile'],queryFn:api.myProfile})
  return data?.status === 'ready' ? data.data.id : null
}
function useRole(){
  const id=useIdentity()
  return useQuery({queryKey:['community-role',id],queryFn:()=>api.community<CommunityRole>('role'),enabled:id!==null||api.preview,retry:false,staleTime:30000,refetchInterval:60000})
}
export function EditorialButton({id,english=false}: {id:number;english?:boolean}) {
  const {data}=useRole(),client=useQueryClient(),[busy,setBusy]=useState(false),[error,setError]=useState('')
  if(!data?.canEdit) return null
  const picked=data.pickedIds.includes(id)
  const label=picked?(english?'Remove editorial pick':'Убрать из выбора редакции'):(english?'Add editorial pick':'В выбор редакции')
  return <span className="editorial-button-wrap"><button className="icon-button" disabled={busy} aria-pressed={picked} aria-label={label} title={label} onClick={()=>{
    setBusy(true);setError('');void api.community('pick',{trackId:id,active:!picked}).then(()=>Promise.all([client.invalidateQueries({queryKey:['community-role']}),client.invalidateQueries({queryKey:['community-home']})])).catch(()=>{setError(english?'Could not update your pick. Check permissions and the 10-track limit.':'Не удалось изменить выбор. Проверь права и лимит в 10 треков.');void client.invalidateQueries({queryKey:['community-role']})}).finally(()=>setBusy(false))
  }}>{picked?<Check size={18}/>:<Star size={18}/>}</button>{error&&<span role="alert" className="editorial-error">{error}</span>}</span>
}
export function CommunityHomeSection({english=false}: {english?:boolean}) {
  const t=(ru:string,en:string)=>english?en:ru,id=useIdentity(),client=useQueryClient()
  const {data,error,isPending,refetch}=useQuery({queryKey:['community-home',id],queryFn:()=>api.community<CommunityHome>('home'),enabled:id!==null||api.preview,retry:false,staleTime:60000,refetchInterval:300000})
  const [period,setPeriod]=useState<'today'|'yesterday'|'week'>('today'),[busy,setBusy]=useState(false),[message,setMessage]=useState('')
  const rows=data?.[period]||[],picks=data?.editorial||[]
  const editablePicks=data?.role.canEdit?picks.filter(r=>r.pickedByMe||data.role.isOwner):[]
  return <section className="community-home" aria-label={t('Музыка сообщества','Community music')} aria-busy={isPending}>
    <h2>{t('Слушают в Veirune','Popular in Veirune')}</h2>
    <div className="tabs" role="group" aria-label={t('Период подборки','Discovery period')}>{([['today','Сегодня','Today'],['yesterday','Вчера','Yesterday'],['week','Неделя','Week']] as const).map(([key,ru,en])=><button key={key} className={key===period?'active':''} aria-pressed={period===key} onClick={()=>setPeriod(key)}>{t(ru,en)}</button>)}</div>
    <p className="community-description">{t('По числу слушателей','By listeners')}</p>
    {error?<p role="status">{t('Не удалось загрузить подборки.','Could not load collections.')} <button className="text-button" onClick={()=>void refetch()}>{t('Повторить','Retry')}</button></p>:<>
      {rows.length>0&&<TrackRows tracks={rows.map(r=>r.track)} listenerCounts={Object.fromEntries(rows.map(r=>[r.trackId,r.listeners||0]))}/>}
      <div className="community-heading"><h2>{t('Выбор редакции','Editors’ picks')}</h2>{picks.length>0&&<button className="secondary-button" onClick={() => void api.play(picks.map(row=>row.track),0).catch(()=>setMessage(t('Не удалось включить подборку.','Could not play this collection.')))}>{t('Слушать всё','Play all')}</button>}</div>
      {picks.length>0&&<TrackRows tracks={picks.map(r=>r.track)}/>}
      {editablePicks.length>0&&<div className="editorial-manage">{editablePicks.map(row=><button className="secondary-button" disabled={busy} key={row.trackId} onClick={()=>{setBusy(true);void api.community('pick',{trackId:row.trackId,active:false}).then(()=>Promise.all([refetch(),client.invalidateQueries({queryKey:['community-role']})])).catch(()=>setMessage(t('Не удалось убрать трек. Попробуй ещё раз.','Could not remove the track. Try again.'))).finally(()=>setBusy(false))}}><Trash2 size={16}/>{t('Убрать','Remove')}: {row.track.title}</button>)}</div>}
      {message&&<p role="alert">{message}</p>}
    </>}
  </section>
}
