"""Fetch one external document, route PDF bytes before HTML parsing, and retain Markdown artifacts."""
import argparse
import json
import re
from pathlib import Path
from urllib.parse import urlparse

import httpx
import pdf_inspector
from patchright.sync_api import Error as BrowserError, TimeoutError as BrowserTimeoutError
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


def failure(kind: str, message: str, retryable: bool = False, **details) -> None:
    """Emit the expected failure contract; diagnostic tracebacks never belong in stdout."""
    print(json.dumps({"status": "error", "error": {"type": kind, "message": message,
                                                    "retryable": retryable, **details}}, ensure_ascii=False))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", required=True)
    parser.add_argument("--mode", choices=["http", "dynamic"], default="http")
    parser.add_argument("--pages")
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if urlparse(args.url).scheme not in {"http", "https"}:
        raise ValueError("web_fetch 只接受 HTTP(S) URL")
    pages = selected_pages(args.pages)
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
        try:
            extracted = pdf_inspector.process_pdf(str(pdf_path), pages=pages)
        except (ValueError, RuntimeError) as error:
            message = str(error).partition("\n")[0][:200]
            failure("parse_error", f"PDF 转换失败：{message}", pdf_path=str(pdf_path.resolve()))
            return
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
    result.update(status="ok" if markdown.strip() or result["needs_ocr_pages"] else "empty",
                  markdown=markdown[:30000], truncated=len(markdown) > 30000,
                  markdown_path=str(markdown_path.resolve()))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    # Translate only known external failures. Unexpected bugs retain their traceback on stderr.
    try:
        main()
    except httpx.TimeoutException:
        failure("timeout", "连接或读取网页超时。", True)
    except httpx.NetworkError:
        failure("connection_error", "无法连接网页，请检查网络、代理、DNS 或证书。", True)
    except httpx.HTTPStatusError as error:
        status = error.response.status_code
        kind = {401: "authentication_error", 402: "quota_exceeded", 403: "permission_denied", 429: "rate_limited"}.get(
            status, "remote_error" if status >= 500 else "invalid_request")
        failure(kind, f"网页返回 HTTP {status}。", status == 429 or status >= 500, http_status=status)
    except (httpx.ProtocolError, httpx.TooManyRedirects):
        failure("invalid_response", "网页响应协议异常或重定向次数过多。")
    except (httpx.InvalidURL, ValueError) as error:
        failure("invalid_request", str(error).partition("\n")[0][:200])
    except BrowserTimeoutError:
        failure("timeout", "动态网页加载超时。", True)
    except BrowserError as error:
        network = re.search(r"net::(ERR_[A-Z_]+)", str(error))
        if network:
            failure("connection_error", f"动态网页连接失败（{network.group(1)}）。", True)
        else:
            failure("browser_error", "动态浏览器抓取失败，请检查浏览器依赖或改用 HTTP 模式。")
    except OSError as error:
        failure("file_error", f"读写抓取产物失败（{type(error).__name__}）。")
