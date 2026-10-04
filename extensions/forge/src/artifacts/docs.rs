use crate::model::{ForgeError, ForgeResult, Operation, ResolvedOpenApi};
use serde::Serialize;

pub(super) fn render_index(contract: &ResolvedOpenApi, digest: &str) -> ForgeResult<String> {
    let search_json = String::from_utf8(render_search(contract, digest)?)
        .map_err(|err| ForgeError(format!("encode docs search: {err}")))?;
    let escaped_search = search_json.replace('<', "\\u003c").replace('>', "\\u003e");
    let mut html = String::new();
    html.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>");
    html.push_str(&escape_html(&contract.title));
    html.push_str(" SDK</title>\n<style>body{font-family:system-ui,sans-serif;max-width:72rem;margin:2rem auto;padding:0 1rem;line-height:1.45}code{background:#f5f5f5;padding:.1rem .25rem;border-radius:.25rem}.digest{overflow-wrap:anywhere;word-break:break-word}section{border-top:1px solid #ddd;padding:1rem 0}details{margin-top:.75rem}pre{overflow:auto;max-width:100%;background:#f7f7f7;padding:.75rem;border-radius:.375rem}pre code{background:transparent;padding:0;white-space:pre-wrap;overflow-wrap:anywhere}label{display:block;font-weight:600}input{width:100%;max-width:32rem;padding:.5rem;margin:.25rem 0 1rem}.muted{color:#666}</style>\n</head>\n<body>\n");
    html.push_str("<h1>");
    html.push_str(&escape_html(&contract.title));
    html.push_str("</h1>\n<p>Namespace <code>");
    html.push_str(&escape_html(&contract.namespace));
    html.push_str("</code>. Contract digest <code class=\"digest\">");
    html.push_str(&escape_html(digest));
    html.push_str("</code>.</p>\n<label for=\"search\">Search operations</label>\n<input id=\"search\" type=\"search\" aria-controls=\"operations\" autocomplete=\"off\">\n<p id=\"result-count\" class=\"muted\" aria-live=\"polite\"></p>\n<p id=\"no-results\" class=\"muted\" hidden>No operations match.</p>\n<div id=\"operations\">\n");
    for operation in &contract.operations {
        let statuses = operation
            .responses
            .keys()
            .map(|status| escape_html(status))
            .collect::<Vec<_>>()
            .join(", ");
        html.push_str("<section data-search=\"");
        html.push_str(&escape_attr(&format!(
            "{} {} {} {}",
            operation.id,
            operation.method,
            operation.path,
            operation.description.as_deref().unwrap_or_default()
        )));
        html.push_str("\">\n<h2><code>");
        html.push_str(&escape_html(&operation.id));
        html.push_str("</code></h2>\n<p><strong>");
        html.push_str(&escape_html(&operation.method));
        html.push_str("</strong> <code>");
        html.push_str(&escape_html(&operation.path));
        html.push_str("</code></p>\n");
        if let Some(description) = &operation.description {
            html.push_str("<p>");
            html.push_str(&escape_html(description));
            html.push_str("</p>\n");
        }
        let detail = render_operation_detail(operation)?;
        html.push_str("<p>Parameters: ");
        html.push_str(&operation.parameters.len().to_string());
        html.push_str(". Responses: ");
        html.push_str(if statuses.is_empty() {
            "none"
        } else {
            &statuses
        });
        html.push_str(".</p>\n<details><summary>Parameters, body, and responses</summary>\n<p class=\"muted\">Resolved schemas are available in <a href=\"../contract.json\">contract.json</a>.</p>\n<pre><code>");
        html.push_str(&escape_html(&detail));
        html.push_str("</code></pre>\n</details>\n</section>\n");
    }
    html.push_str("</div>\n<script type=\"application/json\" id=\"forge-search\">");
    html.push_str(&escaped_search);
    html.push_str("</script>\n<script>const input=document.getElementById('search');const rows=[...document.querySelectorAll('[data-search]')];const count=document.getElementById('result-count');const empty=document.getElementById('no-results');function update(){const q=input.value.toLowerCase();let visible=0;for(const row of rows){const hit=row.dataset.search.toLowerCase().includes(q);row.hidden=!hit;if(hit)visible++;}count.textContent=visible+' of '+rows.length+' operations';empty.hidden=visible!==0;}input.addEventListener('input',update);update();</script>\n</body>\n</html>\n");
    Ok(html)
}

fn render_operation_detail(operation: &Operation) -> ForgeResult<String> {
    serde_json::to_string_pretty(operation)
        .map_err(|err| ForgeError(format!("serialize operation docs: {err}")))
}

pub(super) fn render_search(contract: &ResolvedOpenApi, digest: &str) -> ForgeResult<Vec<u8>> {
    let entries = contract
        .operations
        .iter()
        .map(|operation| SearchEntry {
            id: operation.id.as_str(),
            name: operation.name.as_str(),
            method: operation.method.as_str(),
            path: operation.path.as_str(),
            description: operation.description.as_deref().unwrap_or(""),
        })
        .collect::<Vec<_>>();
    let index = SearchIndex {
        namespace: contract.namespace.as_str(),
        digest,
        entries,
    };
    serde_json::to_vec_pretty(&index)
        .map_err(|err| ForgeError(format!("serialize docs search: {err}")))
}

fn escape_html(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect::<Vec<_>>(),
            '>' => "&gt;".chars().collect::<Vec<_>>(),
            '"' => "&quot;".chars().collect::<Vec<_>>(),
            '\'' => "&#39;".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

fn escape_attr(value: &str) -> String {
    escape_html(value)
}

#[derive(Serialize)]
struct SearchIndex<'a> {
    namespace: &'a str,
    digest: &'a str,
    entries: Vec<SearchEntry<'a>>,
}

#[derive(Serialize)]
struct SearchEntry<'a> {
    id: &'a str,
    name: &'a str,
    method: &'a str,
    path: &'a str,
    description: &'a str,
}
