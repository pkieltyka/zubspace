import {bestBoostHeading} from './authored_map_geometry.mjs';

// Read-only staging geometry. The measurement needs an open runway from the
// pilot's actual position, not a return to a fixed point on the other side of
// a wall. Every proposed hop must be hull-clear and is still flown with keys.
export function extremeChordStageRoute({geometry, safetyDistance, from,
  requiresRunway, maxDistancePx=512}) {
  if(![from?.x,from?.y,maxDistancePx].every(Number.isFinite)||maxDistancePx<0)
    throw new Error('finite bounded staging origin required');
  const stageAt=(x,y)=>{
    // Extra endpoint margin accommodates real-key settling. The measured
    // runway itself retains the original 512px / 128px safety contract.
    if(!geometry.authoredHullIsTraversable(x,y,30)||safetyDistance(x,y)<=192)
      return null;
    if(!geometry.authoredLineIsClear(from.x,from.y,x,y))return null;
    const heading=bestBoostHeading({hullIsTraversable:geometry.authoredHullIsTraversable,
      safetyDistance,x,y,travelPx:512,clearancePx:128});
    if(requiresRunway&&!heading)return null;
    return {from:{...from},target:{x,y},heading,
      distancePx:Math.hypot(x-from.x,y-from.y)};
  };
  const here=stageAt(from.x,from.y);
  if(here)return here;
  const tx=Math.floor(from.x/16),ty=Math.floor(from.y/16);
  for(let ring=0;ring<=Math.ceil(maxDistancePx/16);ring++){
    const candidates=[];
    for(let dy=-ring;dy<=ring;dy++)for(let dx=-ring;dx<=ring;dx++){
      if(Math.max(Math.abs(dx),Math.abs(dy))!==ring)continue;
      const x=(tx+dx+0.5)*16,y=(ty+dy+0.5)*16;
      const distance=Math.hypot(x-from.x,y-from.y);
      if(distance<=maxDistancePx)candidates.push({x,y,distance});
    }
    candidates.sort((a,b)=>a.distance-b.distance);
    for(const {x,y} of candidates){const stage=stageAt(x,y);if(stage)return stage;}
  }
  return null; // The caller must fail, never skip a motion/safety assertion.
}
