import type { Track } from './types'
export type CommunityRole = {canEdit: boolean; isOwner: boolean; pickedIds: number[]}
export type CommunityEntry = {trackId: number; track: Track; listeners?: number; votes?: number; pickedByMe?: boolean}
export type CommunityHome = {today: CommunityEntry[]; yesterday: CommunityEntry[]; week: CommunityEntry[]; editorial: CommunityEntry[]; role: CommunityRole; day: string; timezone: string}
