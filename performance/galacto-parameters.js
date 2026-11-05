// The same normalized positions and tapers as Galacto's pinned controls.
export const GALACTO_DEFAULTS=Object.freeze({
  'temp-slider':.32,'gas-slider':.48,'bulge-slider':.17,'count-slider':.10,
  'speed-slider':.40,'gravity-slider':.50,'halo-slider':.50,'halo-size-slider':.50,
  'size-slider':.50,'glow-slider':.40,'autopilot-speed':.20,'volume-slider':.32,
});
export const bodyCount=p=>Math.max(256,Math.round(p*163840/256)*256);
export function parameterValue(id,p){
  return ({'temp-slider':()=>.5+p*2.5,'gas-slider':()=>p*.5,'bulge-slider':()=>p*.6,
    'count-slider':()=>bodyCount(p),'speed-slider':()=>.25*32**p,'gravity-slider':()=>.25*16**p,
    'halo-slider':()=>p*150,'halo-size-slider':()=>.4*6.25**p,'size-slider':()=>.006+p*.02,
    'glow-slider':()=>p,'autopilot-speed':()=>p*2,'volume-slider':()=>p*p}[id])();
}
export function parameterText(id,p){
  const v=parameterValue(id,p);
  return id==='count-slider'?v.toLocaleString():id==='gas-slider'||id==='bulge-slider'?Math.round(v*100)+'%':
    id==='volume-slider'?Math.round(p*100)+'%':id==='halo-slider'?Math.round(v*220/75)+' km/s':
    id==='halo-size-slider'?(v*15).toFixed(1)+' kpc':id==='size-slider'?(v/.016).toFixed(2)+'×':
    id==='glow-slider'?(1+2*v).toFixed(1)+'×':v.toFixed(2);
}
