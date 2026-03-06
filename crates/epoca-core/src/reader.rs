/// Reader Mode — distill web pages into clean, styled article view.
///
/// Uses Mozilla's Readability.js to extract article content, then renders
/// it in a beautiful typographic template inside the same WKWebView.
/// Toggle off reloads the original page.

use std::sync::OnceLock;

/// Readability.js (Apache 2.0, Mozilla)
const READABILITY_JS: &str = include_str!("readability.js");

/// isProbablyReaderable.js (Apache 2.0, Mozilla) — lightweight check
const READERABLE_JS: &str = include_str!("readerable.js");

/// Init script that checks if the page is reader-capable on load.
/// Posts `{type:'readerable', value: true/false}` to epocaMeta.
pub fn readerable_check_script() -> &'static str {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| {
        format!(
            r#"(function(){{
if(window.__epocaReaderableChecked)return;
window.__epocaReaderableChecked=true;
{readerable}
function _rc(){{
  try{{
    var r=isProbablyReaderable(document);
    if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaMeta){{
      window.webkit.messageHandlers.epocaMeta.postMessage({{type:'readerable',value:r}});
    }}
  }}catch(e){{}}
}}
if(document.readyState==='loading'){{
  document.addEventListener('DOMContentLoaded',_rc);
}}else{{
  _rc();
}}
}})();"#,
            readerable = READERABLE_JS,
        )
    })
}

/// Returns the JS payload that activates reader mode.
/// Cached after first call.
pub fn reader_mode_js() -> &'static str {
    static JS: OnceLock<String> = OnceLock::new();
    JS.get_or_init(|| {
        format!(
            r#"(function(){{
if(window.__epocaReaderActive){{return;}}
{readability}
;var dc=document.cloneNode(true);
var article=new Readability(dc).parse();
if(!article||!article.content){{return;}}
var t=article.title||'';
var b=article.byline||'';
var s=article.siteName||'';
var p=article.publishedTime||'';
var e=article.excerpt||'';
document.open();
document.write({template});
document.close();
window.__epocaReaderActive=true;
}})();"#,
            readability = READABILITY_JS,
            template = READER_TEMPLATE,
        )
    })
}

/// HTML template for the reader view as a JS expression that evaluates to a string.
/// Uses JS variables t, b, s, p, e, article set by the caller.
const READER_TEMPLATE: &str = r##"'<!DOCTYPE html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>'
+'*{margin:0;padding:0;box-sizing:border-box}'
+'body{background:#1a1a1e;color:#d4d4d8;font-family:Georgia,"Times New Roman",serif;line-height:1.8;padding:60px 24px 120px;-webkit-font-smoothing:antialiased}'
+'.reader-container{max-width:680px;margin:0 auto}'
+'.reader-meta{margin-bottom:48px;border-bottom:1px solid #333;padding-bottom:32px}'
+'.reader-meta h1{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;font-size:36px;font-weight:700;line-height:1.2;color:#fafafa;margin-bottom:16px;letter-spacing:-0.02em}'
+'.reader-meta .byline{font-size:15px;color:#a1a1aa;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif}'
+'.reader-meta .byline .site{color:#8b5cf6}'
+'.reader-meta .excerpt{font-size:18px;color:#a1a1aa;margin-top:16px;line-height:1.6;font-style:italic}'
+'.reader-content{font-size:19px;color:#d4d4d8}'
+'.reader-content p{margin-bottom:1.5em}'
+'.reader-content h1,.reader-content h2,.reader-content h3,.reader-content h4{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;color:#fafafa;margin-top:2em;margin-bottom:0.8em;line-height:1.3;letter-spacing:-0.01em}'
+'.reader-content h2{font-size:28px;font-weight:700}'
+'.reader-content h3{font-size:22px;font-weight:600}'
+'.reader-content a{color:#8b5cf6;text-decoration:none;border-bottom:1px solid #8b5cf644}'
+'.reader-content a:hover{border-bottom-color:#8b5cf6}'
+'.reader-content img{max-width:100%;height:auto;border-radius:8px;margin:1.5em 0}'
+'.reader-content figure{margin:2em 0}'
+'.reader-content figcaption{font-size:14px;color:#71717a;text-align:center;margin-top:8px;font-style:italic}'
+'.reader-content blockquote{border-left:3px solid #8b5cf6;padding-left:20px;margin:1.5em 0;color:#a1a1aa;font-style:italic}'
+'.reader-content pre{background:#27272a;border-radius:8px;padding:16px;overflow-x:auto;font-family:"SF Mono",Monaco,Consolas,monospace;font-size:14px;line-height:1.6;margin:1.5em 0;color:#e4e4e7}'
+'.reader-content code{font-family:"SF Mono",Monaco,Consolas,monospace;font-size:0.9em;background:#27272a;padding:2px 6px;border-radius:4px}'
+'.reader-content pre code{background:none;padding:0}'
+'.reader-content ul,.reader-content ol{margin:1em 0 1.5em 1.5em}'
+'.reader-content li{margin-bottom:0.5em}'
+'.reader-content table{width:100%;border-collapse:collapse;margin:1.5em 0}'
+'.reader-content th,.reader-content td{padding:10px 14px;border:1px solid #333;text-align:left}'
+'.reader-content th{background:#27272a;font-weight:600;color:#fafafa}'
+'.reader-content hr{border:none;border-top:1px solid #333;margin:2em 0}'
+'::selection{background:#8b5cf644}'
+'</style></head><body><div class="reader-container"><div class="reader-meta"><h1>'+t.replace(/</g,'&lt;')+'</h1>'
+(b||s?'<div class="byline">'+(b?b.replace(/</g,'&lt;'):'')+( s?' &middot; <span class="site">'+s.replace(/</g,'&lt;')+'</span>':'')+(p?' &middot; '+p:'')+'</div>':'')
+(e?'<div class="excerpt">'+e.replace(/</g,'&lt;')+'</div>':'')
+'</div><div class="reader-content">'+article.content+'</div></div></body></html>'"##;
