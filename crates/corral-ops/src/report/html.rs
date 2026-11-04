//! The report page: inline CSS, inline SVG, a small inline script over an embedded
//! `application/json` model. No external resources; output is deterministic for one model.
use std::collections::BTreeSet;

use anyhow::{Context, Result};

use super::model::{ReportModel, Spend};
use super::svg;

pub(crate) fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Est. API-equivalent dollars; unpriced stays unpriced, never $0.
fn usd(value: Option<f64>) -> String {
    match value {
        Some(v) if v != 0.0 && v.abs() < 0.01 => format!("${v:.4}"),
        Some(v) => format!("${v:.2}"),
        None => "unpriced".into(),
    }
}

fn pct(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |v| format!("{:.1}%", v * 100.0))
}

fn secs(value: Option<f64>) -> String {
    match value {
        None => "-".into(),
        Some(v) if v >= 3600.0 => format!("{:.1} h", v / 3600.0),
        Some(v) if v >= 60.0 => format!("{:.1} min", v / 60.0),
        Some(v) => format!("{v:.0} s"),
    }
}

fn sort_key(value: Option<f64>) -> String {
    value.map_or_else(|| "-1".into(), |v| format!("{v:.6}"))
}

fn td(text: &str) -> String {
    format!("<td>{}</td>", escape(text))
}

fn num(n: impl std::fmt::Display) -> String {
    format!("<td data-v=\"{n}\">{n}</td>")
}

fn money(value: Option<f64>) -> String {
    let class = if value.is_none() {
        " class=\"unpriced\""
    } else {
        ""
    };
    format!(
        "<td data-v=\"{}\"{class}>{}</td>",
        sort_key(value),
        usd(value)
    )
}

fn head(columns: &[&str]) -> String {
    let cells: String = columns
        .iter()
        .map(|c| format!("<th>{}</th>", escape(c)))
        .collect();
    format!("<thead><tr>{cells}</tr></thead>\n<tbody>\n")
}

fn table(columns: &[&str], rows: &[String]) -> String {
    if rows.is_empty() {
        return "<p class=\"empty\">No data in this window.</p>\n".into();
    }
    let mut out = format!(
        "<div class=\"scroll\"><table class=\"sortable\">\n{}",
        head(columns)
    );
    for row in rows {
        out.push_str(&format!("<tr>{row}</tr>\n"));
    }
    out.push_str("</tbody></table></div>\n");
    out
}

fn spend_cells(spend: &Spend) -> String {
    format!(
        "{}{}{}",
        money(spend.estimated_usd),
        num(spend.tokens),
        num(spend.unpriced_tokens)
    )
}

fn card(label: &str, value: &str) -> String {
    format!(
        "<div class=\"card\"><div class=\"k\">{}</div><div class=\"v\">{}</div></div>\n",
        escape(label),
        escape(value)
    )
}

/// The model as an inert JSON data block: `<`, `>` and `&` are escaped so no text in it can
/// close the script element.
pub(crate) fn data_block(model: &ReportModel) -> Result<String> {
    let json = serde_json::to_string(model).context("serializing report model")?;
    Ok(json
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026"))
}

fn summary(model: &ReportModel) -> String {
    let s = &model.summary;
    let mut out = String::from("<section id=\"summary\"><h2>Summary</h2>\n<div class=\"cards\">\n");
    out.push_str(&card(
        "Est. API-equivalent spend",
        &usd(s.spend.estimated_usd),
    ));
    out.push_str(&card(
        "Unpriced tokens",
        &s.spend.unpriced_tokens.to_string(),
    ));
    out.push_str(&card("Tokens", &s.spend.tokens.to_string()));
    out.push_str(&card("Dispatches", &s.dispatches.to_string()));
    out.push_str(&card("Reviewer sessions", &s.reviewer_sessions.to_string()));
    out.push_str(&card(
        "Tasks / landed",
        &format!("{} / {}", s.tasks, s.landed_tasks),
    ));
    out.push_str(&card("Cache hit ratio", &pct(s.cache_hit_ratio)));
    out.push_str(&card("Gate failure rate", &pct(s.gate_failure_rate)));
    out.push_str(&card("Send-backs", &s.send_backs.to_string()));
    out.push_str(&card("Rework share (est.)", &pct(s.rework_share)));
    out.push_str(&card(
        "Sessions without usage",
        &s.sessions_without_usage.to_string(),
    ));
    out.push_str("</div></section>\n");
    out
}

fn spend_bars(rows: &std::collections::BTreeMap<String, Spend>) -> Vec<(String, f64, String)> {
    rows.iter()
        .map(|(k, s)| {
            (
                k.clone(),
                s.estimated_usd.unwrap_or(0.0),
                usd(s.estimated_usd),
            )
        })
        .collect()
}

fn spend(model: &ReportModel) -> String {
    let s = &model.spend;
    let mut out = String::from("<section id=\"spend\"><h2>Spend</h2>\n");
    out.push_str("<h3>By day (est. API-equivalent)</h3>\n");
    out.push_str(&svg::columns("Spend by day", &spend_bars(&s.by_day)));
    out.push_str("<h3>By project</h3>\n");
    out.push_str(&svg::hbars("Spend by project", &spend_bars(&s.by_project)));
    let columns = [
        "Group",
        "Est. USD (API-equivalent)",
        "Tokens",
        "Unpriced tokens",
    ];
    out.push_str(&table(
        &columns,
        &s.by_project
            .iter()
            .map(|(k, v)| td(k) + &spend_cells(v))
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>By role</h3>\n");
    let roles: Vec<(String, f64)> = s
        .by_role
        .iter()
        .map(|(k, v)| (k.clone(), v.estimated_usd.unwrap_or(0.0)))
        .collect();
    out.push_str(&svg::stacked("Spend by role", &roles));
    out.push_str(&table(
        &columns,
        &s.by_role
            .iter()
            .map(|(k, v)| td(k) + &spend_cells(v))
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>By token category</h3>\n");
    let parts: Vec<(String, f64)> = s
        .categories
        .iter()
        .filter_map(|c| c.estimated_usd.map(|v| (c.category.to_owned(), v)))
        .collect();
    out.push_str(&svg::stacked("Est. spend by token category", &parts));
    out.push_str(&table(
        &["Category", "Tokens", "Est. USD (API-equivalent)"],
        &s.categories
            .iter()
            .map(|c| td(c.category) + &num(c.tokens) + &money(c.estimated_usd))
            .collect::<Vec<_>>(),
    ));
    out.push_str(&format!(
        "<p>Cache hit ratio (cache reads over input, cache read and cache write tokens): <b>{}</b></p>\n</section>\n",
        pct(s.cache_hit_ratio)
    ));
    out
}

fn models(model: &ReportModel) -> String {
    let mut out =
        String::from("<section id=\"models\"><h2>Models and modes</h2>\n<h3>Models</h3>\n");
    out.push_str(&table(
        &[
            "Model",
            "Price sources",
            "Responses",
            "Est. USD (API-equivalent)",
            "Tokens",
            "Unpriced tokens",
        ],
        &model
            .models
            .iter()
            .map(|m| {
                td(&m.model)
                    + &td(&m.price_sources.join(", "))
                    + &num(m.spend.entries)
                    + &spend_cells(&m.spend)
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>Modes</h3>\n");
    out.push_str(&table(
        &[
            "Role",
            "Harness",
            "Model",
            "Effort",
            "Mode",
            "Sessions",
            "Est. USD (API-equivalent)",
            "Tokens",
            "Unpriced tokens",
        ],
        &model
            .modes
            .iter()
            .map(|m| {
                td(&m.role)
                    + &td(&m.harness)
                    + &td(&m.model)
                    + &td(&m.effort)
                    + &td(&m.mode)
                    + &num(m.sessions)
                    + &spend_cells(&m.spend)
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str(&crate::throughput::throughput_section_html(
        &model.throughput_by_mode,
    ));
    out.push_str("</section>\n");
    out
}

fn tasks(model: &ReportModel) -> String {
    let mut out = String::from("<section id=\"tasks\"><h2>Tasks</h2>\n<h3>Top spenders</h3>\n");
    let top: Vec<(String, f64, String)> = model
        .tasks
        .iter()
        .filter(|t| t.total.estimated_usd.is_some())
        .take(10)
        .map(|t| {
            (
                format!("{} {}", t.id, t.title),
                t.total.estimated_usd.unwrap_or(0.0),
                usd(t.total.estimated_usd),
            )
        })
        .collect();
    out.push_str(&svg::hbars("Top spending tasks", &top));
    let projects: BTreeSet<&str> = model.tasks.iter().map(|t| t.project.as_str()).collect();
    out.push_str(
        "<div class=\"filters\"><input id=\"task-filter\" type=\"search\" placeholder=\"Filter tasks\" aria-label=\"Filter tasks\">\
<select id=\"task-project\" aria-label=\"Project\"><option value=\"\">All projects</option>",
    );
    for project in projects {
        out.push_str(&format!(
            "<option value=\"{0}\">{0}</option>",
            escape(project)
        ));
    }
    out.push_str(
        "</select></div>\n<p class=\"hint\">Click a column to sort, a row for its runs.</p>\n",
    );
    if model.tasks.is_empty() {
        out.push_str("<p class=\"empty\">No data in this window.</p>\n</section>\n");
        return out;
    }
    out.push_str("<div class=\"scroll\"><table class=\"sortable\" id=\"task-table\">\n");
    out.push_str(&head(&[
        "Task",
        "Title",
        "Project",
        "Status",
        "Runs",
        "Rounds",
        "Landed",
        "Impl est. USD",
        "Review est. USD",
        "Total est. USD",
        "Tokens",
        "Unpriced tokens",
    ]));
    for t in &model.tasks {
        out.push_str(&format!(
            "<tr class=\"task\" data-task=\"{}\" data-project=\"{}\">{}{}{}{}{}{}{}{}{}{}{}{}</tr>\n",
            escape(&t.id),
            escape(&t.project),
            td(&t.id),
            td(&t.title),
            td(&t.project),
            td(&t.status),
            num(t.runs),
            num(t.rounds),
            td(if t.landed { "yes" } else { "no" }),
            money(t.implementation.estimated_usd),
            money(t.review.estimated_usd),
            money(t.total.estimated_usd),
            num(t.total.tokens),
            num(t.total.unpriced_tokens),
        ));
    }
    out.push_str("</tbody></table></div>\n</section>\n");
    out
}

fn quality(model: &ReportModel) -> String {
    let q = &model.quality;
    let mut out = String::from(
        "<section id=\"quality\"><h2>Quality loop</h2>\n<h3>Rounds per dispatch</h3>\n",
    );
    let rounds: Vec<(String, f64, String)> = q
        .rounds_distribution
        .iter()
        .map(|(r, n)| (r.to_string(), f64::from(*n), n.to_string()))
        .collect();
    out.push_str(&svg::columns("Rounds distribution", &rounds));
    out.push_str("<h3>Gate failure by project</h3>\n");
    let gates: Vec<(String, f64, String)> = q
        .gates_by_project
        .iter()
        .map(|(p, g)| {
            (
                p.clone(),
                g.failure_rate.unwrap_or(0.0),
                pct(g.failure_rate),
            )
        })
        .collect();
    out.push_str(&svg::hbars("Gate failure rate by project", &gates));
    out.push_str(&table(
        &["Project", "Pass", "Fail", "Failure rate"],
        &q.gates_by_project
            .iter()
            .map(|(p, g)| {
                td(p)
                    + &num(g.pass)
                    + &num(g.fail)
                    + &format!(
                        "<td data-v=\"{}\">{}</td>",
                        sort_key(g.failure_rate),
                        pct(g.failure_rate)
                    )
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>Send-back causes</h3>\n");
    let causes: Vec<(String, f64, String)> = q
        .send_backs
        .iter()
        .map(|(c, n)| (c.clone(), f64::from(*n), n.to_string()))
        .collect();
    out.push_str(&svg::hbars("Send-back causes", &causes));
    out.push_str(&format!(
        "<p>Rework (est. API-equivalent spend after a dispatch's first send-back): <b>{}</b>, share <b>{}</b>.</p>\n</section>\n",
        usd(q.rework_estimated_usd),
        pct(q.rework_share)
    ));
    out
}

fn time(model: &ReportModel) -> String {
    let t = &model.time;
    let mut out =
        String::from("<section id=\"time\"><h2>Time</h2>\n<h3>Phase medians per project</h3>\n");
    let phases: BTreeSet<_> = t
        .phases_by_project
        .values()
        .flat_map(|p| p.keys().copied())
        .collect();
    let names: Vec<String> = phases.iter().map(super::model::label).collect();
    let mut columns = vec!["Project"];
    columns.extend(names.iter().map(String::as_str));
    let rows: Vec<String> = t
        .phases_by_project
        .iter()
        .map(|(project, totals)| {
            let mut row = td(project);
            for phase in &phases {
                row.push_str(&match totals.get(phase) {
                    Some(p) => format!(
                        "<td data-v=\"{}\" title=\"{} complete, {} incomplete, total {}\">{}</td>",
                        sort_key(p.median_seconds),
                        p.complete,
                        p.incomplete,
                        secs(p.total_seconds),
                        secs(p.median_seconds)
                    ),
                    None => "<td data-v=\"-1\">-</td>".into(),
                });
            }
            row
        })
        .collect();
    out.push_str(&table(&columns, &rows));
    out.push_str("<h3>Gate duration distribution</h3>\n");
    let buckets: Vec<(String, f64, String)> = t
        .gate_durations
        .iter()
        .map(|b| (b.label.to_owned(), f64::from(b.count), b.count.to_string()))
        .collect();
    if t.gate_median_seconds.is_some() {
        out.push_str(&svg::columns("Gate durations", &buckets));
    } else {
        out.push_str("<p class=\"empty\">No data in this window.</p>\n");
    }
    out.push_str(&format!(
        "<p>Median <b>{}</b>, p90 <b>{}</b>.</p>\n<h3>Slowest gate commands</h3>\n",
        secs(t.gate_median_seconds),
        secs(t.gate_p90_seconds)
    ));
    out.push_str(&table(
        &["Command", "Samples", "Median", "Max", "Total"],
        &t.slowest_gate_commands
            .iter()
            .map(|c| {
                format!("<td class=\"cmd\">{}</td>", escape(&c.command))
                    + &num(c.samples)
                    + &format!(
                        "<td data-v=\"{}\">{}</td>",
                        sort_key(c.median_seconds),
                        secs(c.median_seconds)
                    )
                    + &format!(
                        "<td data-v=\"{:.3}\">{}</td>",
                        c.max_seconds,
                        secs(Some(c.max_seconds))
                    )
                    + &format!(
                        "<td data-v=\"{:.3}\">{}</td>",
                        c.total_seconds,
                        secs(Some(c.total_seconds))
                    )
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str("</section>\n");
    out
}

fn data_quality(model: &ReportModel) -> String {
    let d = &model.data_quality;
    let mut out = format!(
        "<section id=\"data-quality\"><h2>Data quality</h2>\n<p>Price catalog date: <b>{}</b>. \
Dollars are est. API-equivalent at configured or catalog rates, not invoices.</p>\n<h3>Price sources</h3>\n",
        escape(d.catalog_date)
    );
    out.push_str(&table(
        &["Source", "Priced responses"],
        &d.price_sources
            .iter()
            .map(|(s, n)| td(s) + &num(n))
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>Unpriced models</h3>\n");
    out.push_str(&table(
        &["Model", "Responses", "Tokens"],
        &d.unpriced_models
            .iter()
            .map(|m| td(&m.model) + &num(m.entries) + &num(m.tokens))
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>Unsupported harnesses</h3>\n");
    out.push_str(&table(
        &["Harness", "Sessions"],
        &d.unsupported_harnesses
            .iter()
            .map(|(h, n)| td(h) + &num(n))
            .collect::<Vec<_>>(),
    ));
    out.push_str("<h3>Sessions without usage</h3>\n");
    out.push_str(&table(
        &["Task", "Run", "Role", "Harness", "Note"],
        &d.runs_without_usage
            .iter()
            .map(|m| {
                td(&m.task)
                    + &td(&m.run)
                    + &td(m.role)
                    + &td(&m.harness)
                    + &td(m.note.as_deref().unwrap_or("-"))
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str("</section>\n");
    out
}

const STYLE: &str = "*{box-sizing:border-box}body{font:14px/1.5 system-ui,sans-serif;margin:0;color:#1b1f24;background:#fff}\
header{padding:1rem 1.5rem;border-bottom:1px solid #d0d7de}header h1{margin:0;font-size:1.4rem}\
header p{margin:.2rem 0 0;color:#57606a}nav{display:flex;flex-wrap:wrap;gap:.3rem 1rem;margin-top:.5rem}\
nav a{color:#0969da;text-decoration:none}main{max-width:72rem;margin:0 auto;padding:0 1.5rem 2rem}\
section{margin-top:1.5rem}h2{border-bottom:1px solid #d0d7de;padding-bottom:.2rem}h3{font-size:1rem;margin:1rem 0 .4rem}\
.cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(11rem,1fr));gap:.6rem}\
.card{border:1px solid #d0d7de;border-radius:6px;padding:.5rem .7rem}.card .k{color:#57606a;font-size:.8rem}\
.card .v{font-size:1.25rem;font-weight:600}.scroll{overflow-x:auto}table{border-collapse:collapse;width:100%}\
th,td{padding:.3rem .6rem;border-bottom:1px solid #d0d7de;text-align:right;white-space:nowrap}\
th:first-child,td:first-child{text-align:left}td.cmd{text-align:left;white-space:normal;font-family:monospace}\
table.sortable th{cursor:pointer;user-select:none}th[data-dir=asc]::after{content:' \\25B2'}th[data-dir=desc]::after{content:' \\25BC'}\
tr.task{cursor:pointer}tr.task:hover{background:#f6f8fa}tr.detail>td{background:#f6f8fa;text-align:left}\
td.unpriced{color:#9a6700;font-style:italic}.empty,.hint{color:#57606a}.filters{display:flex;flex-wrap:wrap;gap:.5rem;margin:.6rem 0}\
.filters input,.filters select{font:inherit;padding:.25rem .4rem}svg.chart{width:100%;height:auto;min-width:30rem;max-width:48rem;display:block}\
svg .bar{fill:#2f81f7}svg .lbl,svg .val{font-size:11px;fill:currentColor}svg .axis{stroke:#8c959f}\
.s0{fill:#2f81f7}.s1{fill:#bf8700}.s2{fill:#1a7f37}.s3{fill:#8250df}.s4{fill:#cf222e}.s5{fill:#57606a}\
footer{color:#57606a;border-top:1px solid #d0d7de;margin-top:2rem;padding-top:.6rem}\
@media(max-width:40rem){main,header{padding-left:.8rem;padding-right:.8rem}.card .v{font-size:1.05rem}}\
@media(prefers-color-scheme:dark){body{color:#e6edf3;background:#0d1117}th,td,h2,header,.card,footer{border-color:#30363d}\
header p,.card .k,.empty,.hint,footer{color:#8b949e}nav a{color:#58a6ff}tr.task:hover,tr.detail>td{background:#161b22}}";

const SCRIPT: &str = r#"(function(){
var data=JSON.parse(document.getElementById('report-data').textContent);
var tasks={};data.tasks.forEach(function(t){tasks[t.id]=t;});
function val(c){return c.hasAttribute('data-v')?c.getAttribute('data-v'):c.textContent;}
function usd(s){var v=s.estimated_usd;if(v===null||v===undefined){return 'unpriced';}return '$'+(v!==0&&Math.abs(v)<0.01?v.toFixed(4):v.toFixed(2));}
function closeDetails(tb){Array.prototype.slice.call(tb.querySelectorAll('tr.detail')).forEach(function(r){r.parentNode.removeChild(r);});}
Array.prototype.forEach.call(document.querySelectorAll('table.sortable'),function(table){
 var ths=table.tHead.rows[0].cells;
 Array.prototype.forEach.call(ths,function(th,i){th.addEventListener('click',function(){
  var tb=table.tBodies[0];closeDetails(tb);
  var asc=th.getAttribute('data-dir')!=='asc';
  Array.prototype.forEach.call(ths,function(h){h.removeAttribute('data-dir');});
  th.setAttribute('data-dir',asc?'asc':'desc');
  var rows=Array.prototype.slice.call(tb.rows);
  rows.sort(function(a,b){var x=val(a.cells[i]),y=val(b.cells[i]);var nx=parseFloat(x),ny=parseFloat(y);
   var c=(!isNaN(nx)&&!isNaN(ny)&&isFinite(x)&&isFinite(y))?nx-ny:String(x).localeCompare(String(y));return asc?c:-c;});
  rows.forEach(function(r){tb.appendChild(r);});
 });});
});
var filter=document.getElementById('task-filter'),project=document.getElementById('task-project');
function apply(){var q=filter.value.toLowerCase(),p=project.value;var tb=document.getElementById('task-table');if(!tb){return;}
 closeDetails(tb.tBodies[0]);
 Array.prototype.forEach.call(tb.tBodies[0].rows,function(r){var ok=(!p||r.getAttribute('data-project')===p)&&r.textContent.toLowerCase().indexOf(q)>=0;r.style.display=ok?'':'none';});}
if(filter){filter.addEventListener('input',apply);project.addEventListener('change',apply);}
function cell(tr,text){var td=document.createElement('td');td.textContent=text;tr.appendChild(td);}
Array.prototype.forEach.call(document.querySelectorAll('tr.task'),function(row){row.addEventListener('click',function(){
 var next=row.nextElementSibling;if(next&&next.className==='detail'){next.parentNode.removeChild(next);return;}
 var t=tasks[row.getAttribute('data-task')];if(!t){return;}
 var tr=document.createElement('tr');tr.className='detail';var td=document.createElement('td');td.colSpan=row.cells.length;
 var table=document.createElement('table');var h=document.createElement('tr');
 ['Run','Started','Outcome','Harness','Model','Mode','Rounds','Gate failures','Reviewers','Usage','Impl est. USD','Review est. USD'].forEach(function(x){var th=document.createElement('th');th.textContent=x;h.appendChild(th);});
 table.appendChild(h);
 t.run_rows.forEach(function(r){var x=document.createElement('tr');
  [r.id,r.started.slice(0,16).replace('T',' '),r.outcome||'open',r.harness||'-',r.model||'-',r.mode,String(r.rounds),String(r.gate_failures),String(r.reviewers),r.usage_measured?'measured':'missing',usd(r.implementation),usd(r.review)].forEach(function(v){cell(x,v);});
  table.appendChild(x);});
 td.appendChild(table);tr.appendChild(td);row.parentNode.insertBefore(tr,row.nextSibling);
});});
})();"#;

/// The full page. The same model always renders the same bytes.
pub fn render(model: &ReportModel) -> Result<String> {
    let scope = model.project.as_deref().map_or_else(
        || "all projects".to_owned(),
        |p| format!("project {}", escape(p)),
    );
    let mut page = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>corral report</title><style>{STYLE}</style></head><body>\n\
<header><h1>corral report</h1><p>Dispatches started in [{}, {}], {scope}. \
All dollar figures are est. API-equivalent; unpriced tokens are shown separately, never as $0.</p>\n\
<nav><a href=\"#summary\">Summary</a><a href=\"#spend\">Spend</a><a href=\"#models\">Models and modes</a>\
<a href=\"#tasks\">Tasks</a><a href=\"#quality\">Quality loop</a><a href=\"#time\">Time</a>\
<a href=\"#data-quality\">Data quality</a></nav></header>\n<main>\n",
        model.since, model.snapshot_at
    );
    for section in [
        summary(model),
        spend(model),
        models(model),
        tasks(model),
        quality(model),
        time(model),
        data_quality(model),
    ] {
        page.push_str(&section);
    }
    page.push_str(&format!(
        "<footer>Schema {} &middot; snapshot {} &middot; generated by corral report.</footer>\n</main>\n\
<script type=\"application/json\" id=\"report-data\">{}</script>\n<script>{SCRIPT}</script>\n</body></html>\n",
        model.schema_version,
        model.snapshot_at,
        data_block(model)?
    ));
    Ok(page)
}
