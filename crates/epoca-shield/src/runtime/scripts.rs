/// Generate the document_end JS: cosmetic removal, overlay sweeper, consent auto-dismiss.
pub fn document_end_script(cosmetic_css: &str) -> String {
    let css_escaped = cosmetic_css.replace('`', "\\`");
    format!(
        r#"(function(){{
'use strict';

// === Cosmetic CSS injection ===
if({has_css}){{
  var s=document.createElement('style');
  s.id='__epoca_cosmetic__';
  s.textContent=`{css}`;
  (document.head||document.documentElement).appendChild(s);
}}

// === Overlay sweeper ===
function _epochSweep(){{
  var els=document.querySelectorAll('*');
  for(var i=0;i<els.length;i++){{
    var el=els[i];try{{
      var st=window.getComputedStyle(el);
      var z=parseInt(st.zIndex,10);
      if(!isNaN(z)&&z>999&&(st.position==='fixed'||st.position==='absolute')
        &&parseFloat(st.opacity)>0.5&&st.display!=='none'
        &&el.offsetWidth>window.innerWidth*0.4&&el.offsetHeight>window.innerHeight*0.4){{
        var cls=(el.className||'').toString();
        var id=(el.id||'').toString();
        if(/ad|popup|overlay|interstitial|banner|promo|modal-ad/i.test(cls+id)){{
          el.remove();
          document.body.style.overflow='';
        }}
      }}
    }}catch(e){{}}
  }}
}}
setInterval(_epochSweep,1200);
var _sweepObs=new MutationObserver(function(m){{
  for(var x of m){{if(x.addedNodes.length>0){{_epochSweep();break;}}}}
}});
if(document.body)_sweepObs.observe(document.body,{{childList:true,subtree:true}});

// === Cookie consent auto-dismiss (reject-only) ===
var _rejectPat=[/reject all/i,/decline all/i,/refuse/i,/necessary only/i,/only essential/i,/continue without/i];
function _epochConsent(){{
  var btns=document.querySelectorAll('button,[role="button"]');
  for(var b of btns){{
    var t=(b.innerText||b.textContent||'').trim();
    if(_rejectPat.some(function(p){{return p.test(t);}})){{b.click();return true;}}
  }}
  return false;
}}
if(!_epochConsent()){{
  var _conObs=new MutationObserver(function(){{if(_epochConsent())_conObs.disconnect();}});
  if(document.body)_conObs.observe(document.body,{{childList:true,subtree:true}});
  setTimeout(function(){{_conObs.disconnect();}},15000);
}}

// === YouTube ad segment skip ===
if(location.hostname==='www.youtube.com'||location.hostname==='youtube.com'){{
  function _epochYtSkip(){{
    var p=document.querySelector('video');
    if(!p)return;
    // Skip overlay ads
    var skip=document.querySelector('.ytp-ad-skip-button,.ytp-ad-skip-button-modern,.ytp-skip-ad-button');
    if(skip){{skip.click();return;}}
    // If an ad is playing and player shows ad UI, skip to end
    var adOverlay=document.querySelector('.ad-showing,.ad-interrupting');
    if(adOverlay&&p.duration&&p.duration<300&&p.currentTime<p.duration){{
      p.currentTime=p.duration;
    }}
  }}
  setInterval(_epochYtSkip,500);
  var _ytObs=new MutationObserver(function(){{_epochYtSkip();}});
  var _ytTarget=document.querySelector('#movie_player')||document.body;
  if(_ytTarget)_ytObs.observe(_ytTarget,{{childList:true,subtree:true,attributes:true}});
}}

// Report cosmetic count to native
if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaShield){{
  window.webkit.messageHandlers.epocaShield.postMessage({{type:'cosmeticReady',count:document.querySelectorAll('#__epoca_cosmetic__').length}});
}}
}})();"#,
        has_css = if cosmetic_css.is_empty() { "false" } else { "true" },
        css = css_escaped,
    )
}
