/// Generate the document_start fingerprint protection JS script.
/// The seed is derived from the session so it is consistent within a page
/// but differs across origins and sessions — preventing cross-site correlation.
pub fn generate_script(session_seed: u64) -> String {
    format!(
        r#"(function(){{
'use strict';
var _s={seed};
function _r(s){{var x=Math.sin(s+_s)*1e4;return x-Math.floor(x);}}

// Canvas noise
var _oTDU=HTMLCanvasElement.prototype.toDataURL;
HTMLCanvasElement.prototype.toDataURL=function(t,q){{
  var ctx=this.getContext('2d');
  if(ctx){{var d=ctx.getImageData(0,0,this.width,this.height);
    for(var i=0;i<d.data.length;i+=4){{var n=(_r(i)-0.5)*2;d.data[i]=Math.max(0,Math.min(255,d.data[i]+n));}}
    ctx.putImageData(d,0,0);}}
  return _oTDU.call(this,t,q);
}};

// WebGL normalization
var _oGP=WebGLRenderingContext.prototype.getParameter;
WebGLRenderingContext.prototype.getParameter=function(p){{
  if(p===37445)return 'Intel Inc.';
  if(p===37446)return 'Intel Iris OpenGL Engine';
  return _oGP.call(this,p);
}};
if(window.WebGL2RenderingContext){{
  var _oGP2=WebGL2RenderingContext.prototype.getParameter;
  WebGL2RenderingContext.prototype.getParameter=function(p){{
    if(p===37445)return 'Intel Inc.';
    if(p===37446)return 'Intel Iris OpenGL Engine';
    return _oGP2.call(this,p);
  }};
}}

// navigator normalization
try{{Object.defineProperty(navigator,'hardwareConcurrency',{{get:function(){{return 4;}}}});}}catch(e){{}}
try{{Object.defineProperty(navigator,'deviceMemory',{{get:function(){{return 8;}}}});}}catch(e){{}}

// Audio oscillator noise: add subtle frequency offset to prevent AudioContext fingerprinting
try{{
  var _oCreateOsc=AudioContext.prototype.createOscillator;
  AudioContext.prototype.createOscillator=function(){{
    var osc=_oCreateOsc.call(this);
    var _oSetFreq=Object.getOwnPropertyDescriptor(OscillatorNode.prototype.__proto__,'frequency');
    // Slightly perturb the frequency value read-back (±0.01%)
    if(osc.frequency&&osc.frequency.value!==undefined){{
      var _origFreqGet=Object.getOwnPropertyDescriptor(AudioParam.prototype,'value').get;
      Object.defineProperty(osc.frequency,'value',{{
        get:function(){{var v=_origFreqGet.call(this);return v*(1+(_r(v*1000)-0.5)*0.0002);}},
        set:function(v){{AudioParam.prototype.value=v;}},
        configurable:true
      }});
    }}
    return osc;
  }};
}}catch(e){{}}

// Screen size rounding to nearest 100px
try{{
  Object.defineProperty(screen,'width',{{get:function(){{return Math.round(window.outerWidth/100)*100;}}}});
  Object.defineProperty(screen,'height',{{get:function(){{return Math.round(window.outerHeight/100)*100;}}}});
  Object.defineProperty(screen,'availWidth',{{get:function(){{return screen.width;}}}});
  Object.defineProperty(screen,'availHeight',{{get:function(){{return screen.height;}}}});
}}catch(e){{}}

// Exit-intent popup suppression: block mouseleave/beforeunload popups
var _oAEL=EventTarget.prototype.addEventListener;
EventTarget.prototype.addEventListener=function(type,fn,opts){{
  if(type==='beforeunload'&&this===window)return;
  if(type==='mouseleave'&&this===document)return;
  if(type==='mouseout'&&this===document)return;
  return _oAEL.call(this,type,fn,opts);
}};
// Neutralize onbeforeunload assignment
try{{Object.defineProperty(window,'onbeforeunload',{{set:function(){{}},get:function(){{return null;}}}});}}catch(e){{}}

// window.open interception
var _oOpen=window.open;
window.open=function(url,target,features){{
  if(window.__epocaAllowNextPopup){{window.__epocaAllowNextPopup=false;return _oOpen.call(this,url,target,features);}}
  if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaShield){{
    window.webkit.messageHandlers.epocaShield.postMessage({{type:'popupBlocked',url:url||''}});
  }}
  return null;
}};
}})();"#,
        seed = session_seed
    )
}
