/** Network tools keep credentials and provider failures outside model-visible arguments. */
import { Type } from '@sinclair/typebox';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import fs from 'node:fs';
import { randomUUID } from 'node:crypto';

async function post(url, headers, body, signal) {
  const response = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', ...headers },
    body: JSON.stringify(body), signal: AbortSignal.any([signal ?? new AbortController().signal, AbortSignal.timeout(25000)]) });
  if (!response.ok) throw new Error(`${new URL(url).hostname}: HTTP ${response.status}`);
  return response.json();
}

/** Keep ranked evidence and provider metadata, deduplicating only candidates with a URL. */
function searchResult(provider, query, mode, candidates, maxResults) {
  const seen = new Set();
  const unique = candidates.filter(candidate => {
    if (!candidate.url) return true;
    if (seen.has(candidate.url)) return false;
    seen.add(candidate.url);
    return true;
  }).slice(0, maxResults);
  return { provider, query, mode, candidates: unique,
    ...(unique.length ? {} : { notice: '没有找到结果。可调整查询，或尝试另一个搜索工具；请保留所需的来源和学术范围约束。' }) };
}

export async function searchExa(params, settings, signal, request = post) {
  signal?.throwIfAborted();
  if (!settings.exaApiKey) throw new Error('Exa: 未配置 API Key。可使用 baidu_qianfan_search 搜索网页。');
  const query = params.query.trim();
  if (!query) throw new Error('query 不能为空白。');
  const maxResults = params.max_results ?? 10;
  const mode = params.mode ?? 'web';
  const days = { day: 1, week: 7, month: 30, year: 365 }[params.recency];
  const data = await request('https://api.exa.ai/search', { 'x-api-key': settings.exaApiKey }, {
    query, type: 'auto', numResults: maxResults,
    ...(mode === 'academic' ? { category: 'research paper' } : {}),
    // Retrieval terms and focused evidence extraction are separate provider capabilities.
    contents: { highlights: params.focus?.trim() ? { query: params.focus.trim() } : true, text: false, summary: false },
    ...(params.include_domains?.length ? { includeDomains: params.include_domains.map(domain => domain.trim().toLowerCase()) } : {}),
    ...(params.exclude_domains?.length ? { excludeDomains: params.exclude_domains.map(domain => domain.trim().toLowerCase()) } : {}),
    ...(days ? { startPublishedDate: new Date(Date.now() - days * 86400000).toISOString() } : {}),
  }, signal);
  signal?.throwIfAborted();
  return searchResult('exa', query, mode, data.results.map(item => ({ title: item.title, url: item.url,
    published_date: item.publishedDate, evidences: item.highlights ?? [],
    metadata: { author: item.author, highlight_scores: item.highlightScores, entities: item.entities },
  })), maxResults);
}

export async function searchBaidu(params, settings, signal, request = post) {
  signal?.throwIfAborted();
  if (!settings.baiduQianfanApiKey) throw new Error('百度千帆: 未配置 API Key。');
  const query = params.query.trim();
  if (!query) throw new Error('query 不能为空白。');
  const maxResults = params.max_results ?? 10;
  const payload = { messages: [{ role: 'user', content: query }], search_source: 'baidu_search_v2',
    resource_type_filter: [{ type: 'web', top_k: maxResults }] };
  if (params.include_domains?.length) payload.search_filter = { match: { site: params.include_domains.map(domain => domain.trim().toLowerCase()) } };
  if (params.recency === 'day') {
    const today = new Date(Date.now() + 8 * 3600000).toISOString().slice(0, 10);
    payload.search_filter = { ...payload.search_filter, range: { page_time: { gte: today, lte: today } } };
  } else if (params.recency) payload.search_recency_filter = params.recency;
  const data = await request('https://qianfan.baidubce.com/v2/ai_search/web_search',
    { 'X-Appbuilder-Authorization': `Bearer ${settings.baiduQianfanApiKey}` }, payload, signal);
  signal?.throwIfAborted();
  return searchResult('baidu_qianfan', query, 'web', data.references.map(item => ({ title: item.title, url: item.url,
    published_date: item.date, evidences: [item.markdown_content || item.content || item.snippet].filter(Boolean),
    metadata: { authority_score: item.authority_score, rerank_score: item.rerank_score, aladdin: item.aladdin },
  })), maxResults);
}

const domainList = Type.Array(Type.String({ pattern: '^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:\\.[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?)+$' }),
  { description: 'Hostnames and their subdomains, e.g. python.org. No URLs, paths, ports or wildcards. [] means unrestricted.' });
const searchParameters = {
  query: Type.String({ minLength: 1, description: 'Concise search terms targeting the needed information.' }),
  max_results: Type.Optional(Type.Integer({ minimum: 1, maximum: 20, default: 10, description: 'Maximum ranked candidates to return; default 10.' })),
  recency: Type.Optional(Type.Union(['day', 'week', 'month', 'year'].map(v => Type.Literal(v)),
    { description: 'Soft freshness filter; omit for unrestricted search. Exact date requirements should also be stated in query.' })),
  include_domains: Type.Optional(domainList),
};
const evidenceRules = 'Treat candidates[].evidences as factual grounding; inspect titles, URLs, dates and metadata for relevance and authority. Refine weak searches instead of inventing facts. Cite supporting sources beside the relevant claim using [short source label](exact URL). A provider summary is only a lead, not independent evidence.';

export function webTools(getSettings, cwd, dataRoot) {
  return [
    { name: 'exa_search', label: 'Exa Search', description: `Preferred web and academic search. mode=academic retrieves research papers and preprints. focus targets a specific fact or passage for evidence extraction while query drives retrieval. Both modes support include_domains, exclude_domains and recency. On failure or no results, try baidu_qianfan_search when its web scope satisfies the request; never silently drop academic or excluded-domain requirements. ${evidenceRules}`,
      parameters: Type.Object({ ...searchParameters,
        mode: Type.Optional(Type.Union([Type.Literal('web'), Type.Literal('academic')], { default: 'web', description: 'web for general sources; academic for scholarly publications.' })),
        focus: Type.Optional(Type.String({ description: 'Specific fact, metric, comparison or passage to prioritize within matched sources.' })),
        exclude_domains: Type.Optional(domainList),
      }, { additionalProperties: false }),
      execute: async (_id, params, signal) => {
        const result = await searchExa(params, { ...getSettings() }, signal);
        return { content: [{ type: 'text', text: JSON.stringify(result) }], details: result };
      },
    },
    { name: 'baidu_qianfan_search', label: 'Baidu Qianfan Search', description: `Search with Baidu Qianfan, especially when Chinese sources are important, or as a fallback after Exa fails or returns no results. Supports web search, include_domains and recency. Does not support academic mode, focus or exclude_domains. recency=day means the current Beijing calendar day; week/month/year follow the provider. ${evidenceRules}`,
      parameters: Type.Object(searchParameters, { additionalProperties: false }),
      execute: async (_id, params, signal) => {
        const result = await searchBaidu(params, { ...getSettings() }, signal);
        return { content: [{ type: 'text', text: JSON.stringify(result) }], details: result };
      },
    },
    { name: 'web_fetch', label: 'Web Fetch', description: 'Fetch a URL as Markdown. PDFs are detected before HTML parsing. Use mode=dynamic explicitly for JavaScript sites. Scanned PDF results include a local file and OCR page numbers; continue with the independent PaddleOCR MCP tool or render selected pages.',
      parameters: Type.Object({ url: Type.String({ format: 'uri' }),
        mode: Type.Optional(Type.Union([Type.Literal('http'), Type.Literal('dynamic')])),
        pages: Type.Optional(Type.String({ description: '1-based PDF pages, e.g. 1,3-5' })) }),
      execute: async (_id, params, signal) => {
        signal?.throwIfAborted();
        const output = path.join(dataRoot, 'ai-artifacts', randomUUID());
        fs.mkdirSync(output, { recursive: true });
        const script = fileURLToPath(new URL('./fetch.py', import.meta.url));
        const python = process.env.FOLIO_FETCH_PYTHON ?? path.join(path.dirname(script), '.venv', 'Scripts', 'python.exe');
        const result = await new Promise((resolve, reject) => {
          const child = spawn(python, [script, '--url', params.url, '--mode', params.mode ?? 'http', '--output', output,
            ...(params.pages ? ['--pages', params.pages] : [])], { cwd, windowsHide: true,
              env: { ...process.env, PYTHONIOENCODING: 'utf-8' } });
          let stdout = '', stderr = '';
          // Killing the Python parent alone can leave its dynamic Chromium child behind on Windows.
          const abort = () => {
            if (process.platform === 'win32' && child.pid) {
              const killer = spawn('taskkill', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
              killer.once('error', reject);
            } else child.kill();
          };
          signal?.addEventListener('abort', abort, { once: true });
          child.stdout.on('data', chunk => { stdout += chunk; });
          child.stderr.on('data', chunk => { stderr += chunk; });
          child.once('error', error => { signal?.removeEventListener('abort', abort); reject(error); });
          child.once('close', code => {
            signal?.removeEventListener('abort', abort);
            if (signal?.aborted) reject(signal.reason);
            else if (code === 0) resolve(stdout);
            else reject(new Error(`web_fetch: ${stderr || `exit ${code}`}`));
          });
        });
        const parsed = JSON.parse(result);
        return { content: [{ type: 'text', text: JSON.stringify(parsed) }], details: parsed };
      },
    },
  ];
}
