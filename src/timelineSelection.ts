import type { TimelineItem } from "./types";
export type TimeRange = { start: number; end: number };
export function normalizeRanges(ranges: TimeRange[]): TimeRange[] {
  const sorted = ranges.filter(r => Number.isFinite(r.start) && Number.isFinite(r.end) && r.end > r.start).map(r => ({ start: Math.round(r.start), end: Math.round(r.end) })).sort((a,b) => a.start-b.start);
  const result: TimeRange[] = [];
  for (const r of sorted) { const last = result[result.length-1]; if (last && r.start <= last.end) last.end = Math.max(last.end,r.end); else result.push({...r}); }
  return result;
}
export function intersectRanges(left: TimeRange[], right: TimeRange[]): TimeRange[] {
  const a = normalizeRanges(left), b = normalizeRanges(right), result: TimeRange[]=[];
  let i=0,j=0;
  while(i<a.length && j<b.length) { const start=Math.max(a[i].start,b[j].start),end=Math.min(a[i].end,b[j].end); if(end>start)result.push({start,end}); if(a[i].end<b[j].end)i++;else j++; }
  return result;
}
export function subtractRanges(left: TimeRange[], right: TimeRange[]): TimeRange[] {
  const a=normalizeRanges(left), b=normalizeRanges(right), result:TimeRange[]=[];let j=0;
  for(const r of a){let cursor=r.start;while(j<b.length && b[j].end<=cursor)j++;for(let k=j;k<b.length && b[k].start<r.end;k++){if(b[k].start>cursor)result.push({start:cursor,end:Math.min(r.end,b[k].start)});cursor=Math.max(cursor,b[k].end);if(cursor>=r.end)break;}if(cursor<r.end)result.push({start:cursor,end:r.end});}
  return result;
}
export const rangeDuration=(ranges:TimeRange[])=>normalizeRanges(ranges).reduce((n,r)=>n+r.end-r.start,0);
export const itemRange=(item:TimelineItem):TimeRange=>({start:item.startedAt,end:item.endedAt});
export type TimelineBlock=TimeRange & {name:string;target:string;color:string;idle:boolean;items:number;category:string};
export function mergeTimeline(items:TimelineItem[],mode:"application"|"category",colors:Map<string,string>):TimelineBlock[]{
  const result:TimelineBlock[]=[];
  for(const item of items){const category=item.category||"未分类",target=mode==="category"?category:item.target||item.appName;const last=result[result.length-1];if(last && last.end===item.startedAt && last.target===target && last.idle===item.idle){last.end=item.endedAt;last.items++;continue;}result.push({start:item.startedAt,end:item.endedAt,name:item.idle?"闲置":mode==="category"?category:item.appName,target,color:mode==="category"?colors.get(category)||"#88938f":item.color||"#32776f",idle:item.idle,items:1,category});}
  return result;
}
export function blockAt(blocks:TimelineBlock[],time:number):TimelineBlock|null{let l=0,r=blocks.length;while(l<r){const m=(l+r)>>>1;if(blocks[m].start<=time)l=m+1;else r=m;}const block=blocks[l-1];return block && time<block.end?block:null;}
