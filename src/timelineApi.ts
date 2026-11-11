import { invoke } from "@tauri-apps/api/core";
import { isDesktop } from "./api";
import { getClassificationWorkspace, type StatCategory } from "./classificationApi";
import type { TimelineItem } from "./types";
import { intersectRanges, itemRange, normalizeRanges, rangeDuration, type TimeRange } from "./timelineSelection";
export type TimelineData={items:TimelineItem[];categories:StatCategory[];undoAvailable:boolean};
export type AnnotationResult={changedMilliseconds:number;undoAvailable:boolean};
type PreviewAnnotation={ranges:TimeRange[];category:string|null};
const changes:PreviewAnnotation[]=[];
let previous:PreviewAnnotation[]|null=null;
function previewItems(start:number):TimelineItem[]{const minute=60000;return [
  [1,9*60,25,"Microsoft Edge","msedge.exe","数学课程 · 第一节","未分类","#32776f"],
  [2,9*60+25,35,"Microsoft Edge","msedge.exe","数学课程 · 第二节","未分类","#32776f"],
  [3,10*60,40,"Visual Studio Code","code.exe","project.ts · 笔记工具","工作","#537da2"],
  [4,10*60+40,20,"离开","idle","","未分类","#b2bdb8"],
  [5,11*60,40,"Drawboard PDF","db.app.exe","数学讲义.pdf","学习","#9873a5"],
  [6,14*60,45,"Microsoft Edge","msedge.exe","英语精读 · 视频","未分类","#32776f"],
  [7,15*60,20,"微信","wechat.exe","微信","未分类","#ad7b41"],
].map(([id,offset,length,appName,target,title,category,color])=>({id:Number(id),startedAt:start+Number(offset)*minute,endedAt:start+(Number(offset)+Number(length))*minute,seconds:Number(length)*60,idle:target==="idle",appName:String(appName),target:String(target),title:String(title),category:String(category),color:String(color)}));}
async function previewData(start:number,end:number):Promise<TimelineData>{
  const catalog=await getClassificationWorkspace({kind:"application",limit:1});
  const categories=catalog.categories.length?catalog.categories:[{id:1001,name:"学习",color:"#32776f",applicationCount:0,websiteCount:0},{id:1002,name:"工作",color:"#537da2",applicationCount:0,websiteCount:0},{id:1003,name:"娱乐",color:"#ad7b41",applicationCount:0,websiteCount:0}];
  const items:TimelineItem[]=[];
  for(const item of previewItems(start)){const cuts=new Set([item.startedAt,item.endedAt]);for(const c of changes)for(const r of c.ranges){if(r.start>item.startedAt&&r.start<item.endedAt)cuts.add(r.start);if(r.end>item.startedAt&&r.end<item.endedAt)cuts.add(r.end);}const points=[...cuts].sort((a,b)=>a-b);for(let i=0;i<points.length-1;i++){const a=points[i],b=Math.min(end,points[i+1]);if(a>=b)continue;const annotation=[...changes].reverse().find(c=>c.ranges.some(r=>r.start<=a&&r.end>=b));items.push({...item,startedAt:a,endedAt:b,milliseconds:b-a,seconds:(b-a)/1000,category:annotation?.category??item.category});}}
  return {items,categories,undoAvailable:previous!==null};
}
export async function getTimelineWorkspace(start:number,end:number):Promise<TimelineData>{return isDesktop()?invoke("get_timeline_workspace",{start,end}):previewData(start,end);}
export async function annotateTimeline(ranges:TimeRange[],categoryId:number|null,onlyUnclassified:boolean):Promise<AnnotationResult>{
  if(isDesktop())return invoke("annotate_timeline",{ranges,categoryId,onlyUnclassified});
  const normalized=normalizeRanges(ranges);if(!normalized.length)throw Error("请先选择时间");const day=new Date(normalized[0].start);day.setHours(0,0,0,0);const data=await previewData(day.getTime(),day.getTime()+86400000);const eligible=data.items.filter(i=>!i.idle&&(!onlyUnclassified||(i.category||"未分类")==="未分类"));const actual=intersectRanges(normalized,eligible.map(itemRange));const category=categoryId===null?null:categoryId===0?"未分类":data.categories.find(c=>c.id===categoryId)?.name;if(category===undefined)throw Error("分类已不存在");if(actual.length){previous=changes.map(c=>({...c,ranges:c.ranges.map(r=>({...r}))}));changes.push({ranges:actual,category});}return {changedMilliseconds:rangeDuration(actual),undoAvailable:previous!==null};
}
export async function undoTimelineAnnotation():Promise<{restoredMilliseconds:number}>{if(isDesktop())return invoke("undo_timeline_annotation");if(previous===null)throw Error("没有可撤销的标注");const restoredMilliseconds=rangeDuration(changes[changes.length-1]?.ranges||[]);changes.splice(0,changes.length,...previous);previous=null;return {restoredMilliseconds};}
