//! The dashboard stylesheet, inlined into every page render.
//!
//! Design rules, so the page stays an instrument and not a dashboard theme:
//! - Saturated color is reserved for meaning: the green↔red scale is price /
//!   money (cheap, savings, expensive, failure), amber is "now / attention".
//!   Chrome — buttons, borders, labels — stays neutral graphite.
//! - Every number, time, and unit is set in the mono stack with tabular
//!   figures; prose is the sans stack.

pub(super) const CSS: &str = r#"
:root{
  --bg:#0c0d0f;--panel:#14161a;--inset:#101216;--line:#23262c;--line2:#31353d;
  --fg:#e8e6e1;--muted:#8f8d86;--amber:#f2b13d;--good:#4dbd7f;--bad:#e2634e;
  --mono:ui-monospace,"SF Mono",SFMono-Regular,Menlo,Consolas,monospace;
  --sans:ui-sans-serif,system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
  color-scheme:dark;
}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);font:14.5px/1.55 var(--sans)}
main{max-width:940px;margin:0 auto;padding:28px 18px 64px}
:focus-visible{outline:2px solid var(--amber);outline-offset:2px}

header{display:flex;align-items:baseline;gap:14px;flex-wrap:wrap;margin-bottom:20px}
header h1{margin:0;font:700 21px var(--mono);letter-spacing:-.02em}
header h1 .w{color:var(--amber)}
.tag{margin:0;font:12px var(--mono);color:var(--muted)}

.card{background:var(--panel);border:1px solid var(--line);border-radius:10px;
padding:18px 20px;margin-bottom:16px}
.card h2{margin:0 0 14px;font:600 11px var(--mono);text-transform:uppercase;
letter-spacing:.14em;color:var(--muted)}

/* --- price readout ------------------------------------------------------ */
.readout{display:flex;align-items:flex-end;gap:28px;flex-wrap:wrap}
.now-price{display:flex;flex-direction:column;gap:2px}
.now-price .label{font:600 10px var(--mono);text-transform:uppercase;
letter-spacing:.12em;color:var(--amber)}
.now-price .value{font:600 34px/1.1 var(--mono);font-variant-numeric:tabular-nums;letter-spacing:-.01em}
.now-price .value small{font:400 12px var(--mono);color:var(--muted);letter-spacing:0}
.summary{display:flex;gap:20px;flex-wrap:wrap;padding-bottom:3px}
.stat{display:flex;flex-direction:column;gap:1px}
.stat .label{font:600 10px var(--mono);text-transform:uppercase;letter-spacing:.1em;color:var(--muted)}
.stat .value{font:500 15px var(--mono);font-variant-numeric:tabular-nums}
.stat .value small{font:400 11px var(--mono);color:var(--muted)}
.stat.good .value{color:var(--good)} .stat.bad .value{color:var(--bad)}

/* --- the horizon: bars, day rules, avg line, now cursor ------------------ */
.chart{margin-top:18px}
.plot{padding-top:22px}
.bars{position:relative;display:flex;align-items:flex-end;gap:2px;height:120px}
.bar{flex:1 1 0;position:relative;border-radius:2px 2px 0 0;min-height:2px;transition:height .3s}
.bar.now{box-shadow:inset 0 2px 0 0 var(--amber)}
.bar.now::after{content:"\25BE";position:absolute;bottom:100%;left:50%;
transform:translateX(-50%);padding-bottom:1px;color:var(--amber);font-size:13px;line-height:1}
.sep{flex:0 0 9px;position:relative;height:100%}
.sep::before{content:"";position:absolute;left:4px;top:0;bottom:0;width:1px;background:var(--line2)}
.sep b{position:absolute;top:-20px;left:3px;font:600 9px var(--mono);
text-transform:uppercase;letter-spacing:.08em;color:var(--muted)}
.avg-line{position:absolute;left:0;right:0;height:0;border-top:1px dashed var(--line2)}
.avg-line::after{content:"avg";position:absolute;right:0;top:-13px;font:9px var(--mono);color:var(--muted)}
.axis{display:flex;gap:2px;margin-top:5px}
.axis .h{flex:1 1 0;text-align:center;font:9.5px var(--mono);color:var(--muted);
font-variant-numeric:tabular-nums;white-space:nowrap;min-width:0}
.axis .h.now{color:var(--amber);font-weight:700}
.axis .sp{flex:0 0 9px}
.legend{display:flex;align-items:center;gap:14px;flex-wrap:wrap;margin-top:12px;
font:11px var(--mono);color:var(--muted)}
.legend .scale{display:inline-block;width:56px;height:7px;border-radius:4px;vertical-align:-1px;
margin-right:6px;background:linear-gradient(90deg,hsl(145 42% 46%),hsl(76 50% 48%),hsl(8 58% 54%))}
.legend .now-key{color:var(--amber)}
.windows{display:flex;gap:10px;flex-wrap:wrap;margin-top:12px}
.win{font:11.5px var(--mono);color:var(--muted);border:1px solid var(--line);
border-radius:6px;padding:3px 9px}
.win b{color:var(--good);font-weight:600}
.footnote{margin:12px 0 0;font-size:12px;line-height:1.5;color:var(--muted)}
.muted{color:var(--muted);font-size:12.5px}
.muted a,.footnote a{color:var(--muted)}

/* --- add-job form --------------------------------------------------------- */
.adder>summary{cursor:pointer;list-style:none;display:flex;align-items:center;
justify-content:space-between;margin:-4px 0}
.adder>summary::-webkit-details-marker{display:none}
.card .adder summary h2{margin:0}
.adder>summary::after{content:"+";font:300 18px/1 var(--mono);color:var(--muted);transition:transform .15s}
.adder[open]>summary::after{transform:rotate(45deg)}
.adder[open]>summary{margin-bottom:16px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(195px,1fr));gap:14px 12px;margin-bottom:16px}
label{display:flex;flex-direction:column;gap:5px;font:600 10.5px var(--mono);
text-transform:uppercase;letter-spacing:.09em;color:var(--muted)}
input,select,textarea{background:var(--bg);border:1px solid var(--line);color:var(--fg);
border-radius:7px;padding:8px 10px;font:13.5px var(--sans)}
input[type=number]{font-family:var(--mono);font-variant-numeric:tabular-nums}
input[type=datetime-local]{font-family:var(--mono)}
textarea{font:13px/1.5 var(--mono);resize:vertical;min-height:44px;width:100%}
input:hover,select:hover,textarea:hover{border-color:var(--line2)}
input:focus,select:focus,textarea:focus{outline:none;border-color:var(--muted)}
label .hint,.wide .hint{font:400 11px/1.45 var(--sans);color:var(--muted);
text-transform:none;letter-spacing:0;margin-top:1px}
.warnhint{display:block;font:400 11px/1.45 var(--sans);color:var(--amber);
text-transform:none;letter-spacing:0;margin-top:3px}
.learned,.recognized{color:var(--good);text-transform:none;letter-spacing:0}
.wide{display:flex;flex-direction:column;gap:5px;margin-bottom:14px}
form:has(select[name=policy] option[value=cheapest]:checked) .threshold-field,
form:has(select[name=policy] option[value=immediate]:checked) .threshold-field{display:none}

button{font:600 12px var(--mono);letter-spacing:.02em;background:var(--fg);color:#101216;
border:1px solid var(--fg);border-radius:7px;padding:8px 16px;cursor:pointer}
button:hover{background:#fff;border-color:#fff}
button.htmx-request,form.htmx-request button{opacity:.5;pointer-events:none}

/* --- jobs ----------------------------------------------------------------- */
.rollup{margin:0 0 12px;font:12px var(--mono);color:var(--muted)}
.rollup b{color:var(--good);font-weight:600}
.errors{color:var(--bad);border:1px solid var(--bad);border-radius:8px;
background:color-mix(in srgb,var(--bad) 8%,transparent);
padding:9px 12px;margin:0 0 12px;font-size:13px}
.empty{color:var(--muted);font-size:13px;margin:2px 0}
.jobs{display:flex;flex-direction:column;gap:10px}
.job{background:var(--inset);border:1px solid var(--line);border-radius:8px;padding:13px 16px}
.job-head{display:flex;align-items:baseline;gap:12px;flex-wrap:wrap}
.job-head .name{font-weight:600;font-size:15px}
.status{display:inline-flex;align-items:center;gap:6px;font:600 10.5px var(--mono);
text-transform:uppercase;letter-spacing:.08em}
.status::before{content:"";width:7px;height:7px;border-radius:50%;background:currentColor;flex:none}
.status.pending{color:var(--muted)}
.status.running{color:var(--amber)}
.status.running::before{animation:pulse 1.8s ease-in-out infinite}
.status.completed{color:var(--good)}
.status.failed{color:var(--bad)}
.status.cancelled{color:var(--muted)}
.status.cancelled::before{background:transparent;border:1px solid currentColor}
@keyframes pulse{50%{opacity:.3}}
.policy,.repeat{font:11px var(--mono);color:var(--muted)}
.prio{font:600 10.5px var(--mono);text-transform:uppercase;letter-spacing:.06em}
.prio.low{color:var(--muted)} .prio.high{color:var(--amber)} .prio.critical{color:var(--bad)}
.cmd{margin:9px 0 7px}
.cmd code{display:block;background:var(--bg);border:1px solid var(--line);border-radius:6px;
padding:6px 10px;font:12.5px/1.5 var(--mono);color:#d3d6cf;
white-space:pre-wrap;word-break:break-word;max-height:96px;overflow:auto}
.meta{display:flex;gap:14px;row-gap:4px;flex-wrap:wrap;margin:2px 0}
.kv{font:12px var(--mono);font-variant-numeric:tabular-nums;color:var(--fg)}
.kv .k{color:var(--muted);margin-right:4px}
.kv.good{color:var(--good)} .kv.bad{color:var(--bad)}
.kv .learned{font-size:11.5px}
.plan{display:flex;gap:12px;align-items:baseline;flex-wrap:wrap;font-size:12.5px;margin-top:6px}
.plan .reason{color:var(--fg)}
.plan .reason.queued{color:var(--amber)}
.plan .when{font:600 12px var(--mono);color:var(--good)}
.plan .warn{color:var(--amber);font-weight:600}
.savings{display:flex;gap:12px;flex-wrap:wrap;font:12px var(--mono);margin-top:4px}
.savings .good{color:var(--good)}
.result{display:flex;gap:14px;flex-wrap:wrap;font:12px var(--mono);color:var(--muted);
font-variant-numeric:tabular-nums;margin-top:6px}
.result .kv{color:var(--muted)}
.result .kv.good{color:var(--good)} .result .kv.bad{color:var(--bad)}
.out{margin-top:8px}
.out summary{cursor:pointer;color:var(--muted);font:12px var(--mono)}
.out summary:hover{color:var(--fg)}
pre{background:var(--bg);border:1px solid var(--line);border-radius:6px;
padding:10px;overflow:auto;font:11.5px/1.5 var(--mono);max-height:240px;margin:8px 0 0}
.actions{display:flex;gap:8px;margin-top:12px}
.actions button{background:transparent;border-color:var(--line2);color:var(--fg);
font-size:11.5px;padding:5px 12px}
.actions button:hover{border-color:var(--muted);background:var(--panel)}
.actions button.now{color:var(--amber)}
.actions button.danger{color:var(--bad);border-color:transparent}
.actions button.danger:hover{border-color:var(--bad);background:transparent}

footer{color:var(--muted);font:11px var(--mono);margin-top:14px;text-align:center}

@media (prefers-reduced-motion:reduce){
  *,*::before,*::after{animation:none!important;transition:none!important}
}
@media (max-width:560px){
  .now-price .value{font-size:27px}
  .readout{gap:18px} .summary{gap:14px}
  .bars,.axis{gap:1px}
  .card{padding:14px}
}
"#;
