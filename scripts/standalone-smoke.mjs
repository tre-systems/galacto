export function standaloneProbe(){
  return `(()=>{localStorage.setItem('galacto-control-layout','xl3');window.midiRequests=0;Object.defineProperty(navigator,'requestMIDIAccess',{value:async()=>{window.midiRequests++;throw Error('Unexpected MIDI request');}});})();`;
}
export async function exerciseStandalone(page){
  const result=await page.send('Runtime.evaluate',{expression:`(()=>{
    const assert=(ok,reason)=>{if(!ok)throw Error(reason);};
    assert(!document.querySelector('#xl3-panel,[data-interface]'),'No standalone XL3 panel');
    const values=()=>JSON.stringify(Array.from(document.querySelectorAll('#controls input,#controls select'),n=>[n.id,n.value,n.checked]));
    const before=values(),toggle=document.querySelector('#controls-toggle');toggle.click();
    assert(toggle.getAttribute('aria-expanded')==='true'&&!document.querySelector('#controls').classList.contains('collapsed'),'Obsolete layout preference cannot hide controls');
    assert(values()===before,'Opening preserves settings');toggle.click();
    assert(toggle.getAttribute('aria-expanded')==='false','Close controls');
    assert(window.midiRequests===0,'No automatic MIDI request');return 'pass';
  })()`,returnByValue:true,userGesture:true});
  if(result.exceptionDetails||result.result?.value!=='pass')throw Error('Standalone controls: '+JSON.stringify(result));
}
