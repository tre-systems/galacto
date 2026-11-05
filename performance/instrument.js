import init,* as mod from '../pkg/galacto.js';
import {controllerBindings,GALACTO_XL3_LAYOUT} from '../static/controller-bindings.js';
import {GALACTO_DEFAULTS,bodyCount,parameterValue,parameterText} from './galacto-parameters.js';

const setters={
  'temp-slider':'set_disk_temperature','gas-slider':'stage_gas_fraction','bulge-slider':'stage_bulge_fraction',
  'count-slider':'stage_particle_count','speed-slider':'set_speed','gravity-slider':'set_gravity',
  'halo-slider':'set_halo','halo-size-slider':'set_halo_concentration','size-slider':'set_particle_size',
  'glow-slider':'set_glow','autopilot-speed':'set_autopilot_speed','volume-slider':'set_volume',
};
export class Galacto {
  constructor(canvas,curve,changed,failed){
    Object.assign(this,{canvas,curve,changed,failed,active:false,ready:false,frames:0,scenario:6,
      values:{...GALACTO_DEFAULTS},staged:new Map(),halo:false,muted:false,curveVisible:false});
    this.layout=structuredClone(GALACTO_XL3_LAYOUT);this.layout.buttons=this.layout.buttons.filter(b=>b[1]!==52);
    this.bindings=controllerBindings({available:()=>this.ready?false:'Connect or Play to prepare Galacto',
      camera:()=>this.ready?mod.controller_snapshot():[],setCamera:(axis,value)=>{mod.controller_camera(axis,value);this.changed();},
      parameter:id=>{const p=this.staged.get(id)??this.values[id];return {position:p,text:parameterText(id,p)};},
      setParameter:(id,p)=>{
        if(['temp-slider','gas-slider','bulge-slider','count-slider'].includes(id))this.staged.set(id,p);
        else{this.values[id]=p;this.apply(id);}this.changed();
      }});
  }
  async prepare(context,bus){
    if(this.ready)return;
    if(!navigator.gpu)throw Error('Galacto needs WebGPU. Open this local review in a WebGPU-capable browser.');
    await init();this.resize();await mod.host_prepare(this.canvas,context,bus);this.ready=true;
    // Match the standalone page's initialization handlers, including its level taper.
    Object.keys(this.values).forEach(id=>this.apply(id));
  }
  apply(id){
    if(!this.ready)return;
    let value=parameterValue(id,this.values[id]);
    if(id==='speed-slider')value=Math.min(value,mod.max_speed_for_particle_count(bodyCount(this.values['count-slider'])));
    mod[setters[id]](value);
  }
  resize(){
    const w=Math.max(1,Math.round(innerWidth*devicePixelRatio)),h=Math.max(1,Math.round(innerHeight*devicePixelRatio));
    if(this.canvas.width===w&&this.canvas.height===h)return;
    this.canvas.width=w;this.canvas.height=h;if(this.ready)mod.host_resize(w,h);
  }
  async start(){if(this.active)return;mod.host_sound(true);this.active=true;this.changed();}
  tick(ms){
    if(!this.active)return;
    try{this.resize();mod.host_tick(ms);this.frames++;if(this.curveVisible&&ms-(this.curveAt||0)>200){this.curveAt=ms;this.drawCurve();}}
    catch(error){this.failed(error);}
  }
  drawCurve(){
    const ctx=this.curve.getContext('2d'),data=mod.rotation_curve(64),w=this.curve.width,h=this.curve.height;
    ctx.clearRect(0,0,w,h);ctx.fillStyle='#071110d9';ctx.fillRect(0,0,w,h);
    // Upstream rows are radius, bulge, disk, halo and total circular velocity.
    const rows=Array.from({length:data.length/5},(_,i)=>Array.from(data.slice(i*5,i*5+5)));
    const max=Math.max(1,...rows.map(row=>row[4]));
    ['#e4d7ad','#77b4c0','#ac8aaf','#667da2'].forEach((colour,j)=>{
      ctx.strokeStyle=colour;ctx.lineWidth=1.5;ctx.beginPath();
      rows.forEach((row,i)=>{const x=12+i*(w-24)/(rows.length-1),y=h-12-row[j+1]/max*(h-36);i?ctx.lineTo(x,y):ctx.moveTo(x,y);});ctx.stroke();
    });
    ctx.fillStyle='#dae0de';ctx.font='12px system-ui';ctx.fillText('Rotation curve',12,18);
  }
  action(cc,shift=false,down=true){
    if(!down||!this.ready)return;
    const s=mod.controller_snapshot();
    if(cc===37)mod.set_autopilot(!s[4]);
    else if(cc===38){this.halo=!this.halo;mod.set_halo_visible(this.halo);}
    else if(cc===39){this.curveVisible=!this.curveVisible;this.curve.hidden=!this.curveVisible;this.drawCurve();}
    else if(cc===40){this.muted=!this.muted;mod.set_muted(this.muted);}
    else if(cc===41)mod.controller_pause(!s[3]);
    else if(cc===42)mod.controller_home();
    else if(cc===43&&this.staged.size){
      for(const [id,p] of this.staged){this.values[id]=p;this.apply(id);}
      mod.set_particle_count(bodyCount(this.values['count-slider']));this.apply('speed-slider');this.staged.clear();
    }else if(cc===44&&shift)mod.restart();
    else if(cc>=45&&cc<=51){this.staged.clear();this.scenario=cc-45;mod.set_scenario(this.scenario);}
    this.changed();
  }
  keys(){
    const s=this.ready?mod.controller_snapshot():[];
    return new Map([[37,{pressed:!!s[4]}],[38,{pressed:this.halo}],[39,{pressed:this.curveVisible}],
      [40,{pressed:this.muted}],[41,{pressed:!!s[3]}],[43,{pressed:!!this.staged.size,disabled:!this.staged.size}],
      ...Array.from({length:7},(_,i)=>[45+i,{pressed:this.scenario===i}])]);
  }
  diagnostics(){return {ready:this.ready,frames:this.frames,scenario:this.scenario,values:{...this.values},staged:Object.fromEntries(this.staged),native:this.ready?Array.from(mod.host_values()):[],camera:this.ready?Array.from(mod.controller_snapshot()):[]};}
  release(){}
  park(){if(this.ready)mod.host_sound(false);this.active=false;this.changed();}
}
