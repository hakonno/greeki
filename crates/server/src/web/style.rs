//! The dashboard stylesheet, inlined into every page render.

pub(super) const CSS: &str = r#"
:root{--bg:#0f1115;--card:#171a21;--line:#262b36;--fg:#e6e9ef;--muted:#8b93a3;
--accent:#f4b740;--good:#46c98b;--bad:#ef6f6f;--blue:#5b8def}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);
font:15px/1.5 ui-sans-serif,system-ui,-apple-system,Segoe UI,Roboto,sans-serif}
main{max-width:920px;margin:0 auto;padding:24px 16px 60px}
header h1{margin:0;font-size:26px;letter-spacing:.5px}
.tag{color:var(--muted);margin:.2em 0 1.4em}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;
padding:18px 20px;margin-bottom:20px}
.card h2{margin:0 0 14px;font-size:15px;text-transform:uppercase;letter-spacing:.08em;color:var(--muted)}
.summary{display:flex;gap:22px;flex-wrap:wrap;margin-bottom:14px}
.stat{display:flex;flex-direction:column}
.stat .label{font-size:11px;color:var(--muted);text-transform:uppercase}
.stat .value{font-size:20px;font-weight:600}
.stat.good .value{color:var(--good)} .stat.bad .value{color:var(--bad)}
.chart{display:flex;align-items:flex-end;gap:3px;height:140px;margin-top:8px}
.bar-wrap{flex:1;display:flex;flex-direction:column;justify-content:flex-end;align-items:center;height:100%}
.bar{width:100%;background:var(--blue);border-radius:3px 3px 0 0;min-height:3px;transition:height .3s}
.bar.now{background:var(--accent)} .bar.cheap{background:var(--good)}
.hour{font-size:9px;color:var(--muted);margin-top:3px}
.muted{color:var(--muted);font-size:13px}
.muted a{color:var(--muted)}
.windows{display:flex;gap:18px;flex-wrap:wrap;font-size:13px;color:var(--muted);margin-top:10px}
.windows .good{color:var(--good)}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px;margin-bottom:14px}
label{display:flex;flex-direction:column;font-size:12px;color:var(--muted);gap:4px}
input,select,textarea{background:var(--bg);border:1px solid var(--line);color:var(--fg);
border-radius:8px;padding:8px 10px;font-size:14px}
textarea{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;resize:vertical;min-height:42px;width:100%}
.wide{display:flex;flex-direction:column;gap:4px;margin-bottom:12px}
.hint{display:block;font-size:11px;color:var(--muted);margin-top:3px;font-weight:400}
.learned{color:var(--good)}
.recognized{color:var(--good)}
.warnhint{display:block;font-size:11px;color:var(--accent);margin-top:3px}
.plan .reason.queued{color:var(--accent)}
button{background:var(--blue);color:#fff;border:0;border-radius:8px;padding:8px 14px;
font-size:13px;cursor:pointer}
button:hover{filter:brightness(1.1)}
button.danger{background:transparent;border:1px solid var(--bad);color:var(--bad)}
.jobs{display:flex;flex-direction:column;gap:12px}
.job{border:1px solid var(--line);border-radius:10px;padding:14px 16px;background:#12151c}
.job-head{display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.job-head .name{font-weight:600;font-size:16px}
.badge{font-size:11px;padding:2px 8px;border-radius:20px;text-transform:uppercase;letter-spacing:.05em}
.badge.pending{background:#2a2f3a;color:var(--muted)}
.badge.running{background:#36506f;color:#cfe2ff}
.badge.completed{background:#1f4636;color:#9fe6c2}
.badge.failed{background:#4a2330;color:#ffb3b3}
.badge.cancelled{background:#33363f;color:#9aa2b1}
.policy,.prio{font-size:12px;color:var(--muted)}
.prio{color:var(--accent)}
.cmd{margin:8px 0}
.cmd code{background:#0b0d12;border:1px solid var(--line);border-radius:6px;
padding:3px 8px;font-size:13px;display:inline-block;color:#cdd6e3}
.meta{display:flex;gap:16px;flex-wrap:wrap;color:var(--muted);font-size:13px;margin-bottom:6px}
.plan{display:flex;gap:12px;align-items:center;flex-wrap:wrap;font-size:13px;margin-top:4px}
.plan .reason{color:#cdd6e3}
.plan .when{color:var(--blue)}
.plan .warn{color:var(--accent);font-weight:600}
.savings{display:flex;gap:14px;font-size:13px;margin-top:4px}
.savings .good,.result .good{color:var(--good)}
.rollup{font-size:13px;color:var(--good);margin:0 0 10px}
.errors{color:var(--bad);border:1px solid var(--bad);border-radius:8px;
padding:8px 12px;margin:0 0 12px;font-size:13px}
.result{display:flex;gap:16px;color:var(--muted);font-size:13px;margin-top:6px}
details{margin-top:8px} summary{cursor:pointer;color:var(--muted);font-size:13px}
pre{background:#0b0d12;border:1px solid var(--line);border-radius:6px;
padding:10px;overflow:auto;font-size:12px;max-height:240px}
.actions{display:flex;gap:8px;margin-top:12px}
.actions button{font-size:12px;padding:6px 12px}
footer{color:var(--muted);font-size:12px;margin-top:10px;text-align:center}
"#;
