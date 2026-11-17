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
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

/** Only external failures and empty Exa results enter the configured fallback chain. */
export async function searchWeb(params, settings, signal, request = post) {
  const errors = [];
  let exaResults;
  if (settings.exa_api_key) {
    try {
      const days = { day: 1, week: 7, month: 30, year: 365 }[params.recency];
      const data = await request('https://api.exa.ai/search', { 'x-api-key': settings.exa_api_key }, {
        query: params.query, type: 'auto', numResults: params.max_results ?? 5,
        contents: { highlights: true, text: false, summary: false },
        ...(params.include_domains?.length ? { includeDomains: params.include_domains } : {}),
        ...(days ? { startPublishedDate: new Date(Date.now() - days * 86400000).toISOString() } : {}),
      }, signal);
      exaResults = data.results.map(item => ({ title: item.title, url: item.url,
        published_date: item.publishedDate, summary: (item.highlights ?? []).join('\n'), provider: 'exa' }));
      if (exaResults.length) return { provider: 'exa', results: exaResults };
      errors.push('Exa: 无结果');
    } catch (error) {
      if (signal?.aborted) throw signal.reason;
      errors.push(`Exa: ${error.message}`);
    }
  } else errors.push('Exa: 未配置 API Key');
  signal?.throwIfAborted();
  if (!settings.baidu_qianfan_api_key) {
    if (exaResults) return { provider: 'exa', results: [], fallback: '百度未配置 API Key' };
    throw new Error([...errors, '百度千帆: 未配置 API Key'].join('; '));
  }
  const payload = { messages: [{ role: 'user', content: params.query }], search_source: 'baidu_search_v2',
    resource_type_filter: [{ type: 'web', top_k: params.max_results ?? 5 }] };
  if (params.include_domains?.length) payload.search_filter = { match: { site: params.include_domains } };
  if (params.recency === 'day') {
    const today = new Date(Date.now() + 8 * 3600000).toISOString().slice(0, 10);
    payload.search_filter = { ...payload.search_filter, range: { page_time: { gte: today, lte: today } } };
  } else if (params.recency) payload.search_recency_filter = params.recency;
  try {
    const data = await request('https://qianfan.baidubce.com/v2/ai_search/web_search',
      { 'X-Appbuilder-Authorization': `Bearer ${settings.baidu_qianfan_api_key}` }, payload, signal);
    return { provider: 'baidu_qianfan', fallback: errors.join('; '), results: data.references.map(item => ({
      title: item.title, url: item.url, published_date: item.date,
      summary: item.markdown_content ?? item.content ?? item.snippet ?? '', provider: 'baidu_qianfan',
    })) };
  } catch (error) {
    if (signal?.aborted) throw signal.reason;
    throw new Error([...errors, `百度千帆: ${error.message}`].join('; '));
  }
}

export function webTools(getSettings, cwd, dataRoot) {
  return [
    { name: 'web_search', label: 'Web Search', description: 'Search the web with Exa, falling back to Baidu on failure or empty results. Returns source URLs and excerpts.',
      parameters: Type.Object({ query: Type.String({ minLength: 1 }), max_results: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })),
        recency: Type.Optional(Type.Union(['day', 'week', 'month', 'year'].map(v => Type.Literal(v)))),
        include_domains: Type.Optional(Type.Array(Type.String())) }),
      execute: async (_id, params, signal) => {
        const result = await searchWeb(params, { ...getSettings() }, signal);
        return { content: [{ type: 'text', text: JSON.stringify(result) }], details: result };
      },
    },
    { name: 'web_fetch', label: 'Web Fetch', description: 'Fetch a URL as Markdown. PDFs are detected before HTML parsing. Use mode=dynamic explicitly for JavaScript sites. Scanned PDF results include a local file and OCR page numbers; continue with the independent PaddleOCR MCP tool or render selected pages.',
      parameters: Type.Object({ url: Type.String({ format: 'uri' }),
        mode: Type.Optional(Type.Union([Type.Literal('http'), Type.Literal('dynamic')])),
        pages: Type.Optional(Type.String({ description: '1-based PDF pages, e.g. 1,3-5' })) }),
      execute: async (_id, params, signal) => {
        const output = path.join(dataRoot, 'ai-artifacts', randomUUID());
        fs.mkdirSync(output, { recursive: true });
        const script = fileURLToPath(new URL('./fetch.py', import.meta.url));
        const python = process.env.FOLIO_FETCH_PYTHON ?? path.join(path.dirname(script), '.venv', 'Scripts', 'python.exe');
        const result = await new Promise((resolve, reject) => {
          const child = spawn(python, [script, '--url', params.url, '--mode', params.mode ?? 'http', '--output', output,
            ...(params.pages ? ['--pages', params.pages] : [])], { cwd, windowsHide: true, signal });
          let stdout = '', stderr = '';
          child.stdout.on('data', chunk => { stdout += chunk; });
          child.stderr.on('data', chunk => { stderr += chunk; });
          child.once('error', reject);
          child.once('close', code => code === 0 ? resolve(stdout) : reject(new Error(`web_fetch: ${stderr || `exit ${code}`}`)));
        });
        const parsed = JSON.parse(result);
        return { content: [{ type: 'text', text: JSON.stringify(parsed) }], details: parsed };
      },
    },
  ];
}
