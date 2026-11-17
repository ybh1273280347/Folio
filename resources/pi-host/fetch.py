"""Fetch one external document, route PDF bytes before HTML parsing, and retain Markdown artifacts."""
import argparse
import json
from pathlib import Path
from urllib.parse import urlparse

import httpx
import pdf_inspector
from scrapling.engines.toolbelt.custom import Response
from scrapling.fetchers import StealthyFetcher


def selected_pages(value: str | None) -> list[int] | None:
    """Validate one-based page selections for pdf-inspector's process_pdf API."""
    if value is None:
        return None
    pages = set()
    for item in value.split(","):
        bounds = item.strip().split("-")
        start = int(bounds[0])
        end = int(bounds[-1])
        if start < 1 or end < start or len(bounds) > 2:
            raise ValueError("页码必须是从 1 开始的数字或范围")
        pages.update(range(start, end + 1))
    return sorted(pages)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--mode", choices=["http", "dynamic"], default="http")
    parser.add_argument("--pages")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if urlparse(args.url).scheme not in {"http", "https"}:
        raise ValueError("web_fetch 只接受 HTTP(S) URL")
    args.output.mkdir(parents=True, exist_ok=True)
    # Read raw bytes before creating a Scrapling selector: extension-less download URLs can be PDFs.
    with httpx.Client(follow_redirects=True, timeout=35) as client:
        response = client.get(args.url, headers={"User-Agent": "Mozilla/5.0 Folio/1.0"})
        response.raise_for_status()
    data = response.content
    result = {"url": str(response.url), "title": "", "needs_ocr_pages": []}
    content_type = response.headers.get("content-type", "").lower()
    pdf_download = urlparse(str(response.url)).path.lower().endswith(".pdf") and content_type in {"", "application/octet-stream"}
    is_pdf = data.lstrip().startswith(b"%PDF-") or "application/pdf" in content_type or pdf_download
    if is_pdf:
        pdf_path = args.output / "source.pdf"
        pdf_path.write_bytes(data)
        extracted = pdf_inspector.process_pdf(str(pdf_path), pages=selected_pages(args.pages))
        markdown = extracted.markdown or ""
        result.update(kind="pdf", pdf_path=str(pdf_path.resolve()), pdf_type=extracted.pdf_type,
                      page_count=extracted.page_count, title=extracted.title or "")
        result["needs_ocr_pages"] = extracted.pages_needing_ocr
        if result["needs_ocr_pages"]:
            result["next_step"] = "使用独立 PaddleOCR MCP 的 paddleocr_vl 工具处理 pdf_path，或渲染指定页面读图。"
    else:
        if args.mode == "dynamic":
            page = StealthyFetcher.fetch(str(response.url), headless=True, network_idle=True, google_search=False)
        else:
            page = Response(str(response.url), data, response.status_code, response.reason_phrase,
                            dict(response.cookies), dict(response.headers), dict(response.request.headers))
        result.update(kind="html", title=page.css("title::text").get() or "")
        markdown = page.markdown(main_content_only=True)
    markdown_path = args.output / "content.md"
    markdown_path.write_text(markdown, encoding="utf-8")
    result.update(markdown=markdown[:30000], truncated=len(markdown) > 30000,
                  markdown_path=str(markdown_path.resolve()))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
