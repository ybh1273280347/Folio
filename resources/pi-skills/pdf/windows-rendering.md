## Windows: render selected pages for visual reading

Use text extraction for ordinary reading. For figures, formulas, layout inspection, or incomplete extraction, render the relevant pages and open the returned PNG paths:

```bash
python scripts/render_pages.py "C:/path/report.pdf" --pages "1,3-5" --output "C:/path/report-preview" --dpi 144
```

Pages are **one-based**; omit `--pages` only to render every page. JSON output contains page numbers and **absolute image paths**. Read the images with the image-reading tool. Requires `pypdfium2` and `Pillow`. Scanned or complex PDFs can also be sent to the independently configured PaddleOCR MCP tool for Markdown extraction. This supplements the existing PDF workflows.
